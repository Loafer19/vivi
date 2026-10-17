#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod about;
mod app;
mod config;
mod desktop;
mod dialogs;
mod fx;
mod ipc;
mod keymap;
mod library;
mod libview;
mod loading;
mod menus;
mod mini;
#[cfg(target_os = "linux")]
mod mpris;
mod overlays;
mod peaks;
mod player;
mod plugins;
mod preview;
mod render;
mod scenes;
mod settings;
mod store;
mod viz;

use app::debug::{debug_ui, open_plugin_link, panel_tab, screenshot_mode};
use app::keys::{forward_key, key_debug, LAST_PHYSICAL};
use app::panel::*;
use app::session::*;
use player::{Player, PlayerEvent};
use slint::{ComponentHandle, Model};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::include_modules!();

/// Environment for bundled files, set before any thread starts:
/// * `SLINT_FONT_PATH`: the Inter font shipped by the Flatpak/AppImage in
///   `<prefix>/share/vivi/fonts` (next to `<prefix>/bin/vivi`), unless already set.
/// * `PATH` gets the YouTube plugin's managed yt-dlp folder appended, so mpv's ytdl_hook
///   and the library find it when there is no system yt-dlp (always the case in the
///   Flatpak). A system yt-dlp earlier in `PATH` still wins.
fn bundled_env() {
    if std::env::var_os("SLINT_FONT_PATH").is_none() {
        let fonts = std::env::current_exe()
            .ok()
            .and_then(|e| Some(e.parent()?.parent()?.join("share/vivi/fonts")));
        if let Some(f) = fonts.filter(|f| f.is_dir()) {
            std::env::set_var("SLINT_FONT_PATH", f);
        }
    }
    let ytdlp = store::data_dir().join("plugin-data/youtube/bin");
    let mut paths: Vec<_> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if !paths.contains(&ytdlp) {
        paths.push(ytdlp);
        if let Ok(p) = std::env::join_paths(paths) {
            std::env::set_var("PATH", p);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // winit 0.30 has no Wayland drag & drop; VIVI_X11=1 runs under XWayland instead,
    // where dropping files works (#5). Done before any thread or backend starts.
    if std::env::var_os("VIVI_X11").is_some_and(|v| v != "0") {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    bundled_env();
    // Prefer winit + femtovg (OpenGL): works on Wayland (Hyprland) and X11.
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .select()?;

    let app = AppWindow::new()?;
    let log = std::env::var_os("VIVI_LOG").is_some();

    let weak = app.as_weak();
    let (player, events) = Player::new(move || {
        let w = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(a) = w.upgrade() {
                a.window().request_redraw();
            }
        });
    })?;
    let player = Rc::new(player);
    let cfg = Rc::new(std::cell::RefCell::new(config::Config::load()));
    settings::setup(&app, &player, cfg.clone(), log);
    libview::setup(&app, &player, cfg.clone(), log);
    menus::setup(&app, &player, cfg.clone());
    desktop::setup(&app, &player, log);
    mini::setup(&app, cfg.clone());
    viz::setup(&app, &player);
    plugins::setup(&app, &player, cfg.clone());
    fx::setup(&app, &player);
    dialogs::setup(&app, &player);
    ipc::setup(&app, &player, cfg.borrow().general.remote_control);
    if screenshot_mode() {
        debug_ui(&app);
    }

    // Drain player events on the UI thread.
    {
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
                        PlayerEvent::Volume(v) => {
                            a.global::<crate::Playback>().set_volume(v as f32)
                        }
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
                            if a.global::<crate::Library>().get_lib_full() {
                                a.global::<crate::Overlays>().set_library_open(false);
                                a.global::<crate::Library>().set_lib_full(false);
                            }
                            a.global::<crate::Library>().invoke_lib_refresh();
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
                                a.global::<crate::Customize>().get_bar_style()
                                    == BarStyle::Waveform
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
                            if load.end(err).is_some() {
                                let why = if last_err.is_empty() {
                                    "the stream did not respond".to_string()
                                } else {
                                    last_err.clone()
                                };
                                show_stream_error(&a, &format!("Could not open {name}: {why}"));
                            }
                        }
                        PlayerEvent::ClientMessage(args) => {
                            match args.first().map(String::as_str) {
                                // Chapter Thumbnails package: new files in its cache.
                                Some("vivi-preview-frames") => {
                                    preview::load_cached(
                                        &a,
                                        &player.get("path").unwrap_or_default(),
                                    );
                                }
                                Some("vivi-chapter-thumbs") => chapter_thumbs(&a, &player),
                                // `vivi-range <a> <b> [label]` / `vivi-range clear`: a plugin's
                                // range (Cut & Export In/Out) drawn on the seekbar.
                                Some("vivi-range") => {
                                    let t = |i: usize| {
                                        args.get(i)
                                            .and_then(|v| v.parse::<f32>().ok())
                                            .unwrap_or(-1.0)
                                    };
                                    a.global::<crate::Playback>().set_range_a(t(1));
                                    a.global::<crate::Playback>().set_range_b(t(2));
                                    a.global::<crate::Playback>().set_range_label(
                                        args.get(3).cloned().unwrap_or_default().into(),
                                    );
                                }
                                // `vivi-fullscreen [toggle|on|off]` (Remote Control package):
                                // Vivi owns the window, so mpv's own fullscreen does nothing.
                                Some("vivi-fullscreen") => {
                                    let w = a.window();
                                    let fs = match args.get(1).map(String::as_str) {
                                        Some("on") => true,
                                        Some("off") => false,
                                        _ => !w.is_fullscreen(),
                                    };
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

    // First CLI file opens once rendering is ready; further files are queued.
    // vivi://plugin/<id> (marketplace "Open in Vivi") opens the plugin in Extensions → Browse.
    let (links, cli): (Vec<String>, Vec<String>) = std::env::args()
        .skip(1)
        .partition(|a| a.starts_with("vivi://"));
    if let Some(id) = links.iter().find_map(|a| plugins::packages::deep_link(a)) {
        open_plugin_link(&app, &id);
    }
    let p = player.clone();
    render::attach(&app, player.mpv.clone(), move || {
        let mut it = cli.iter();
        if let Some(first) = it.next() {
            p.open(first);
        }
        for f in it {
            p.enqueue(f);
        }
    })?;

    let p = player.clone();
    app.global::<crate::Playback>()
        .on_toggle_play(move || p.toggle_pause());
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_seek(move |t| p.seek_abs(t as f64));
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_volume_changed(move |v| p.set_volume(v as f64));
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_toggle_mute(move || p.toggle_mute());
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_prev(move || p.playlist_step(false));
    let p = player.clone();
    app.global::<crate::Playback>()
        .on_next(move || p.playlist_step(true));

    let weak = app.as_weak();
    let toggle_fs = move || {
        if let Some(a) = weak.upgrade() {
            let w = a.window();
            let fs = !w.is_fullscreen();
            w.set_fullscreen(fs);
            a.set_fullscreen(fs);
        }
    };
    let tfs = toggle_fs.clone();
    app.global::<crate::Playback>().on_toggle_fullscreen(tfs);

    let open_file = {
        let p = player.clone();
        let weak = app.as_weak();
        move || {
            // rfd uses the XDG desktop portal (GTK/KDE/Hyprland portals) on Linux.
            let file = rfd::FileDialog::new()
                .set_title("Open media")
                .add_filter(
                    "Media",
                    &[
                        "mkv", "mp4", "webm", "avi", "mov", "m4v", "ts", "mp3", "flac", "ogg",
                        "opus", "m4a", "wav",
                    ],
                )
                .add_filter("All files", &["*"])
                .pick_file();
            if let Some(f) = file {
                p.open(&f.to_string_lossy());
            }
            if let Some(a) = weak.upgrade() {
                a.window().request_redraw();
            }
        }
    };
    let of = open_file.clone();
    app.global::<crate::Playback>().on_open_file(of);

    {
        let p = player.clone();
        app.global::<crate::Playback>().on_stop(move || {
            p.save_position();
            p.run("stop");
        });
        let p = player.clone();
        let weak = app.as_weak();
        app.global::<crate::Playback>().on_toggle_record(move || {
            // mpv stream-record: dump the stream to a file next to the user's videos.
            let rec = p.get("stream-record").unwrap_or_default();
            let key = keymap::shown(keymap::Action::Record);
            let msg = if !rec.is_empty() {
                p.set("stream-record", "");
                format!("Recording stopped · saved to {rec}")
            } else {
                let dir = store::home().join("Videos");
                let dir = if dir.is_dir() { dir } else { store::home() };
                let mut n = 1;
                let out = loop {
                    let f = dir.join(format!("vivi-recording-{n}.mkv"));
                    if !f.exists() {
                        break f;
                    }
                    n += 1;
                };
                p.set("stream-record", &out.to_string_lossy());
                format!("Recording to {} · press {key} to stop", out.display())
            };
            if let Some(a) = weak.upgrade() {
                plugins::toast(&a, &msg);
            }
        });
        let p = player.clone();
        app.global::<crate::Library>().on_open_folder(move || {
            let Some(d) = rfd::FileDialog::new()
                .set_title("Open folder")
                .pick_folder()
            else {
                return;
            };
            let mut files: Vec<String> = walk_media(&d);
            files.sort();
            for (i, f) in files.iter().enumerate() {
                if i == 0 {
                    p.open(f);
                } else {
                    p.enqueue(f);
                }
            }
        });
    }

    let p = player.clone();
    let weak = app.as_weak();
    app.on_key(move |text, shift, ctrl, alt| {
        let s = text.as_str();
        if key_debug() {
            let cps: Vec<String> = s.chars().map(|c| format!("U+{:04X}", c as u32)).collect();
            eprintln!(
                "vivi: key {s:?} [{}] shift={shift} ctrl={ctrl} alt={alt}",
                cps.join(" ")
            );
        }
        use keymap::Action as A;
        let action = keymap::lookup(s, shift, ctrl);
        // Alt combinations belong to mpv scripts / plugin packages (V is the exception:
        // the visualizer cycles with Alt held too, as it always did).
        if alt && action != Some(A::Visualizer) {
            return forward_key(&p, s, ctrl, alt);
        }
        let Some(action) = action else {
            forward_key(&p, s, ctrl, alt);
            return false;
        };
        let skip =
            |a: &std::sync::atomic::AtomicU32| a.load(std::sync::atomic::Ordering::Relaxed) as f64;
        let step = if shift {
            skip(&settings::LONG_SKIP)
        } else {
            skip(&settings::SHORT_SKIP)
        };
        let vstep = skip(&settings::VOLUME_STEP);
        let Some(a) = weak.upgrade() else {
            return false;
        };
        match action {
            A::PlayPause => p.toggle_pause(),
            A::SeekForward => p.seek_rel(step),
            A::SeekBack => p.seek_rel(-step),
            A::VolumeUp => p.add_volume(vstep),
            A::VolumeDown => p.add_volume(-vstep),
            A::Fullscreen => toggle_fs(),
            A::Miniplayer => {
                if a.global::<crate::Playback>().get_has_media() {
                    a.global::<crate::Playback>().invoke_toggle_mini();
                }
            }
            A::Mute => p.toggle_mute(),
            A::Stop => a.global::<crate::Playback>().invoke_stop(),
            A::QueueStep => p.playlist_step(!shift),
            A::AbLoop => a.global::<crate::Panel>().invoke_run("ab-loop".into()),
            A::Karaoke => crate::plugins::command(plugins::lyrics::ID, "karaoke", &[]),
            A::FindLyrics => crate::plugins::command(plugins::lyrics::ID, "search-open", &[]),
            // [ ] lyrics timing offset (Shift or { }: 0.5 s).
            A::LyricsOffset => {
                let big = shift || s == "{" || s == "}";
                let d = if big { 0.5 } else { 0.1 };
                let d = if s == "[" || s == "{" { -d } else { d };
                crate::plugins::command(plugins::lyrics::ID, "offset", &[d.to_string()]);
            }
            // z / x subtitle offset −0.1 / +0.1 s (Shift: 1 s), saved per file.
            A::SubtitleOffset => {
                let big = shift || s == "Z" || s == "X";
                let d = if big { 1.0 } else { 0.1 };
                let d = if s.eq_ignore_ascii_case("z") { -d } else { d };
                crate::plugins::command(plugins::subtitles::ID, "offset", &[d.to_string()]);
            }
            A::Visualizer => {
                let n = a.global::<crate::Viz>().get_modes().row_count() as i32;
                if !(a.global::<crate::Playback>().get_has_media()
                    && a.global::<crate::Viz>().get_audio())
                    || n == 0
                {
                    forward_key(&p, s, ctrl, alt);
                    return false;
                }
                a.global::<crate::Viz>()
                    .invoke_select((a.global::<crate::Viz>().get_mode() + 1).rem_euclid(n));
            }
            A::Record => a.global::<crate::Playback>().invoke_toggle_record(),
            A::HideUi => toggle_ui_hidden(&a),
            A::Quit => {
                let _ = slint::quit_event_loop();
            }
            A::OpenFile => open_file(),
            A::Settings => {
                a.global::<crate::Overlays>()
                    .set_settings_open(!a.global::<crate::Overlays>().get_settings_open());
                a.global::<crate::Settings>().invoke_settings_refresh(
                    a.global::<crate::Settings>().get_settings_category(),
                    "".into(),
                    false,
                );
            }
            A::Bookmark => a.global::<crate::Panel>().invoke_add_bookmark(),
            A::Shortcuts => a
                .global::<crate::Overlays>()
                .set_shortcuts_open(!a.global::<crate::Overlays>().get_shortcuts_open()),
            A::Escape => {
                overlays::escape(&a);
            }
        }
        true
    });

    // Auto-hide controls/cursor after 2s of inactivity while playing.
    let last = Rc::new(Cell::new(Instant::now()));
    {
        let last = last.clone();
        let weak = app.as_weak();
        app.on_activity(move || {
            last.set(Instant::now());
            if let Some(a) = weak.upgrade() {
                if !a.get_controls_visible() {
                    a.set_controls_visible(true);
                }
            }
        });
    }
    let hide_timer = slint::Timer::default();
    {
        let sub_margin = Cell::new(-1i64);
        let hp = player.clone();
        let weak = app.as_weak();
        hide_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(250),
            move || {
                let Some(a) = weak.upgrade() else { return };
                let idle = last.get().elapsed()
                    > Duration::from_secs(
                        settings::AUTOHIDE_SECS.load(std::sync::atomic::Ordering::Relaxed) as u64,
                    );
                let want = !(idle
                    && a.global::<crate::Playback>().get_playing()
                    && a.global::<crate::Playback>().get_has_media());
                if a.get_controls_visible() != want {
                    a.set_controls_visible(want);
                }
                // Lift subtitles above the bottom bar while it is shown.
                let bar = want || a.global::<crate::Overlays>().get_panel_open();
                let h = a
                    .window()
                    .size()
                    .to_logical(a.window().scale_factor())
                    .height
                    .max(1.0);
                let margin = if bar {
                    22 + (72.0 * 720.0 / h) as i64
                } else {
                    22
                };
                if sub_margin.get() != margin {
                    sub_margin.set(margin);
                    hp.set("sub-margin-y", &margin.to_string());
                }
            },
        );
    }

    // Drag & drop (winit delivers DroppedFile on X11/Windows/macOS; not on Wayland yet).
    {
        use slint::winit_030::{winit::event::WindowEvent, EventResult, WinitWindowAccessor};
        let p = player.clone();
        let weak = app.as_weak();
        app.window().on_winit_window_event(move |_w, ev| {
            // Remember the physical key of the key press Slint is about to dispatch, so
            // forwarded modifier combos are layout-independent (see `forward_key`).
            if let WindowEvent::KeyboardInput { event, .. } = ev {
                if event.state.is_pressed() {
                    LAST_PHYSICAL.set(Some(event.physical_key));
                }
            }
            if let WindowEvent::DroppedFile(path) = ev {
                if weak.upgrade().is_some_and(|a| dialogs::take_drop(&a, path)) {
                    return EventResult::PreventDefault;
                }
                // Drop on an idle player opens; otherwise adds to the queue.
                if p.get("path").is_some() {
                    p.enqueue(&path.to_string_lossy());
                } else {
                    p.open(&path.to_string_lossy());
                }
                return EventResult::PreventDefault;
            }
            EventResult::Propagate
        });
    }

    side_panel(&app, &player, log);
    queue_and_resume(&app, &player, log);

    app.run()?;
    player.save_position();
    Ok(())
}

pub(crate) fn playlist_items() -> slint::ModelRc<Item> {
    items(
        store::playlists()
            .into_iter()
            .map(|(n, p)| (p.to_string_lossy().into_owned(), n, false)),
    )
}
