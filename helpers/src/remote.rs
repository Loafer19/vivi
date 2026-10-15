//! Remote Control server (`remote-control` plugin): a phone-friendly page (play/pause,
//! seek, volume, previous/next, queue, cover art) for devices on the local network.
//!
//! * LAN only: connections from addresses that are not loopback, private (RFC 1918 /
//!   unique-local) or link-local are dropped before reading the request.
//! * Every API call needs the PIN (`X-Vivi-Pin` header, or `?pin=` for the art image);
//!   five wrong PINs from one address block it for a minute.
//!
//! Server threads only read the [`Snapshot`]; commands go back over a channel to the
//! helper's main loop, which owns the mpv IPC connection.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("remote_page.html");
pub const MAX_QUEUE: usize = 300;

#[derive(Debug, Clone, Default, PartialEq)]
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

impl Snapshot {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "title": self.title, "artist": self.artist, "path": self.path,
            "paused": self.paused, "position": self.position, "duration": self.duration,
            "volume": self.volume, "volume_max": self.volume_max, "queue": self.queue,
            "current": self.current, "has_art": self.has_art,
        })
    }
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

pub fn random_pin() -> String {
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

pub struct Shared {
    pub snap: Mutex<Snapshot>,
    /// Cover of the current file (bytes, MIME type).
    pub art: Mutex<Option<(Vec<u8>, &'static str)>>,
    pub pin: Mutex<String>,
    pub stop: AtomicBool,
    pub fails: Mutex<HashMap<IpAddr, (u32, Instant)>>,
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
            let j =
                serde_json::to_vec(&sh.snap.lock().unwrap_or_else(|e| e.into_inner()).to_json())
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
            let _ = p;
            let a = sh.art.lock().unwrap_or_else(|e| e.into_inner()).clone();
            match a {
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

pub fn serve(l: TcpListener, sh: Arc<Shared>, tx: Sender<Cmd>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

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
            art: Mutex::new(None),
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
