//! Discord Rich Presence (#57): shows what is playing in Discord through its local IPC
//! socket (`$XDG_RUNTIME_DIR/discord-ipc-0` on Linux, also Flatpak/Snap paths; a named
//! pipe on Windows) with the `discord-rich-presence` crate. Needs the id of a Discord
//! application (created at <https://discord.com/developers/applications>); its name is
//! what Discord shows ("Listening to <name>").
//!
//! All IPC runs on a worker thread: the UI thread only sends [`Presence`] values when
//! the title, pause state or position (seek) changes. When Discord is not running the
//! worker retries every 15 s.

use super::{Ctx, Manifest, Plugin, SettingSpec};
use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const ID: &str = "discord";

/// What to show; `None` in the channel clears the activity.
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
    /// Build from the player state. `hide` replaces file and stream names.
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
            (
                if audio {
                    "Listening to music"
                } else {
                    "Watching a video"
                }
                .to_string(),
                String::new(),
            )
        } else {
            (title.to_string(), artist.to_string())
        };
        let state = if paused {
            if state.is_empty() {
                "Paused".into()
            } else {
                format!("{state} · Paused")
            }
        } else {
            state
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
    pub fn activity(&self) -> Activity<'_> {
        let mut a = Activity::new()
            .details(self.details.as_str())
            .activity_type(if self.audio {
                ActivityType::Listening
            } else {
                ActivityType::Watching
            });
        if !self.state.is_empty() {
            a = a.state(self.state.as_str());
        }
        if let Some(s) = self.start {
            let mut t = Timestamps::new().start(s);
            if let Some(e) = self.end {
                t = t.end(e);
            }
            a = a.timestamps(t);
        }
        if !self.image.is_empty() {
            a = a.assets(
                Assets::new()
                    .large_image(self.image.as_str())
                    .large_text("Vivi"),
            );
        }
        a
    }
}

/// Discord limits these fields to 128 characters (and needs at least 2).
fn clip(s: &str) -> String {
    let mut s: String = s.chars().take(128).collect();
    while s.chars().count() < 2 {
        s.push(' ');
    }
    s
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

enum Cmd {
    Set(Option<Presence>),
    Quit,
}

/// Status messages back from the worker.
fn worker(app_id: String, rx: Receiver<Cmd>, status: Sender<String>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut want: Option<Presence> = None;
    let mut dirty = false;
    let mut next_try = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Cmd::Set(p)) => {
                want = p;
                dirty = true;
            }
            Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if !dirty {
            continue;
        }
        if client.is_none() {
            if Instant::now() < next_try {
                continue;
            }
            let mut c = DiscordIpcClient::new(&app_id);
            match c.connect() {
                Ok(()) => {
                    let _ = status.send("Connected to Discord".into());
                    client = Some(c);
                }
                Err(e) => {
                    let _ = status.send(format!("Discord not reachable: {e}"));
                    next_try = Instant::now() + Duration::from_secs(15);
                    continue;
                }
            }
        }
        let Some(c) = client.as_mut() else { continue };
        let r = match &want {
            Some(p) => c.set_activity(p.activity()),
            None => c.clear_activity(),
        };
        match r {
            Ok(()) => dirty = false,
            Err(e) => {
                let _ = status.send(format!("Discord connection lost: {e}"));
                let _ = c.close();
                client = None;
                next_try = Instant::now() + Duration::from_secs(5);
            }
        }
    }
    if let Some(mut c) = client {
        let _ = c.clear_activity();
        let _ = c.close();
    }
}

#[derive(Default)]
pub struct Discord {
    tx: Option<Sender<Cmd>>,
    status_rx: Option<Receiver<String>>,
    last: Option<Presence>,
    status: String,
    poll: u32,
}

impl Discord {
    fn stop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Cmd::Quit);
        }
        self.status_rx = None;
        self.last = None;
    }
    fn start(&mut self, cx: &Ctx) {
        self.stop();
        let id = cx.get("app-id").trim().to_string();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            self.status = "Enter your Discord application ID in Settings → Extensions → Discord Rich Presence".into();
            cx.toast(&self.status);
            return;
        }
        let (tx, rx) = channel();
        let (stx, srx) = channel();
        std::thread::spawn(move || worker(id, rx, stx));
        self.tx = Some(tx);
        self.status_rx = Some(srx);
        self.status = "Connecting to Discord…".into();
    }
    fn current(cx: &Ctx) -> Option<Presence> {
        let p = cx.player;
        let path = p.get("path")?;
        if path.is_empty() {
            return None;
        }
        let f = |k: &str| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        let audio = p.get("current-tracks/video/id").is_none()
            || p.get("current-tracks/video/albumart").as_deref() == Some("yes")
            || p.get("current-tracks/video/image").as_deref() == Some("yes");
        let title = p
            .get("metadata/by-key/title")
            .or_else(|| p.get("media-title"))
            .unwrap_or_default();
        let artist = p
            .get("metadata/by-key/artist")
            .or_else(|| p.get("metadata/by-key/icy-name"))
            .unwrap_or_default();
        Some(Presence::new(
            &title,
            &artist,
            audio,
            p.get("pause").as_deref() == Some("yes"),
            f("time-pos"),
            f("duration"),
            now_ms(),
            cx.flag("hide-names"),
            &cx.get("image"),
        ))
    }
    fn update(&mut self, cx: &Ctx, force: bool) {
        let Some(tx) = &self.tx else { return };
        let cur = Self::current(cx);
        // Same content, and start time within 2 s (no seek): nothing to send.
        let same = match (&cur, &self.last) {
            (Some(a), Some(b)) => {
                a.details == b.details
                    && a.state == b.state
                    && a.paused == b.paused
                    && match (a.start, b.start) {
                        (Some(x), Some(y)) => (x - y).abs() < 2000,
                        (None, None) => true,
                        _ => false,
                    }
            }
            (None, None) => true,
            _ => false,
        };
        if same && !force {
            return;
        }
        let _ = tx.send(Cmd::Set(cur.clone()));
        self.last = cur;
    }
}

