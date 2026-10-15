//! Shared code of the plugin helpers: a small client for mpv's JSON IPC
//! (`input-ipc-server`, a Unix socket or a Windows named pipe) and argument parsing.
//!
//! The plugin's Lua script makes sure `input-ipc-server` is set, spawns the helper with
//! `--ipc <path>` and kills it when mpv quits or the plugin is turned off. The helper
//! also exits on its own once the IPC connection closes.

pub mod discord;
pub mod remote;

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};

pub mod pipe {
    use std::io::{self, Read, Write};

    /// A bidirectional local stream: Unix socket or Windows named pipe.
    pub trait Stream: Read + Write + Send {}
    impl<T: Read + Write + Send> Stream for T {}

    /// Connect to a socket path, or `\\.\pipe\name` on Windows.
    pub fn connect(path: &str) -> io::Result<Box<dyn Stream>> {
        #[cfg(unix)]
        {
            Ok(Box::new(std::os::unix::net::UnixStream::connect(path)?))
        }
        #[cfg(windows)]
        {
            Ok(Box::new(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)?,
            ))
        }
    }
}

/// `--key value` pairs (and `--flag` = "yes").
pub fn args() -> HashMap<String, String> {
    parse_args(std::env::args().skip(1))
}

pub fn parse_args(it: impl IntoIterator<Item = String>) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut it = it.into_iter().peekable();
    while let Some(a) = it.next() {
        if let Some(k) = a.strip_prefix("--") {
            let v = match it.peek() {
                Some(n) if !n.starts_with("--") => it.next().unwrap_or_default(),
                _ => "yes".into(),
            };
            m.insert(k.to_string(), v);
        }
    }
    m
}

/// Request/response mpv IPC client. Events are disabled on this connection, so every
/// line read is a reply; the helpers poll properties instead of observing them.
pub struct Mpv {
    r: BufReader<Box<dyn pipe::Stream>>,
    next: u64,
}

impl Mpv {
    pub fn connect(path: &str) -> io::Result<Self> {
        let s = pipe::connect(path)?;
        Self::from_stream(s)
    }
    pub fn from_stream(s: Box<dyn pipe::Stream>) -> io::Result<Self> {
        let mut m = Self {
            r: BufReader::new(s),
            next: 1,
        };
        m.command(&[json!("disable_event"), json!("all")])?;
        Ok(m)
    }
    /// Run a command; `Ok(data)` on success, `Err` on IPC failure (mpv gone) or an mpv
    /// error (kind `Other`).
    pub fn command(&mut self, args: &[Value]) -> io::Result<Value> {
        let id = self.next;
        self.next += 1;
        let mut line = json!({"command": args, "request_id": id}).to_string();
        line.push('\n');
        self.r.get_mut().write_all(line.as_bytes())?;
        self.r.get_mut().flush()?;
        loop {
            let mut l = String::new();
            if self.r.read_line(&mut l)? == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "mpv closed"));
            }
            let Ok(v) = serde_json::from_str::<Value>(&l) else {
                continue;
            };
            if v.get("request_id").and_then(Value::as_u64) != Some(id) {
                continue; // an event that slipped through
            }
            return if v["error"] == "success" {
                Ok(v.get("data").cloned().unwrap_or(Value::Null))
            } else {
                Err(io::Error::other(
                    v["error"].as_str().unwrap_or("error").to_string(),
                ))
            };
        }
    }
    pub fn cmd(&mut self, args: &[&str]) -> io::Result<Value> {
        let v: Vec<Value> = args.iter().map(|a| json!(a)).collect();
        self.command(&v)
    }
    /// Property value, `Ok(None)` when unavailable; `Err` only when mpv is gone.
    pub fn get(&mut self, prop: &str) -> io::Result<Option<Value>> {
        match self.cmd(&["get_property", prop]) {
            Ok(v) if v.is_null() => Ok(None),
            Ok(v) => Ok(Some(v)),
            Err(e) if e.kind() == io::ErrorKind::Other => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn get_str(&mut self, prop: &str) -> io::Result<String> {
        Ok(match self.get(prop)? {
            Some(Value::String(s)) => s,
            Some(v) => v.to_string(),
            None => String::new(),
        })
    }
    pub fn get_f64(&mut self, prop: &str) -> io::Result<f64> {
        Ok(self.get(prop)?.and_then(|v| v.as_f64()).unwrap_or(0.0))
    }
    pub fn get_bool(&mut self, prop: &str) -> io::Result<bool> {
        Ok(self.get(prop)?.and_then(|v| v.as_bool()).unwrap_or(false))
    }
    /// Vivi's toast (`script-message vivi-toast`).
    pub fn toast(&mut self, text: &str) -> io::Result<()> {
        self.cmd(&["script-message", "vivi-toast", text])
            .map(|_| ())
    }
    /// Publish a status string the Lua script can read (`user-data/<key>`).
    pub fn set_status(&mut self, key: &str, text: &str) -> io::Result<()> {
        match self.command(&[
            json!("set_property"),
            json!(format!("user-data/{key}")),
            json!(text),
        ]) {
            Err(e) if e.kind() != io::ErrorKind::Other => Err(e),
            _ => Ok(()),
        }
    }
}

/// Fake mpv for tests: answers every request with `reply(command)` on a socketpair.
#[cfg(all(test, unix))]
pub fn fake_mpv(
    reply: impl Fn(&Value) -> Value + Send + 'static,
) -> (Mpv, std::thread::JoinHandle<Vec<Value>>) {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let h = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut r = BufReader::new(b.try_clone().unwrap());
        let mut w = b;
        let mut l = String::new();
        while r.read_line(&mut l).unwrap_or(0) > 0 {
            let v: Value = serde_json::from_str(&l).unwrap();
            let data = reply(&v["command"]);
            let out = json!({"request_id": v["request_id"], "error": if data.is_null() {"property unavailable"} else {"success"}, "data": data});
            // An unrelated event first, to check it is skipped.
            let _ = w.write_all(b"{\"event\":\"idle\"}\n");
            let _ = w.write_all(format!("{out}\n").as_bytes());
            seen.push(v["command"].clone());
            l.clear();
        }
        seen
    });
    (Mpv::from_stream(Box::new(a)).unwrap(), h)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn args_parse() {
        let a = parse_args(["--ipc", "/x", "--hide", "--n", "3"].map(String::from));
        assert_eq!(a["ipc"], "/x");
        assert_eq!(a["hide"], "yes");
        assert_eq!(a["n"], "3");
    }

    #[test]
    fn mpv_requests() {
        let (mut m, h) = fake_mpv(|c| match c[1].as_str() {
            Some("pause") => json!(true),
            Some("volume") => json!(55.0),
            Some("media-title") => json!("Song"),
            _ if c[0] == "disable_event" || c[0] == "script-message" => json!("ok"),
            _ => Value::Null,
        });
        assert!(m.get_bool("pause").unwrap());
        assert_eq!(m.get_f64("volume").unwrap(), 55.0);
        assert_eq!(m.get_str("media-title").unwrap(), "Song");
        assert_eq!(m.get("nope").unwrap(), None);
        m.toast("hi").unwrap();
        drop(m);
        let seen = h.join().unwrap();
        assert_eq!(seen[0], json!(["disable_event", "all"]));
        assert_eq!(
            seen.last().unwrap(),
            &json!(["script-message", "vivi-toast", "hi"])
        );
    }
}
