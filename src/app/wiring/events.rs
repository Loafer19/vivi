//! Player events (time, duration, file loaded, errors, stream loading state) applied
//! to the UI state globals when the mpv event thread wakes the UI (#107).

use crate::*;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

struct Drain {
    weak: slint::Weak<AppWindow>,
    player: Rc<Player>,
    cfg: Rc<RefCell<config::Config>>,
    events: Receiver<PlayerEvent>,
    log: bool,
    load: loading::Loading,
    last_err: String,
    fit: bool,
}

thread_local! {
    static DRAIN: RefCell<Option<Drain>> = const { RefCell::new(None) };
}

/// Coalesce bursts of mpv events into one UI-thread drain (time-pos can arrive often).
static WAKE: AtomicBool = AtomicBool::new(false);

/// Called from the mpv event thread after an event is queued.
pub(crate) fn wake() {
    if WAKE.swap(true, Ordering::AcqRel) {
        return;
    }
    if slint::invoke_from_event_loop(on_wake).is_err() {
        WAKE.store(false, Ordering::Release);
    }
}

fn on_wake() {
    WAKE.store(false, Ordering::Release);
    drain();
}

fn drain() {
    DRAIN.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(d) = slot.as_mut() else {
            return;
        };
        let Some(a) = d.weak.upgrade() else {
            return;
        };
        let was = (d.load.visible(), d.load.buffering());
        let mut hide = false;
        let mut activity = false;
        while let Ok(ev) = d.events.try_recv() {
            match ev {
                PlayerEvent::TimePos(t) => {
                    if d.log {
                        eprintln!("time-pos {t:.2}");
                    }
                    a.global::<crate::Playback>().set_position(t as f32)
                }
                PlayerEvent::Duration(dur) => {
                    a.global::<crate::Playback>().set_duration(dur as f32)
                }
                PlayerEvent::Pause(p) => {
                    hide = true;
                    activity = true;
                    a.global::<crate::Playback>().set_playing(!p)
                }
                PlayerEvent::Volume(v) => {
                    activity = true;
                    a.global::<crate::Playback>().set_volume(v as f32)
                }
                PlayerEvent::Mute(m) => {
                    activity = true;
                    a.global::<crate::Playback>().set_muted(m)
                }
                PlayerEvent::Title(t) => {
                    activity = true;
                    a.global::<crate::Playback>().set_media_title(t.into())
                }
                PlayerEvent::FileLoaded => {
                    hide = true;
                    activity = true;
                    d.fit = true;
                    a.global::<crate::Playback>().set_has_media(true);
                    refresh_lists(&a, &d.player, d.log);
                    on_file_loaded(&a, &d.player, d.log);
                    libview::flush_pending(&d.player);
                    // The docked library sidebar stays; the full library closes.
                    if a.global::<crate::Library>().get_full() {
                        a.global::<crate::Overlays>().set_library_open(false);
                        a.global::<crate::Library>().set_full(false);
                    }
                    a.global::<crate::Library>().invoke_refresh();
                    let dur: f64 = d.player.mpv.get_property("duration").unwrap_or(0.0);
                    preview::start(&a, &d.player.get("path").unwrap_or_default(), dur);
                    peaks::start(
                        &a,
                        &d.player.get("path").unwrap_or_default(),
                        a.global::<crate::Customize>().get_bar_style() == BarStyle::Waveform
                            && viz::audio_only(&d.player),
                    );
                    a.global::<crate::Playback>().set_range_a(-1.0);
                    a.global::<crate::Playback>().set_range_b(-1.0);
                    abloop::reset(&a, &d.player);
                    let c = d.cfg.borrow();
                    let key = settings::apply_preset(&d.player, &c, d.log);
                    settings::sync_panel(&a, &d.player);
                    drop(c);
                    // After the preset, so the file's own offset wins.
                    suboffset::file_loaded(&a, &d.player);
                    crate::nowplaying::file_loaded();
                    plugins::file_loaded();
                    crate::bookmarks::file_loaded(&d.player);
                    crate::sublocal::file_loaded(&a, &d.player);
                    crate::sleep::file_loaded(&d.player);
                    crate::transitions::file_loaded();
                    a.global::<crate::Panel>().set_preset_name(
                        key.or_else(|| d.player.get("path").map(|p| config::show_key(&p)))
                            .unwrap_or_default()
                            .into(),
                    );
                }
                PlayerEvent::ListsChanged => {
                    activity = true;
                    refresh_lists(&a, &d.player, d.log)
                }
                PlayerEvent::AbLoop => abloop::sync(&a, &d.player),
                PlayerEvent::LoopShuffle => crate::loopsync::sync(&a, &d.player),
                PlayerEvent::TrackCount(n) => {
                    if d.log {
                        eprintln!("tracks {n}");
                    }
                }
                PlayerEvent::StartFile => {
                    hide = true;
                    activity = true;
                    crate::transitions::start_file();
                    d.last_err.clear();
                    let path = d.player.get("path").unwrap_or_default();
                    d.load.start(&path);
                    if d.load.url().is_some() {
                        a.global::<crate::Notices>().set_stream_error("".into());
                        let t = d.player.get("media-title").unwrap_or_default();
                        a.global::<crate::Notices>()
                            .set_loading_name(loading::name_for(&path, &t).into());
                    }
                }
                PlayerEvent::PlaybackRestart => {
                    d.load.restart();
                    if std::mem::take(&mut d.fit) {
                        crate::winstate::fit_video(&a, &d.player, &d.cfg.borrow());
                    }
                }
                PlayerEvent::Buffering(b) => d.load.cache(b),
                PlayerEvent::EndFile(err) => {
                    hide = true;
                    activity = true;
                    crate::nowplaying::sync();
                    let name = a.global::<crate::Notices>().get_loading_name().to_string();
                    if let Some(url) = d.load.end(err) {
                        let why = if d.last_err.is_empty() {
                            "the stream did not respond".to_string()
                        } else {
                            d.last_err.clone()
                        };
                        show_stream_error(&a, &format!("Could not open {name}: {why}"), &url);
                    }
                }
                PlayerEvent::ClientMessage(args) => {
                    use crate::plugins::protocol::ScriptMessage as M;
                    match M::parse(&args) {
                        // Chapter Thumbnails package: new files in its cache.
                        Some(M::PreviewFrames) => {
                            preview::load_cached(&a, &d.player.get("path").unwrap_or_default());
                        }
                        Some(M::ChapterThumbs) => chapter_thumbs(&a, &d.player),
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
                            hide = true;
                        }
                        _ => plugins::client_message(&args),
                    }
                }
                PlayerEvent::Log(prefix, text) => {
                    if d.load.url().is_some() && !text.is_empty() {
                        d.last_err = text.chars().take(120).collect();
                    }
                    plugins::mpv_log(&prefix, &text)
                }
            }
        }
        if was != (d.load.visible(), d.load.buffering()) {
            a.global::<crate::Notices>()
                .set_loading_show(d.load.visible());
            a.global::<crate::Notices>()
                .set_loading_buffering(d.load.buffering());
        }
        if hide {
            crate::app::wiring::autohide::reconsider();
        }
        if activity {
            crate::plugins::arm_ticks();
            crate::streams::arm();
            crate::tray::sync();
        }
        // Commands arrive via invoke_from_event_loop; position and metadata
        // are published here so MPRIS does not need a 100 ms poll.
        crate::desktop::poke();
        a.window().request_redraw();
    });
}

