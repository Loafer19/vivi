//! Thin wrapper over libmpv: playback commands + an event thread that forwards
//! property changes through a channel.

use libmpv2::events::{Event, EventContext, PropertyData};
use libmpv2::{Format, Mpv};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    TimePos(f64),
    Duration(f64),
    Pause(bool),
    Volume(f64),
    Mute(bool),
    Title(String),
    TrackCount(i64),
    FileLoaded,
    /// A list-like property changed (tracks, selection, chapters, devices).
    ListsChanged,
    /// `ab-loop-a` / `ab-loop-b` changed (any source: keys, scripts, IPC).
    AbLoop,
    /// `loop-file`, `loop-playlist` or `user-data/vivi/shuffle` changed (see `loopsync`).
    LoopShuffle,
    /// The file ended; `true` when it failed to open or play.
    EndFile(bool),
    /// A new file starts loading.
    StartFile,
    /// Playback (re)started: first frame / audio out after load or seek.
    PlaybackRestart,
    /// `paused-for-cache` changed (network buffering).
    Buffering(bool),
    /// `script-message` broadcast (e.g. `vivi-toast <text>` from a plugin package).
    ClientMessage(Vec<String>),
    /// mpv warning/error log line: (prefix = module or script name, text).
    Log(String, String),
}

pub struct Player {
    pub mpv: Arc<Mpv>,
}

