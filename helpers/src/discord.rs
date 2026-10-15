//! Discord Rich Presence over Discord's local IPC (`discord-ipc-N` socket or named pipe).
//! Frames are `op: u32 LE, len: u32 LE, JSON`; op 0 = handshake, 1 = frame, 2 = close.

use serde_json::{json, Value};
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::pipe::{self, Stream};

/// What to show (`None` clears the activity).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Presence {
    pub details: String,
    pub state: String,
    pub audio: bool,
    pub paused: bool,
    /// Unix ms when playback (re)started at position 0, while playing.
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub image: String,
}

impl Presence {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: &str,
        artist: &str,
        audio: bool,
        paused: bool,
        pos: f64,
        dur: f64,
        now_ms: i64,
        hide: bool,
        image: &str,
    ) -> Self {
        let (details, state) = if hide {
            let d = if audio {
                "Listening to music"
            } else {
                "Watching a video"
            };
            (d.to_string(), String::new())
        } else {
            (title.to_string(), artist.to_string())
        };
        let state = match (paused, state.is_empty()) {
            (true, true) => "Paused".into(),
            (true, false) => format!("{state} · Paused"),
            _ => state,
        };
        let (start, end) = if paused {
            (None, None)
        } else {
            let start = now_ms - (pos * 1000.0) as i64;
            (
                Some(start),
                (dur > 0.0).then(|| start + (dur * 1000.0) as i64),
            )
        };
        Self {
            details: clip(&details),
            state: clip(&state),
            audio,
            paused,
            start,
            end,
            image: image.trim().to_string(),
        }
    }

    /// Same text and pause state, start within 2 s (no seek).
    pub fn same(&self, o: &Self) -> bool {
        self.details == o.details
            && self.state == o.state
            && self.paused == o.paused
            && match (self.start, o.start) {
                (Some(x), Some(y)) => (x - y).abs() < 2000,
                (None, None) => true,
                _ => false,
            }
    }

    pub fn activity(&self) -> Value {
        let mut a = json!({
            "details": self.details,
            "type": if self.audio { 2 } else { 3 },
        });
        if !self.state.is_empty() {
            a["state"] = json!(self.state);
        }
        if let Some(s) = self.start {
            a["timestamps"] = json!({"start": s});
            if let Some(e) = self.end {
                a["timestamps"]["end"] = json!(e);
            }
        }
        if !self.image.is_empty() {
            a["assets"] = json!({"large_image": self.image, "large_text": "Vivi"});
        }
        a
    }
}

