//! Player events (time, duration, file loaded, errors, stream loading state) drained
//! on the UI thread every 30 ms and applied to the UI state globals.

use crate::*;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

pub(crate) fn wire(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &Rc<RefCell<config::Config>>,
    events: Receiver<PlayerEvent>,
    log: bool,
) {
    let weak = app.as_weak();
    let player = player.clone();
    let cfg = cfg.clone();
    let timer = Box::leak(Box::new(slint::Timer::default()));
    let mut load = loading::Loading::default();
    let mut last_err = String::new();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(30),
        move || {
            let Some(a) = weak.upgrade() else { return };
            let was = (load.visible(), load.buffering());
            while let Ok(ev) = events.try_recv() {
                match ev {
                    PlayerEvent::TimePos(t) => {
                        if log {
                            eprintln!("time-pos {t:.2}");
                        }
                        a.global::<crate::Playback>().set_position(t as f32)
                    }
                    PlayerEvent::Duration(d) => {
                        a.global::<crate::Playback>().set_duration(d as f32)
                    }
                    PlayerEvent::Pause(p) => a.global::<crate::Playback>().set_playing(!p),
                    PlayerEvent::Volume(v) => a.global::<crate::Playback>().set_volume(v as f32),
                    PlayerEvent::Mute(m) => a.global::<crate::Playback>().set_muted(m),
                    PlayerEvent::Title(t) => {
                        a.global::<crate::Playback>().set_media_title(t.into())
                    }
                    PlayerEvent::FileLoaded => {
                        a.global::<crate::Playback>().set_has_media(true);
                        refresh_lists(&a, &player, log);
                        on_file_loaded(&a, &player, log);
                        libview::flush_pending(&player);
                        // The docked library sidebar stays; the full library closes.
                        if a.global::<crate::Library>().get_full() {
                            a.global::<crate::Overlays>().set_library_open(false);
                            a.global::<crate::Library>().set_full(false);
                        }
                        a.global::<crate::Library>().invoke_refresh();
                        let dur: f64 = player.mpv.get_property("duration").unwrap_or(0.0);
                        preview::start(
                            &a,
                            &player.get("path").unwrap_or_default(),
                            dur,
                            cfg.borrow().library.thumbnails,
                        );
                        peaks::start(
                            &a,
                            &player.get("path").unwrap_or_default(),
                            a.global::<crate::Customize>().get_bar_style() == BarStyle::Waveform
                                && viz::audio_only(&player),
                        );
                        a.global::<crate::Playback>().set_range_a(-1.0);
                        a.global::<crate::Playback>().set_range_b(-1.0);
                        let c = cfg.borrow();
                        let key = settings::apply_preset(&player, &c, log);
                        settings::sync_panel(&a, &player);
                        drop(c);
                        plugins::file_loaded();
                        a.global::<crate::Panel>().set_preset_name(
                            key.or_else(|| player.get("path").map(|p| config::show_key(&p)))
                                .unwrap_or_default()
                                .into(),
                        );
                    }
                    PlayerEvent::ListsChanged => refresh_lists(&a, &player, log),
                    PlayerEvent::TrackCount(n) => {
                        if log {
                            eprintln!("tracks {n}");
                        }
                    }
                    PlayerEvent::StartFile => {
                        last_err.clear();
                        let path = player.get("path").unwrap_or_default();
                        load.start(&path);
                        if load.url().is_some() {
                            a.global::<crate::Notices>().set_stream_error("".into());
                            let t = player.get("media-title").unwrap_or_default();
                            a.global::<crate::Notices>()
                                .set_loading_name(loading::name_for(&path, &t).into());
                        }
                    }
                    PlayerEvent::PlaybackRestart => load.restart(),
                    PlayerEvent::Buffering(b) => load.cache(b),
                    PlayerEvent::EndFile(err) => {
                        let name = a.global::<crate::Notices>().get_loading_name().to_string();
                        if let Some(url) = load.end(err) {
                            let why = if last_err.is_empty() {
                                "the stream did not respond".to_string()
                            } else {
                                last_err.clone()
                            };
                            show_stream_error(&a, &format!("Could not open {name}: {why}"), &url);
                        }
                    }
                    PlayerEvent::ClientMessage(args) => {
                        use crate::plugins::protocol::ScriptMessage as M;
                        match M::parse(&args) {
                            // Chapter Thumbnails package: new files in its cache.
                            Some(M::PreviewFrames) => {
                                preview::load_cached(&a, &player.get("path").unwrap_or_default());
                            }
                            Some(M::ChapterThumbs) => chapter_thumbs(&a, &player),
                            // A plugin's range (Cut & Export In/Out) drawn on the seekbar.
                            Some(M::Range {
                                a: ra,
                                b: rb,
                                label,
                            }) => {
                                a.global::<crate::Playback>().set_range_a(ra);
                                a.global::<crate::Playback>().set_range_b(rb);
                                a.global::<crate::Playback>().set_range_label(label.into());
                            }
                            // Remote Control package: Vivi owns the window, so mpv's own
                            // fullscreen does nothing.
                            Some(M::Fullscreen(on)) => {
                                let w = a.window();
                                let fs = on.unwrap_or(!w.is_fullscreen());
                                w.set_fullscreen(fs);
                                a.set_fullscreen(fs);
                            }
                            _ => plugins::client_message(&args),
                        }
                    }
                    PlayerEvent::Log(prefix, text) => {
                        if load.url().is_some() && !text.is_empty() {
                            last_err = text.chars().take(120).collect();
                        }
                        plugins::mpv_log(&prefix, &text)
                    }
                }
            }
            if was != (load.visible(), load.buffering()) {
                a.global::<crate::Notices>()
                    .set_loading_show(load.visible());
                a.global::<crate::Notices>()
                    .set_loading_buffering(load.buffering());
            }
        },
    );
}