impl Player {
    /// Creates mpv with `vo=libmpv` (we render through the render API) and starts the
    /// event thread. `wakeup` is called after each event is queued (e.g. to poke the UI).
    pub fn new(
        wakeup: impl Fn() + Send + 'static,
    ) -> Result<(Self, Receiver<PlayerEvent>), libmpv2::Error> {
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("vo", "libmpv")?;
            init.set_property("hwdec", "auto-safe")?;
            // Fall back to a null sink so video still plays without an audio device.
            init.set_property("ao", "pipewire,pulse,alsa,null")?;
            init.set_property("audio-fallback-to-null", "yes")?;
            init.set_property("keep-open", "yes")?;
            init.set_property("idle", "yes")?;
            init.set_property("input-default-bindings", "no")?;
            init.set_property("volume-max", 150i64)?;
            // mpv's built-in OSD scripts (stats, console, select, positioning, commands)
            // are not reachable in vivi (no default bindings, its own UI); each is a Lua
            // state and a thread. ytdl_hook stays for web URLs.
            for o in [
                "load-stats-overlay",
                "load-console",
                "load-select",
                "load-positioning",
                "load-commands",
                "load-auto-profiles",
            ] {
                init.set_property(o, "no")?;
            }
            // Resume: mpv watch-later files in $XDG_STATE_HOME/vivi/watch_later.
            let wl = crate::store::watch_later_dir();
            let _ = std::fs::create_dir_all(&wl);
            init.set_property("watch-later-dir", wl.to_string_lossy().as_ref())?;
            init.set_property("resume-playback", "yes")?;
            init.set_property(
                "watch-later-options",
                "start,aid,sid,sub-delay,audio-delay,volume,speed",
            )?;
            if std::env::var_os("VIVI_MPV_DEBUG").is_some() {
                init.set_property("terminal", "yes")?;
                init.set_property("msg-level", "all=v")?;
            }
            Ok(())
        })?;
        let mpv = Arc::new(mpv);
        let (tx, rx) = channel();

        let mut ev = EventContext::new(mpv.ctx);
        ev.disable_deprecated_events()?;
        // Warnings and errors (script load/runtime failures) for the Extensions page.
        // SAFETY: valid handle owned by `mpv`; the level is a NUL-terminated string.
        unsafe {
            libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), c"warn".as_ptr());
        }
        ev.observe_property("time-pos", Format::Double, 1)?;
        ev.observe_property("duration", Format::Double, 2)?;
        ev.observe_property("pause", Format::Flag, 3)?;
        ev.observe_property("volume", Format::Double, 4)?;
        ev.observe_property("mute", Format::Flag, 5)?;
        ev.observe_property("media-title", Format::String, 6)?;
        ev.observe_property("track-list/count", Format::Int64, 7)?;
        ev.observe_property("paused-for-cache", Format::Flag, 8)?;
        for (i, p) in [
            "aid",
            "sid",
            "chapter-list/count",
            "audio-device",
            "audio-device-list",
            "playlist-count",
            "playlist-pos",
            "ab-loop-a",
            "ab-loop-b",
            "loop-file",
            "loop-playlist",
            crate::loopsync::SHUFFLE,
        ]
        .iter()
        .enumerate()
        {
            ev.observe_property(p, Format::String, 10 + i as u64)?;
        }

        let keep_alive = mpv.clone();
        thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || {
                let _mpv = keep_alive;
                loop {
                    let msg = match ev.wait_event(1.0) {
                        Some(Ok(Event::Shutdown)) => break,
                        Some(Ok(Event::FileLoaded)) => Some(PlayerEvent::FileLoaded),
                        Some(Ok(Event::EndFile(r))) => Some(PlayerEvent::EndFile(
                            r == libmpv2::mpv_end_file_reason::Error,
                        )),
                        Some(Ok(Event::StartFile)) => Some(PlayerEvent::StartFile),
                        Some(Ok(Event::PlaybackRestart)) => Some(PlayerEvent::PlaybackRestart),
                        Some(Ok(Event::ClientMessage(args))) => Some(PlayerEvent::ClientMessage(
                            args.iter().map(|a| a.to_string()).collect(),
                        )),
                        Some(Ok(Event::LogMessage { prefix, text, .. })) => Some(PlayerEvent::Log(
                            prefix.to_string(),
                            text.trim_end().to_string(),
                        )),
                        Some(Ok(Event::PropertyChange { name, change, .. })) => {
                            match (name, change) {
                                ("time-pos", PropertyData::Double(v)) => {
                                    Some(PlayerEvent::TimePos(v))
                                }
                                ("duration", PropertyData::Double(v)) => {
                                    Some(PlayerEvent::Duration(v))
                                }
                                ("pause", PropertyData::Flag(v)) => Some(PlayerEvent::Pause(v)),
                                ("volume", PropertyData::Double(v)) => Some(PlayerEvent::Volume(v)),
                                ("mute", PropertyData::Flag(v)) => Some(PlayerEvent::Mute(v)),
                                ("paused-for-cache", PropertyData::Flag(v)) => {
                                    Some(PlayerEvent::Buffering(v))
                                }
                                ("media-title", PropertyData::Str(v)) => {
                                    Some(PlayerEvent::Title(v.to_owned()))
                                }
                                ("track-list/count", PropertyData::Int64(v)) => {
                                    Some(PlayerEvent::TrackCount(v))
                                }
                                (
                                    "aid" | "sid" | "chapter-list/count" | "audio-device"
                                    | "audio-device-list" | "playlist-count" | "playlist-pos",
                                    _,
                                ) => Some(PlayerEvent::ListsChanged),
                                ("ab-loop-a" | "ab-loop-b", _) => Some(PlayerEvent::AbLoop),
                                ("loop-file" | "loop-playlist" | crate::loopsync::SHUFFLE, _) => {
                                    Some(PlayerEvent::LoopShuffle)
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    if let Some(m) = msg {
                        if tx.send(m).is_err() {
                            break;
                        }
                        wakeup();
                    }
                }
            })
            .expect("spawn mpv event thread");

        Ok((Self { mpv }, rx))
    }

    /// Save the resume position of the current file (no-op when idle).
    /// Stop mpv before Vivi exits. `quit` sends every script its `shutdown` event, so a
    /// script can stop its own tools (an extension's curl / yt-dlp / ffmpeg download).
    /// Without it the process ended with mpv still running: scripts never heard of the quit
    /// and their tools kept running, orphaned. On Linux it waits (at most `max`) until Vivi
    /// has no child processes left, then stops the ones still running (TERM, then KILL):
    /// mpv would otherwise wait for such a tool before it lets the script go. Elsewhere a
    /// short fixed wait.
    pub fn shutdown(&self, max: std::time::Duration) {
        let _ = self.mpv.command("quit", &[]);
        let t0 = std::time::Instant::now();
        #[cfg(target_os = "linux")]
        {
            while t0.elapsed() < max && !children().is_empty() {
                thread::sleep(std::time::Duration::from_millis(50));
            }
            // A tool a script did not stop (a download that would run for hours) also keeps
            // mpv from finishing its shutdown: stop it and everything it started.
            for (sig, wait) in [(libc::SIGTERM, 1500), (libc::SIGKILL, 300)] {
                let left = descendants();
                if left.is_empty() {
                    break;
                }
                for p in left {
                    // SAFETY: kill(2) on our own (direct or indirect) child processes.
                    unsafe { libc::kill(p, sig) };
                }
                let t1 = std::time::Instant::now();
                while t1.elapsed() < std::time::Duration::from_millis(wait)
                    && !children().is_empty()
                {
                    thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        thread::sleep(
            max.min(std::time::Duration::from_millis(300))
                .saturating_sub(t0.elapsed()),
        );
    }
    pub fn save_position(&self) {
        if self.get("path").is_some() {
            let _ = self.mpv.command("write-watch-later-config", &[]);
        }
    }
    pub fn open(&self, path: &str) {
        self.save_position();
        let _ = self.mpv.command("loadfile", &[&quote(path)]);
        let _ = self.mpv.set_property("pause", false);
    }
    pub fn toggle_pause(&self) {
        let _ = self.mpv.command("cycle", &["pause"]);
    }
    pub fn seek_abs(&self, secs: f64) {
        let _ = self
            .mpv
            .command("seek", &[&format!("{secs:.3}"), "absolute"]);
    }
    pub fn seek_rel(&self, secs: f64) {
        let _ = self
            .mpv
            .command("seek", &[&format!("{secs:.3}"), "relative"]);
    }
    pub fn set_volume(&self, v: f64) {
        let _ = self.mpv.set_property("volume", v.clamp(0.0, 1000.0));
    }
    pub fn add_volume(&self, d: f64) {
        let _ = self.mpv.command("add", &["volume", &format!("{d}")]);
    }
    pub fn toggle_mute(&self) {
        let _ = self.mpv.command("cycle", &["mute"]);
    }
    pub fn playlist_step(&self, next: bool) {
        let _ = self.mpv.command(
            if next {
                "playlist-next"
            } else {
                "playlist-prev"
            },
            &[],
        );
    }

    /// Set any mpv property from its string form (e.g. `("sub-delay", "0.5")`).
    pub fn set(&self, name: &str, value: &str) {
        if let Err(e) = self.mpv.set_property(name, value) {
            eprintln!("vivi: set {name}={value}: {e:?}");
        }
    }
    pub fn get(&self, name: &str) -> Option<String> {
        self.mpv.get_property::<String>(name).ok()
    }
    pub fn run(&self, cmd: &str) {
        if let Err(e) = self.mpv.command(cmd, &[]) {
            eprintln!("vivi: command {cmd}: {e:?}");
        }
    }
    /// Run an mpv command with arguments (any characters; double-quoted with C escapes,
    /// since `mpv_command_string` does not understand `%len%` quoting).
    pub fn cmd(&self, name: &str, args: &[&str]) {
        let q: Vec<String> = args.iter().map(|a| quote(a)).collect();
        let q: Vec<&str> = q.iter().map(String::as_str).collect();
        if let Err(e) = self.mpv.command(name, &q) {
            eprintln!("vivi: command {name}: {e:?}");
        }
    }
    /// Like [`Player::cmd`], but returns mpv's error instead of logging it.
    pub fn try_cmd(&self, name: &str, args: &[&str]) -> Result<(), String> {
        let q: Vec<String> = args.iter().map(|a| quote(a)).collect();
        let q: Vec<&str> = q.iter().map(String::as_str).collect();
        self.mpv.command(name, &q).map_err(|e| format!("{e:?}"))
    }
    /// Tracks of a type ("audio" | "sub" | "video"): (id, label, selected).
    pub fn tracks(&self, kind: &str) -> Vec<(i64, String, bool)> {
        let n: i64 = self.mpv.get_property("track-list/count").unwrap_or(0);
        (0..n)
            .filter_map(|i| {
                let g = |f: &str| self.get(&format!("track-list/{i}/{f}"));
                if g("type")? != kind {
                    return None;
                }
                let id: i64 = self.mpv.get_property(&format!("track-list/{i}/id")).ok()?;
                let mut label = g("title").unwrap_or_default();
                if let Some(lang) = g("lang") {
                    label = if label.is_empty() {
                        lang
                    } else {
                        format!("{label} ({lang})")
                    };
                }
                if label.is_empty() {
                    label = format!("Track {id}");
                }
                if g("external").as_deref() == Some("yes") {
                    label.push_str(" · external");
                }
                Some((id, label, g("selected").as_deref() == Some("yes")))
            })
            .collect()
    }
    /// Audio output devices: (name, description).
    pub fn audio_devices(&self) -> Vec<(String, String)> {
        let n: i64 = self
            .mpv
            .get_property("audio-device-list/count")
            .unwrap_or(0);
        (0..n)
            .filter_map(|i| {
                let name = self.get(&format!("audio-device-list/{i}/name"))?;
                let desc = self
                    .get(&format!("audio-device-list/{i}/description"))
                    .unwrap_or_else(|| name.clone());
                Some((name, desc))
            })
            .collect()
    }
    /// Chapters: (title, start seconds).
    pub fn chapters(&self) -> Vec<(String, f64)> {
        let n: i64 = self.mpv.get_property("chapter-list/count").unwrap_or(0);
        (0..n)
            .map(|i| {
                let t: f64 = self
                    .mpv
                    .get_property(&format!("chapter-list/{i}/time"))
                    .unwrap_or(0.0);
                let title = self
                    .get(&format!("chapter-list/{i}/title"))
                    .unwrap_or_else(|| format!("Chapter {}", i + 1));
                (title, t)
            })
            .collect()
    }

    /// Append to the queue (mpv playlist); starts playing if idle.
    pub fn enqueue(&self, path: &str) {
        let _ = self.mpv.command("loadfile", &[&quote(path), "append-play"]);
    }
    /// Queue entries: (filename, is_current).
    pub fn queue(&self) -> Vec<(String, bool)> {
        let n: i64 = self.mpv.get_property("playlist/count").unwrap_or(0);
        (0..n)
            .filter_map(|i| {
                let f = self.get(&format!("playlist/{i}/filename"))?;
                let cur = self.get(&format!("playlist/{i}/current")).as_deref() == Some("yes");
                Some((f, cur))
            })
            .collect()
    }
    pub fn play_index(&self, i: i32) {
        self.save_position();
        let _ = self.mpv.command("playlist-play-index", &[&i.to_string()]);
        let _ = self.mpv.set_property("pause", false);
    }
    pub fn remove_index(&self, i: i32) {
        let _ = self.mpv.command("playlist-remove", &[&i.to_string()]);
    }
    /// Move entry `from` one step up (-1) or down (+1).
    pub fn move_index(&self, from: i32, delta: i32) {
        let n: i64 = self.mpv.get_property("playlist/count").unwrap_or(0);
        let to = from + delta;
        if to < 0 || to as i64 >= n {
            return;
        }
        // mpv inserts before `to`; moving down needs +1.
        let target = if delta > 0 { to + 1 } else { to };
        let _ = self
            .mpv
            .command("playlist-move", &[&from.to_string(), &target.to_string()]);
    }
    /// Sort the queue by file name (A→Z) with `playlist-move`, keeping the current entry playing.
    pub fn sort_queue(&self) {
        let mut names: Vec<String> = self
            .queue()
            .into_iter()
            .map(|(f, _)| crate::store::display_name(&f).to_lowercase())
            .collect();
        for (i, j) in sort_moves(&names) {
            let _ = self
                .mpv
                .command("playlist-move", &[&j.to_string(), &i.to_string()]);
            let x = names.remove(j);
            names.insert(i, x);
        }
    }
    pub fn clear_queue(&self) {
        let _ = self.mpv.command("playlist-clear", &[]);
    }
}

/// Quote one argument for mpv's command parser (C-style escapes in double quotes).
fn quote(path: &str) -> String {
    format!(
        "\"{}\"",
        path.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

/// Selection-sort moves (from index j to position i) that order `v` ascending.
fn sort_moves(v: &[String]) -> Vec<(usize, usize)> {
    let mut v = v.to_vec();
    let mut out = Vec::new();
    for i in 0..v.len() {
        let j = (i..v.len()).min_by(|a, b| v[*a].cmp(&v[*b])).unwrap_or(i);
        if j != i {
            let x = v.remove(j);
            v.insert(i, x);
            out.push((i, j));
        }
    }
    out
}

/// Whether this process has a child process (Linux: `/proc/self/task/*/children`).
#[cfg(target_os = "linux")]
fn children_of(path: &std::path::Path) -> Vec<i32> {
    std::fs::read_dir(path.join("task"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|t| std::fs::read_to_string(t.path().join("children")).ok())
        .flat_map(|c| {
            c.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect::<Vec<i32>>()
        })
        .collect()
}

/// This process's direct children.
#[cfg(target_os = "linux")]
fn children() -> Vec<i32> {
    children_of(std::path::Path::new("/proc/self"))
}

/// This process's children and their children, deepest first.
#[cfg(target_os = "linux")]
fn descendants() -> Vec<i32> {
    let mut out = Vec::new();
    let mut todo = children();
    while let Some(p) = todo.pop() {
        if out.contains(&p) {
            continue;
        }
        todo.extend(children_of(std::path::Path::new(&format!("/proc/{p}"))));
        out.push(p);
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn sort_moves() {
        let v: Vec<String> = ["c", "a", "b"].iter().map(|s| s.to_string()).collect();
        let mut w = v.clone();
        for (i, j) in super::sort_moves(&v) {
            let x = w.remove(j);
            w.insert(i, x);
        }
        assert_eq!(w, ["a", "b", "c"]);
    }

    #[test]
    fn quoting() {
        assert_eq!(super::quote(r#"/a b/"x"\y"#), r#""/a b/\"x\"\\y""#);
    }
}
