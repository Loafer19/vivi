//! Remote Control (#58): a small HTTP server with a phone-friendly page (play/pause,
//! seek, volume, previous/next, queue, now-playing art) for devices on the local network.
//!
//! * Off by default; port and PIN in Settings (a random 6-digit PIN is created when the
//!   PIN is empty).
//! * LAN only: connections from addresses that are not loopback, private (RFC 1918 /
//!   unique-local) or link-local are dropped before reading the request.
//! * Every API call needs the PIN (`X-Vivi-Pin` header, or `?pin=` for the art image);
//!   five wrong PINs from one address block it for a minute.
//!
//! The server runs on its own thread and only reads a [`Snapshot`] the UI thread updates
//! in [`Plugin::tick`]; commands go back over a channel and are applied on the UI thread.

use super::{Ctx, Manifest, Plugin, SettingSpec};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ID: &str = "remote-control";
const PAGE: &str = include_str!("remote_page.html");
const MAX_QUEUE: usize = 300;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Snapshot {
    pub title: String,
    pub artist: String,
    pub path: String,
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f64,
    pub volume_max: f64,
    pub queue: Vec<String>,
    pub current: i64,
    pub has_art: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Pause,
    Seek(f64),
    SeekRel(f64),
    Volume(f64),
    Next,
    Prev,
    PlayIndex(i64),
}

impl Cmd {
    pub fn parse(v: &serde_json::Value) -> Option<Self> {
        let n = v["value"].as_f64().unwrap_or(0.0);
        if !n.is_finite() {
            return None;
        }
        Some(match v["cmd"].as_str()? {
            "pause" => Self::Pause,
            "seek" => Self::Seek(n.max(0.0)),
            "seek-rel" => Self::SeekRel(n.clamp(-600.0, 600.0)),
            "volume" => Self::Volume(n.clamp(0.0, 1000.0)),
            "next" => Self::Next,
            "prev" => Self::Prev,
            "play-index" => Self::PlayIndex(n as i64),
            _ => return None,
        })
    }
}

/// Loopback, private, unique-local or link-local address.
pub fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_lan(IpAddr::V4(v4));
            }
            let s = v.segments()[0];
            v.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    }
}

/// Length-independent comparison of the PIN.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut d = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        d |= (*a.get(i).unwrap_or(&0) ^ *b.get(i).unwrap_or(&1)) as usize;
    }
    d == 0
}

/// The address phones should open (the LAN IP of the default route; no packet is sent).
pub fn lan_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.255.255:9").ok()?;
    Some(s.local_addr().ok()?.ip())
}

fn random_pin() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:06}", h.finish() % 1_000_000)
}

