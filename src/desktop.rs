//! Desktop integration glue: MPRIS2 on Linux (media keys, playerctl, widgets).

use crate::player::Player;
use crate::AppWindow;
use std::rc::Rc;

/// Drain pending MPRIS commands and publish the current player state.
/// No-op when MPRIS is not registered (no session bus, or not Linux).
pub fn poke() {
    #[cfg(target_os = "linux")]
    linux::poke();
}

#[cfg(target_os = "linux")]
pub fn setup(app: &AppWindow, player: &Rc<Player>, log: bool) {
    linux::setup(app, player, log);
}

#[cfg(not(target_os = "linux"))]
pub fn setup(_app: &AppWindow, _player: &Rc<Player>, _log: bool) {}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::mpris::{Cmd, Mpris, State};
    use slint::ComponentHandle;
    use std::cell::{Cell, RefCell};
    use std::sync::mpsc::Receiver;

    struct Bridge {
        mpris: RefCell<Mpris>,
        rx: Receiver<Cmd>,
        last_pos: Cell<f64>,
        log: bool,
        app: slint::Weak<AppWindow>,
        player: Rc<Player>,
    }

    thread_local! {
        static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
    }

    pub fn setup(app: &AppWindow, player: &Rc<Player>, log: bool) {
        let Some((mpris, rx)) = Mpris::start() else {
            if log {
                eprintln!("mpris: no session bus");
            }
            return;
        };
        if log {
            eprintln!("mpris: registered");
        }
        BRIDGE.with(|b| {
            *b.borrow_mut() = Some(Bridge {
                mpris: RefCell::new(mpris),
                rx,
                last_pos: Cell::new(0.0),
                log,
                app: app.as_weak(),
                player: player.clone(),
            });
        });
        poke();
    }

    pub fn poke() {
        BRIDGE.with(|b| {
            let b = b.borrow();
            let Some(b) = b.as_ref() else {
                return;
            };
            let Some(a) = b.app.upgrade() else {
                return;
            };
            let p = &b.player;
            let mut seeked = None;
            while let Ok(cmd) = b.rx.try_recv() {
                if b.log {
                    eprintln!("mpris: {cmd:?}");
                }
                match cmd {
                    Cmd::PlayPause => p.toggle_pause(),
                    Cmd::Play => p.set("pause", "no"),
                    Cmd::Pause => p.set("pause", "yes"),
                    Cmd::Stop => {
                        p.set("pause", "yes");
                        p.seek_abs(0.0);
                        seeked = Some(0.0);
                    }
                    Cmd::Next => p.playlist_step(true),
                    Cmd::Previous => p.playlist_step(false),
                    Cmd::Seek(d) => {
                        let t = (a.global::<crate::Playback>().get_position() as f64 + d).max(0.0);
                        p.seek_abs(t);
                        seeked = Some(t);
                    }
                    Cmd::SetPosition(t) => {
                        p.seek_abs(t);
                        seeked = Some(t);
                    }
                    Cmd::Volume(v) => p.set_volume(v),
                    Cmd::Open(u) => p.open(u.strip_prefix("file://").unwrap_or(&u)),
                    Cmd::Raise => crate::tray::show(&a),
                    Cmd::Quit => {
                        let _ = slint::quit_event_loop();
                    }
                }
            }
            // Title, artist, album and cover from the shared now-playing track
            // (src/nowplaying.rs: tags, package metadata, stream titles, library).
            let path = p.get("path").unwrap_or_default();
            let t = crate::nowplaying::current();
            let t = if t.path == path {
                t
            } else {
                Default::default()
            };
            let n: i64 = p.mpv.get_property("playlist-count").unwrap_or(0);
            let i: i64 = p.mpv.get_property("playlist-pos").unwrap_or(-1);
            let pos = a.global::<crate::Playback>().get_position() as f64;
            let st = State {
                playing: a.global::<crate::Playback>().get_playing(),
                has_media: a.global::<crate::Playback>().get_has_media(),
                title: if t.title.is_empty() {
                    a.global::<crate::Playback>().get_media_title().to_string()
                } else {
                    t.title.clone()
                },
                artist: t.artist.clone(),
                album: t.album.clone(),
                album_artist: t.album_artist.clone(),
                track: t.track,
                url: if path.contains("://") {
                    path.clone()
                } else if path.is_empty() {
                    String::new()
                } else {
                    format!("file://{path}")
                },
                art_url: t.cover.clone(),
                length: a.global::<crate::Playback>().get_duration() as f64,
                position: pos,
                volume: a.global::<crate::Playback>().get_volume() as f64,
                rate: p.get("speed").and_then(|s| s.parse().ok()).unwrap_or(1.0),
                can_next: i + 1 < n,
                can_prev: i > 0,
            };
            let mut m = b.mpris.borrow_mut();
            m.update(st);
            // Report user seeks (and big jumps from the UI) as Seeked.
            let jumped = (pos - b.last_pos.get()).abs() > 1.5;
            b.last_pos.set(pos);
            if let Some(t) = seeked.or(jumped.then_some(pos)) {
                m.seeked(t);
            }
        });
    }
}