pub(crate) fn wire(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &Rc<RefCell<config::Config>>,
    events: Receiver<PlayerEvent>,
    log: bool,
) {
    let weak = app.as_weak();
    {
        let w = weak.clone();
        let c = cfg.clone();
        let p = player.clone();
        app.global::<crate::Playback>().on_preview_wanted(move || {
            let Some(a) = w.upgrade() else { return };
            // First hover of this file: Chapter Thumbnails ("On hover") renders its frames.
            if preview::hovered(&a, &c.borrow()) {
                p.cmd("script-message", &["vivi-preview-wanted"]);
            }
        });
        let w = weak.clone();
        app.global::<crate::Notices>().on_hint_accept(move || {
            if let Some(a) = w.upgrade() {
                preview::hint_accept(&a);
            }
        });
        let w = weak.clone();
        let c = cfg.clone();
        app.global::<crate::Notices>().on_hint_never(move || {
            let Some(a) = w.upgrade() else { return };
            preview::hide_hint(&a);
            c.borrow_mut().interface.hint_chapter_thumbnails = false;
            if let Err(e) = c.borrow().save() {
                eprintln!("vivi: save config: {e}");
            }
            settings::refresh_ui();
        });
    }
    DRAIN.with(|slot| {
        *slot.borrow_mut() = Some(Drain {
            weak,
            player: player.clone(),
            cfg: cfg.clone(),
            events,
            log,
            load: loading::Loading::default(),
            last_err: String::new(),
            fit: false,
        });
    });
    // Events queued before wiring stay in the channel until the first wake; take them now.
    drain();
}