struct Shared {
    snap: Mutex<Snapshot>,
    pin: Mutex<String>,
    stop: AtomicBool,
    fails: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match s
                    .get(i + 1..i + 3)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or(())
                {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn read_request(s: &mut TcpStream) -> Option<Request> {
    let mut r = BufReader::new(s.take(64 * 1024));
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut it = line.split_whitespace();
    let method = it.next()?.to_string();
    let target = it.next()?.to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let n: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
        .min(8192);
    let mut body = vec![0; n];
    r.read_exact(&mut body).ok()?;
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect();
    Some(Request {
        method,
        path: path.to_string(),
        query,
        headers,
        body,
    })
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(body);
}

/// Embedded cover (lofty) or a cover/folder/front image next to the file.
pub fn art(path: &Path) -> Option<(Vec<u8>, &'static str)> {
    use lofty::file::TaggedFileExt;
    if let Ok(f) = lofty::read_from_path(path) {
        if let Some(p) = f
            .primary_tag()
            .or(f.first_tag())
            .and_then(|t| t.pictures().first())
        {
            let mime = if p.data().starts_with(&[0x89, b'P']) {
                "image/png"
            } else {
                "image/jpeg"
            };
            return Some((p.data().to_vec(), mime));
        }
    }
    let dir = path.parent()?;
    for n in ["cover", "folder", "front", "Cover", "Folder", "Front"] {
        for (ext, mime) in [
            ("jpg", "image/jpeg"),
            ("jpeg", "image/jpeg"),
            ("png", "image/png"),
        ] {
            let p: PathBuf = dir.join(format!("{n}.{ext}"));
            if let Ok(b) = std::fs::read(&p) {
                return Some((b, mime));
            }
        }
    }
    None
}

fn handle(mut s: TcpStream, sh: &Shared, tx: &Sender<Cmd>) {
    let Ok(peer) = s.peer_addr() else { return };
    if !is_lan(peer.ip()) {
        return; // dropped without an answer
    }
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let Some(req) = read_request(&mut s) else {
        return;
    };
    if req.method == "GET" && (req.path == "/" || req.path == "/index.html") {
        return respond(
            &mut s,
            "200 OK",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        );
    }
    if !req.path.starts_with("/api/") {
        return respond(&mut s, "404 Not Found", "text/plain", b"not found");
    }
    {
        let mut f = sh.fails.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((n, t)) = f.get(&peer.ip()) {
            if *n >= 5 && t.elapsed() < Duration::from_secs(60) {
                return respond(&mut s, "429 Too Many Requests", "text/plain", b"wait");
            }
        }
        let given = req
            .headers
            .get("x-vivi-pin")
            .or(req.query.get("pin"))
            .cloned()
            .unwrap_or_default();
        let pin = sh.pin.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if pin.is_empty() || !same(&given, &pin) {
            let e = f.entry(peer.ip()).or_insert((0, Instant::now()));
            if e.1.elapsed() > Duration::from_secs(60) {
                *e = (0, Instant::now());
            }
            e.0 += 1;
            e.1 = Instant::now();
            return respond(&mut s, "401 Unauthorized", "text/plain", b"pin");
        }
        f.remove(&peer.ip());
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/state") => {
            let j = serde_json::to_vec(&*sh.snap.lock().unwrap_or_else(|e| e.into_inner()))
                .unwrap_or_default();
            respond(&mut s, "200 OK", "application/json", &j);
        }
        ("GET", "/api/art") => {
            let p = sh
                .snap
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .path
                .clone();
            match (!p.contains("://")).then(|| art(Path::new(&p))).flatten() {
                Some((b, m)) => respond(&mut s, "200 OK", m, &b),
                None => respond(&mut s, "404 Not Found", "text/plain", b"no art"),
            }
        }
        ("POST", "/api/cmd") => {
            // JSON body only: a cross-site form post cannot send application/json.
            let json = req
                .headers
                .get("content-type")
                .is_some_and(|c| c.starts_with("application/json"));
            match serde_json::from_slice(&req.body)
                .ok()
                .filter(|_| json)
                .as_ref()
                .and_then(Cmd::parse)
            {
                Some(c) => {
                    let _ = tx.send(c);
                    respond(&mut s, "204 No Content", "text/plain", b"");
                }
                None => respond(&mut s, "400 Bad Request", "text/plain", b"bad command"),
            }
        }
        _ => respond(&mut s, "404 Not Found", "text/plain", b"not found"),
    }
}

fn serve(l: TcpListener, sh: Arc<Shared>, tx: Sender<Cmd>) {
    let _ = l.set_nonblocking(true);
    while !sh.stop.load(Ordering::Relaxed) {
        match l.accept() {
            Ok((s, _)) => {
                let _ = s.set_nonblocking(false);
                let (sh, tx) = (sh.clone(), tx.clone());
                std::thread::spawn(move || handle(s, &sh, &tx));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

#[derive(Default)]
pub struct Remote {
    shared: Option<Arc<Shared>>,
    rx: Option<Receiver<Cmd>>,
    url: String,
    poll: u32,
}

impl Remote {
    fn stop(&mut self) {
        if let Some(s) = self.shared.take() {
            s.stop.store(true, Ordering::Relaxed);
        }
        self.rx = None;
    }
    fn pin(cx: &Ctx) -> String {
        let p = cx.get("pin").trim().to_string();
        if p.is_empty() {
            let p = random_pin();
            cx.set("pin", &p);
            p
        } else {
            p
        }
    }
    fn start(&mut self, cx: &Ctx) {
        self.stop();
        let port: u16 = cx.get("port").trim().parse().unwrap_or(8765);
        let l = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => {
                cx.toast(&format!("Remote control: port {port}: {e}"));
                return;
            }
        };
        let pin = Self::pin(cx);
        let sh = Arc::new(Shared {
            snap: Mutex::new(Snapshot::default()),
            pin: Mutex::new(pin.clone()),
            stop: AtomicBool::new(false),
            fails: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = channel();
        let s2 = sh.clone();
        std::thread::spawn(move || serve(l, s2, tx));
        self.shared = Some(sh);
        self.rx = Some(rx);
        let host = lan_ip()
            .map(|i| i.to_string())
            .unwrap_or("<this computer>".into());
        self.url = format!("http://{host}:{port}");
        eprintln!("vivi: remote control on {}", self.url);
        cx.toast(&format!(
            "Remote control: open {} on your phone, PIN {pin}",
            self.url
        ));
    }
    fn snapshot(cx: &Ctx) -> Snapshot {
        let p = cx.player;
        let f = |k: &str| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let n = (f("playlist-count") as usize).min(MAX_QUEUE);
        let queue = (0..n)
            .map(|i| {
                p.get(&format!("playlist/{i}/title"))
                    .or_else(|| {
                        p.get(&format!("playlist/{i}/filename"))
                            .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(&f).to_string())
                    })
                    .unwrap_or_default()
            })
            .collect();
        let path = p.get("path").unwrap_or_default();
        let has_art = !path.is_empty()
            && !path.contains("://")
            && (p.get("current-tracks/video/albumart").as_deref() == Some("yes")
                || Path::new(&path).parent().is_some_and(|d| {
                    ["cover.jpg", "folder.jpg", "front.jpg", "cover.png"]
                        .iter()
                        .any(|n| d.join(n).is_file())
                }));
        Snapshot {
            title: p
                .get("metadata/by-key/title")
                .or_else(|| p.get("media-title"))
                .unwrap_or_default(),
            artist: p.get("metadata/by-key/artist").unwrap_or_default(),
            path,
            paused: p.get("pause").as_deref() == Some("yes"),
            position: f("time-pos"),
            duration: f("duration"),
            volume: f("volume"),
            volume_max: f("volume-max"),
            queue,
            current: p
                .get("playlist-pos")
                .and_then(|v| v.parse().ok())
                .unwrap_or(-1),
            has_art,
        }
    }
    fn apply(cx: &Ctx, c: Cmd) {
        let p = cx.player;
        match c {
            Cmd::Pause => p.cmd("cycle", &["pause"]),
            Cmd::Seek(t) => p.cmd("seek", &[&format!("{t:.2}"), "absolute"]),
            Cmd::SeekRel(t) => p.cmd("seek", &[&format!("{t:.2}"), "relative"]),
            Cmd::Volume(v) => p.set("volume", &format!("{v:.0}")),
            Cmd::Next => p.playlist_step(true),
            Cmd::Prev => p.playlist_step(false),
            Cmd::PlayIndex(i) if i >= 0 => p.cmd("playlist-play-index", &[&i.to_string()]),
            Cmd::PlayIndex(_) => {}
        }
    }
}

impl Plugin for Remote {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Remote Control",
            "Control playback from your phone: a web page on your local network with play/pause, seek, volume, next/previous, the queue and cover art. PIN protected; only devices on the local network can connect.",
            "remote",
        )
        .setting(SettingSpec::text(
            "port",
            "Port",
            "TCP port of the web page (restart the plugin after changing it).",
            "8765",
            "8765",
        ))
        .setting(SettingSpec::text(
            "pin",
            "PIN",
            "Asked once by the phone. Leave empty to get a new random 6-digit PIN.",
            "random",
            "",
        ))
        .setting(SettingSpec::action(
            "address",
            "Phone address",
            "Shows the address to open on your phone and the PIN.",
            "Show",
        ))
        .default_off()
    }
    fn enable(&mut self, cx: &Ctx) {
        self.start(cx);
    }
    fn disable(&mut self, _cx: &Ctx) {
        self.stop();
    }
    fn setting_changed(&mut self, cx: &Ctx, key: &str) {
        match key {
            "port" => self.start(cx),
            "pin" => {
                let pin = Self::pin(cx);
                if let Some(s) = &self.shared {
                    *s.pin.lock().unwrap_or_else(|e| e.into_inner()) = pin;
                }
            }
            _ => {}
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, _args: &[String]) {
        if name == "address" {
            if self.shared.is_some() {
                cx.toast(&format!(
                    "Open {} on your phone, PIN {}",
                    self.url,
                    Self::pin(cx)
                ));
            } else {
                cx.toast("Remote control is not running");
            }
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        if let Some(rx) = &self.rx {
            while let Ok(c) = rx.try_recv() {
                Self::apply(cx, c);
            }
        }
        self.poll += 1;
        if self.poll.is_multiple_of(5) {
            if let Some(s) = &self.shared {
                let snap = Self::snapshot(cx);
                *s.snap.lock().unwrap_or_else(|e| e.into_inner()) = snap;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_only() {
        for a in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.5",
            "172.20.0.1",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:192.168.0.2",
        ] {
            assert!(is_lan(a.parse().unwrap()), "{a}");
        }
        for a in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "::ffff:1.1.1.1"] {
            assert!(!is_lan(a.parse().unwrap()), "{a}");
        }
    }

    #[test]
    fn commands_and_pin() {
        let c = |s: &str| Cmd::parse(&serde_json::from_str(s).unwrap());
        assert_eq!(c(r#"{"cmd":"seek","value":12.5}"#), Some(Cmd::Seek(12.5)));
        assert_eq!(
            c(r#"{"cmd":"volume","value":5000}"#),
            Some(Cmd::Volume(1000.0))
        );
        assert_eq!(c(r#"{"cmd":"pause"}"#), Some(Cmd::Pause));
        assert_eq!(c(r#"{"cmd":"quit"}"#), None);
        assert!(same("123456", "123456"));
        assert!(!same("123456", "12345"));
        assert!(!same("", "1"));
        assert_eq!(random_pin().len(), 6);
        assert_eq!(decode("a%20b+c%2"), "a b c%2");
    }

    fn http(port: u16, req: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    /// The real server on localhost: page without PIN, API with PIN, lockout, commands.
    #[test]
    fn server_end_to_end() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let sh = Arc::new(Shared {
            snap: Mutex::new(Snapshot {
                title: "Song".into(),
                ..Default::default()
            }),
            pin: Mutex::new("4242".into()),
            stop: AtomicBool::new(false),
            fails: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = channel();
        let s2 = sh.clone();
        std::thread::spawn(move || serve(l, s2, tx));
        assert!(http(port, "GET / HTTP/1.1\r\n\r\n").contains("Vivi Remote"));
        assert!(http(port, "GET /api/state HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 401"));
        let ok = http(port, "GET /api/state HTTP/1.1\r\nX-Vivi-Pin: 4242\r\n\r\n");
        assert!(
            ok.starts_with("HTTP/1.1 200") && ok.contains("\"title\":\"Song\""),
            "{ok}"
        );
        let body = r#"{"cmd":"seek","value":30}"#;
        let r = http(port, &format!("POST /api/cmd HTTP/1.1\r\nX-Vivi-Pin: 4242\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
        assert!(r.starts_with("HTTP/1.1 204"), "{r}");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Cmd::Seek(30.0)
        );
        // Form posts (no JSON content type) are refused.
        let r = http(port, &format!("POST /api/cmd HTTP/1.1\r\nX-Vivi-Pin: 4242\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
        assert!(r.starts_with("HTTP/1.1 400"), "{r}");
        for _ in 0..5 {
            http(port, "GET /api/state HTTP/1.1\r\nX-Vivi-Pin: 0000\r\n\r\n");
        }
        let r = http(port, "GET /api/state HTTP/1.1\r\nX-Vivi-Pin: 4242\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 429"), "{r}");
        sh.stop.store(true, Ordering::Relaxed);
    }
}
