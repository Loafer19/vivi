//! Desktop integration glue: MPRIS2 on Linux (media keys, playerctl, widgets).

use crate::player::Player;
use crate::AppWindow;
use std::rc::Rc;

#[cfg(target_os = "linux")]
pub fn setup(app: &AppWindow, player: &Rc<Player>, log: bool) {
    use crate::mpris::{Cmd, Mpris, State};
    use slint::ComponentHandle;
    use std::cell::RefCell;
    use std::time::Duration;

    let Some((mpris, rx)) = Mpris::start() else {
        if log {
            eprintln!("mpris: no session bus");
        }
        return;
    };
    if log {
        eprintln!("mpris: registered");
    }
    let mpris = RefCell::new(mpris);
    let last_pos = std::cell::Cell::new(0.0f64);
    let w = app.as_weak();
    let p = player.clone();
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(100),
        move || {
            let Some(a) = w.upgrade() else { return };
            let mut seeked = None;
            while let Ok(cmd) = rx.try_recv() {
                if log {
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
            let mut m = mpris.borrow_mut();
            m.update(st);
            // Report user seeks (and big jumps from the UI) as Seeked.
            let jumped = (pos - last_pos.get()).abs() > 1.5;
            last_pos.set(pos);
            if let Some(t) = seeked.or(jumped.then_some(pos)) {
                m.seeked(t);
            }
        },
    );
}

#[cfg(not(target_os = "linux"))]
pub fn setup(_app: &AppWindow, _player: &Rc<Player>, _log: bool) {}
