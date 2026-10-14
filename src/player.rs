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
    EndFile,
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
        ev.observe_property("time-pos", Format::Double, 1)?;
        ev.observe_property("duration", Format::Double, 2)?;
        ev.observe_property("pause", Format::Flag, 3)?;
        ev.observe_property("volume", Format::Double, 4)?;
        ev.observe_property("mute", Format::Flag, 5)?;
        ev.observe_property("media-title", Format::String, 6)?;
        ev.observe_property("track-list/count", Format::Int64, 7)?;
        for (i, p) in [
            "aid",
            "sid",
            "chapter-list/count",
            "audio-device",
            "audio-device-list",
            "playlist-count",
            "playlist-pos",
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
                        Some(Ok(Event::EndFile(_))) => Some(PlayerEvent::EndFile),
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

#[cfg(test)]
mod tests {
    #[test]
    fn quoting() {
        assert_eq!(super::quote(r#"/a b/"x"\y"#), r#""/a b/\"x\"\\y""#);
    }
}