/// Discord limits these fields to 128 characters (and needs at least 2).
pub fn clip(s: &str) -> String {
    let mut s: String = s.chars().take(128).collect();
    while s.chars().count() < 2 {
        s.push(' ');
    }
    s
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Candidate IPC paths, most likely first.
pub fn candidates() -> Vec<String> {
    let mut v = Vec::new();
    #[cfg(unix)]
    {
        let mut dirs: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .collect();
        dirs.push("/tmp".into());
        for d in dirs {
            let d = d.trim_end_matches('/');
            for sub in [
                "",
                "app/com.discordapp.Discord/",
                "snap.discord/",
                ".flatpak/dev.vencord.Vesktop/xdg-run/",
            ] {
                for i in 0..10 {
                    v.push(format!("{d}/{sub}discord-ipc-{i}"));
                }
            }
        }
    }
    #[cfg(windows)]
    for i in 0..10 {
        v.push(format!(r"\\.\pipe\discord-ipc-{i}"));
    }
    v
}

pub struct Client {
    s: Box<dyn Stream>,
    nonce: u64,
}

fn write_frame(s: &mut dyn Stream, op: u32, v: &Value) -> io::Result<()> {
    let b = v.to_string().into_bytes();
    let mut f = Vec::with_capacity(8 + b.len());
    f.extend_from_slice(&op.to_le_bytes());
    f.extend_from_slice(&(b.len() as u32).to_le_bytes());
    f.extend_from_slice(&b);
    s.write_all(&f)?;
    s.flush()
}

fn read_frame(s: &mut dyn Stream) -> io::Result<(u32, Value)> {
    let mut h = [0u8; 8];
    s.read_exact(&mut h)?;
    let op = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
    let n = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
    if n > 1 << 20 {
        return Err(io::Error::other("frame too large"));
    }
    let mut b = vec![0; n];
    s.read_exact(&mut b)?;
    Ok((op, serde_json::from_slice(&b).unwrap_or(Value::Null)))
}

impl Client {
    /// Connect and handshake with the first reachable Discord.
    pub fn connect(app_id: &str) -> io::Result<Self> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "Discord is not running");
        for p in candidates() {
            match pipe::connect(&p).and_then(|s| Self::handshake(s, app_id)) {
                Ok(c) => return Ok(c),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => last = e,
            }
        }
        Err(last)
    }
    pub fn handshake(mut s: Box<dyn Stream>, app_id: &str) -> io::Result<Self> {
        write_frame(&mut *s, 0, &json!({"v": 1, "client_id": app_id}))?;
        let (op, v) = read_frame(&mut *s)?;
        if op == 2 || v["evt"] != "READY" {
            return Err(io::Error::other(format!(
                "Discord refused: {}",
                v["message"].as_str().unwrap_or("bad application ID?")
            )));
        }
        Ok(Self { s, nonce: 0 })
    }
    pub fn set(&mut self, p: Option<&Presence>) -> io::Result<()> {
        self.nonce += 1;
        let f = json!({
            "cmd": "SET_ACTIVITY",
            "args": {"pid": std::process::id(), "activity": p.map(Presence::activity)},
            "nonce": format!("vivi-{}-{}", std::process::id(), self.nonce),
        });
        write_frame(&mut *self.s, 1, &f)?;
        let (op, v) = read_frame(&mut *self.s)?;
        if op == 2 {
            return Err(io::Error::other("Discord closed the connection"));
        }
        if v["evt"] == "ERROR" {
            return Err(io::Error::other(
                v["data"]["message"].as_str().unwrap_or("error").to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn presence_fields() {
        let p = Presence::new(
            "Song", "Band", true, false, 10.0, 100.0, 1_000_000, false, "",
        );
        assert_eq!(p.start, Some(990_000));
        assert_eq!(p.end, Some(1_090_000));
        let v = p.activity();
        assert_eq!(v["details"], "Song");
        assert_eq!(v["state"], "Band");
        assert_eq!(v["type"], 2);
        let q = Presence::new(
            "Song", "Band", true, false, 11.0, 100.0, 1_001_500, false, "",
        );
        assert!(p.same(&q));
        let p = Presence::new("secret.mkv", "", false, true, 5.0, 0.0, 0, true, "logo");
        assert_eq!(p.details, "Watching a video");
        assert_eq!(p.state, "Paused");
        let v = p.activity();
        assert_eq!(v["type"], 3);
        assert_eq!(v["assets"]["large_image"], "logo");
        assert!(v.get("timestamps").is_none());
        assert_eq!(clip("x"), "x ");
    }

    /// Handshake and SET_ACTIVITY against a fake Discord on a socketpair.
    #[test]
    fn talks_ipc() {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let srv = std::thread::spawn(move || {
            let mut b: Box<dyn Stream> = Box::new(b);
            let (op, hello) = read_frame(&mut *b).unwrap();
            assert_eq!((op, hello["client_id"].as_str()), (0, Some("1234")));
            write_frame(
                &mut *b,
                1,
                &json!({"cmd":"DISPATCH","evt":"READY","data":{}}),
            )
            .unwrap();
            let (op, set) = read_frame(&mut *b).unwrap();
            assert_eq!(op, 1);
            write_frame(&mut *b, 1, &json!({"cmd":"SET_ACTIVITY","evt":null})).unwrap();
            let (_, clear) = read_frame(&mut *b).unwrap();
            write_frame(&mut *b, 1, &json!({"cmd":"SET_ACTIVITY"})).unwrap();
            (set, clear)
        });
        let mut c = Client::handshake(Box::new(a), "1234").unwrap();
        let p = Presence::new("Song", "Band", true, false, 0.0, 60.0, 5_000, false, "");
        c.set(Some(&p)).unwrap();
        c.set(None).unwrap();
        let (set, clear) = srv.join().unwrap();
        assert_eq!(set["cmd"], "SET_ACTIVITY");
        assert_eq!(set["args"]["activity"]["details"], "Song");
        assert_eq!(set["args"]["activity"]["timestamps"]["start"], 5000);
        assert!(clear["args"]["activity"].is_null());
    }
}