impl Plugin for Discord {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Discord Rich Presence",
            "Shows the current title, artist and progress in your Discord status. Needs a Discord application ID; Discord must be running on this computer.",
            "discord",
        )
        .setting(SettingSpec::text(
            "app-id",
            "Discord application ID",
            "Create an application at discord.com/developers/applications and paste its Application ID. Its name is shown as \"Listening to <name>\".",
            "123456789012345678",
            "",
        ))
        .setting(SettingSpec::toggle(
            "hide-names",
            "Hide titles and file names",
            "Only show \"Watching a video\" / \"Listening to music\" and the progress.",
            false,
        ))
        .setting(SettingSpec::text(
            "image",
            "Large image key",
            "Optional: name of an art asset uploaded to your Discord application.",
            "logo",
            "",
        ))
        .setting(SettingSpec::action(
            "status",
            "Connection status",
            "Shows whether Vivi is connected to Discord.",
            "Show",
        ))
        .default_off()
    }
    fn enable(&mut self, cx: &Ctx) {
        self.start(cx);
        self.update(cx, true);
    }
    fn disable(&mut self, _cx: &Ctx) {
        self.stop();
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.update(cx, true);
    }
    fn setting_changed(&mut self, cx: &Ctx, key: &str) {
        if key == "app-id" {
            self.start(cx);
        }
        self.update(cx, true);
    }
    fn command(&mut self, cx: &Ctx, name: &str, _args: &[String]) {
        if name == "status" {
            cx.toast(&self.status);
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        if let Some(rx) = &self.status_rx {
            while let Ok(s) = rx.try_recv() {
                eprintln!("vivi: discord: {s}");
                self.status = s;
            }
        }
        // Every 0.5 s: pause, seek and title changes.
        self.poll += 1;
        if self.poll.is_multiple_of(5) {
            self.update(cx, false);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn presence_fields() {
        let p = Presence::new(
            "Song", "Band", true, false, 10.0, 100.0, 1_000_000, false, "",
        );
        assert_eq!(p.start, Some(990_000));
        assert_eq!(p.end, Some(1_090_000));
        let v = serde_json::to_value(p.activity()).unwrap();
        assert_eq!(v["details"], "Song");
        assert_eq!(v["state"], "Band");
        assert_eq!(v["type"], 2);
        let p = Presence::new("secret.mkv", "", false, true, 5.0, 0.0, 0, true, "logo");
        assert_eq!(p.details, "Watching a video");
        assert_eq!(p.state, "Paused");
        assert_eq!(p.start, None);
        let v = serde_json::to_value(p.activity()).unwrap();
        assert_eq!(v["type"], 3);
        assert_eq!(v["assets"]["large_image"], "logo");
        assert_eq!(clip("x"), "x ");
    }

    fn frame(s: &mut std::os::unix::net::UnixStream) -> (u32, serde_json::Value) {
        let mut h = [0u8; 8];
        s.read_exact(&mut h).unwrap();
        let op = u32::from_le_bytes(h[..4].try_into().unwrap());
        let n = u32::from_le_bytes(h[4..].try_into().unwrap()) as usize;
        let mut b = vec![0; n];
        s.read_exact(&mut b).unwrap();
        (op, serde_json::from_slice(&b).unwrap())
    }

    /// The worker against a fake Discord IPC server on `$XDG_RUNTIME_DIR/discord-ipc-0`.
    #[test]
    fn worker_talks_ipc() {
        let dir = std::env::temp_dir().join(format!("vivi-discord-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        let l = std::os::unix::net::UnixListener::bind(dir.join("discord-ipc-0")).unwrap();
        let (tx, rx) = channel();
        let (stx, srx) = channel();
        let w = std::thread::spawn(move || worker("1234".into(), rx, stx));
        let p = Presence::new("Song", "Band", true, false, 0.0, 60.0, 5_000, false, "");
        tx.send(Cmd::Set(Some(p))).unwrap();
        let (mut s, _) = l.accept().unwrap();
        let (op, hello) = frame(&mut s);
        assert_eq!(op, 0);
        assert_eq!(hello["client_id"], "1234");
        let ready = br#"{"cmd":"DISPATCH","evt":"READY","data":{}}"#;
        s.write_all(&1u32.to_le_bytes()).unwrap();
        s.write_all(&(ready.len() as u32).to_le_bytes()).unwrap();
        s.write_all(ready).unwrap();
        let (op, set) = frame(&mut s);
        assert_eq!(op, 1);
        assert_eq!(set["cmd"], "SET_ACTIVITY");
        assert_eq!(set["args"]["activity"]["details"], "Song");
        assert_eq!(set["args"]["activity"]["timestamps"]["start"], 5000);
        assert_eq!(
            srx.recv_timeout(Duration::from_secs(5)).unwrap(),
            "Connected to Discord"
        );
        tx.send(Cmd::Quit).unwrap();
        let (_, clear) = frame(&mut s);
        assert!(clear["args"]["activity"].is_null());
        w.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
