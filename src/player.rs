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
    EndFile,
}

pub struct Player {
    pub mpv: Arc<Mpv>,
}

impl Player {
    /// Creates mpv with `vo=libmpv` (we render through the render API) and starts the
    /// event thread. `wakeup` is called after each event is queued (e.g. to poke the UI).
    pub fn new(wakeup: impl Fn() + Send + 'static) -> Result<(Self, Receiver<PlayerEvent>), libmpv2::Error> {
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
                        Some(Ok(Event::PropertyChange { name, change, .. })) => match (name, change) {
                            ("time-pos", PropertyData::Double(v)) => Some(PlayerEvent::TimePos(v)),
                            ("duration", PropertyData::Double(v)) => Some(PlayerEvent::Duration(v)),
                            ("pause", PropertyData::Flag(v)) => Some(PlayerEvent::Pause(v)),
                            ("volume", PropertyData::Double(v)) => Some(PlayerEvent::Volume(v)),
                            ("mute", PropertyData::Flag(v)) => Some(PlayerEvent::Mute(v)),
                            ("media-title", PropertyData::Str(v)) => Some(PlayerEvent::Title(v.to_owned())),
                            ("track-list/count", PropertyData::Int64(v)) => Some(PlayerEvent::TrackCount(v)),
                            _ => None,
                        },
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

    pub fn open(&self, path: &str) {
        let _ = self.mpv.command("loadfile", &[&format!("\"{}\"", path.replace('"', "\\\""))]);
        let _ = self.mpv.set_property("pause", false);
    }
    pub fn toggle_pause(&self) {
        let _ = self.mpv.command("cycle", &["pause"]);
    }
    pub fn seek_abs(&self, secs: f64) {
        let _ = self.mpv.command("seek", &[&format!("{secs:.3}"), "absolute"]);
    }
    pub fn seek_rel(&self, secs: f64) {
        let _ = self.mpv.command("seek", &[&format!("{secs:.3}"), "relative"]);
    }
    pub fn set_volume(&self, v: f64) {
        let _ = self.mpv.set_property("volume", v.clamp(0.0, 150.0));
    }
    pub fn add_volume(&self, d: f64) {
        let _ = self.mpv.command("add", &["volume", &format!("{d}")]);
    }
    pub fn toggle_mute(&self) {
        let _ = self.mpv.command("cycle", &["mute"]);
    }
    pub fn playlist_step(&self, next: bool) {
        let _ = self.mpv.command(if next { "playlist-next" } else { "playlist-prev" }, &[]);
    }
}
