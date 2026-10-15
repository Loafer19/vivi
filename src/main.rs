#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod config;
mod desktop;
mod dialogs;
mod fx;
mod ipc;
mod library;
mod libview;
mod loading;
mod menus;
mod mini;
#[cfg(target_os = "linux")]
mod mpris;
mod player;
mod plugins;
mod preview;
mod render;
mod scenes;
mod settings;
mod store;
mod viz;

use player::{Player, PlayerEvent};
use slint::{ComponentHandle, Model};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::include_modules!();

/// Error toast of a network stream that failed to open (shown without media too).
fn show_stream_error(app: &AppWindow, msg: &str) {
    app.set_stream_error(msg.into());
    let w = app.as_weak();
    let m = msg.to_string();
    slint::Timer::single_shot(Duration::from_secs(6), move || {
        if let Some(a) = w.upgrade() {
            if a.get_stream_error().as_str() == m {
                a.set_stream_error("".into());
            }
        }
    });
}

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
                            a.set_position(t as f32)
                        }
                        PlayerEvent::Duration(d) => a.set_duration(d as f32),
                        PlayerEvent::Pause(p) => a.set_playing(!p),
                        PlayerEvent::Volume(v) => a.set_volume(v as f32),
                        PlayerEvent::Mute(m) => a.set_muted(m),
                        PlayerEvent::Title(t) => a.set_media_title(t.into()),
                        PlayerEvent::FileLoaded => {
                            a.set_has_media(true);
                            refresh_lists(&a, &player, log);
                            on_file_loaded(&a, &player, log);
                            libview::flush_pending(&player);
                            // The docked library sidebar stays; the full library closes.
                            if a.get_lib_full() {
                                a.set_library_open(false);
                                a.set_lib_full(false);
                            }
                            a.invoke_lib_refresh();
                            let dur: f64 = player.mpv.get_property("duration").unwrap_or(0.0);
                            preview::start(
                                &a,
                                &player.get("path").unwrap_or_default(),
                                dur,
                                cfg.borrow().library.thumbnails,
                            );
                            let c = cfg.borrow();
                            let key = settings::apply_preset(&player, &c, log);
                            settings::sync_panel(&a, &player);
                            drop(c);
                            plugins::file_loaded();
                            a.set_preset_name(
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
                                a.set_stream_error("".into());
                                let t = player.get("media-title").unwrap_or_default();
                                a.set_loading_name(loading::name_for(&path, &t).into());
                            }
                        }
                        PlayerEvent::PlaybackRestart => load.restart(),
                        PlayerEvent::Buffering(b) => load.cache(b),
                        PlayerEvent::EndFile(err) => {
                            let name = a.get_loading_name().to_string();
                            if load.end(err).is_some() {
                                let why = if last_err.is_empty() {
                                    "the stream did not respond".to_string()
                                } else {
                                    last_err.clone()
                                };
                                show_stream_error(&a, &format!("Could not open {name}: {why}"));
                            }
                        }
                        PlayerEvent::ClientMessage(args) => plugins::client_message(&args),
                        PlayerEvent::Log(prefix, text) => {
                            if load.url().is_some() && !text.is_empty() {
                                last_err = text.chars().take(120).collect();
                            }
                            plugins::mpv_log(&prefix, &text)
                        }
                    }
                }
                if was != (load.visible(), load.buffering()) {
                    a.set_loading_show(load.visible());
                    a.set_loading_buffering(load.buffering());
                }
            },
        );
    }

    // First CLI file opens once rendering is ready; further files are queued.
    let cli: Vec<String> = std::env::args().skip(1).collect();
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
    app.on_toggle_play(move || p.toggle_pause());
    let p = player.clone();
    app.on_seek(move |t| p.seek_abs(t as f64));
    let p = player.clone();
    app.on_volume_changed(move |v| p.set_volume(v as f64));
    let p = player.clone();
    app.on_toggle_mute(move || p.toggle_mute());
    let p = player.clone();
    app.on_prev(move || p.playlist_step(false));
    let p = player.clone();
    app.on_next(move || p.playlist_step(true));

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
    app.on_toggle_fullscreen(tfs);

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
    app.on_open_file(of);

    {
        let p = player.clone();
        app.on_stop(move || {
            p.save_position();
            p.run("stop");
        });
        let p = player.clone();
        let weak = app.as_weak();
        app.on_toggle_record(move || {
            // mpv stream-record: dump the stream to a file next to the user's videos.
            let on = p.get("stream-record").is_some_and(|s| !s.is_empty());
            let msg = if on {
                p.set("stream-record", "");
                "Recording stopped".to_string()
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
                format!("Recording to {}", out.display())
            };
            if let Some(a) = weak.upgrade() {
                a.set_resume_text(msg.into());
            }
        });
        let p = player.clone();
        app.on_open_folder(move || {
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
        use slint::platform::Key;
        let s = text.as_str();
        let is = |k: Key| s == slint::SharedString::from(k).as_str();
        if key_debug() {
            let cps: Vec<String> = s.chars().map(|c| format!("U+{:04X}", c as u32)).collect();
            eprintln!(
                "vivi: key {s:?} [{}] shift={shift} ctrl={ctrl} alt={alt}",
                cps.join(" ")
            );
        }
        // Alt combinations belong to mpv scripts / plugin packages.
        if alt {
            return forward_key(&p, s, ctrl, alt);
        }
        let skip =
            |a: &std::sync::atomic::AtomicU32| a.load(std::sync::atomic::Ordering::Relaxed) as f64;
        let step = if shift {
            skip(&settings::LONG_SKIP)
        } else {
            skip(&settings::SHORT_SKIP)
        };
        let vstep = skip(&settings::VOLUME_STEP);
        match () {
            _ if s == " " || s == "k" => p.toggle_pause(),
            _ if is(Key::RightArrow) => p.seek_rel(step),
            _ if is(Key::LeftArrow) => p.seek_rel(-step),
            _ if is(Key::UpArrow) => p.add_volume(vstep),
            _ if is(Key::DownArrow) => p.add_volume(-vstep),
            _ if s == "f" => toggle_fs(),
            _ if s == "p" && !ctrl => {
                if let Some(a) = weak.upgrade() {
                    if a.get_has_media() {
                        a.invoke_toggle_mini();
                    }
                }
            }
            _ if s == "m" => p.toggle_mute(),
            _ if !ctrl && (s == "s" || s == "S") => {
                if let Some(a) = weak.upgrade() {
                    a.invoke_stop();
                }
            }
            _ if !ctrl && (s == "n" || s == "N") => p.playlist_step(!shift),
            _ if !ctrl && (s == "r" || s == "R") => {
                if let Some(a) = weak.upgrade() {
                    a.invoke_toggle_record();
                }
            }
            _ if ctrl && s == "\\" => {
                if let Some(a) = weak.upgrade() {
                    a.set_ui_hidden(!a.get_ui_hidden());
                }
            }
            _ if ctrl && s == "q" => {
                let _ = slint::quit_event_loop();
            }
            _ if ctrl && s == "o" => open_file(),
            _ if ctrl && s == "," => {
                if let Some(a) = weak.upgrade() {
                    a.set_settings_open(!a.get_settings_open());
                    a.invoke_settings_refresh(a.get_settings_category(), "".into(), false);
                }
            }
            _ if ctrl && s == "b" => {
                if let Some(a) = weak.upgrade() {
                    a.invoke_add_bookmark();
                }
            }
            _ if is(Key::Escape) => {
                if let Some(a) = weak.upgrade() {
                    if a.get_mini() {
                        a.invoke_toggle_mini();
                    } else if a.window().is_fullscreen() {
                        a.window().set_fullscreen(false);
                        a.set_fullscreen(false);
                    } else if a.get_customize_open() {
                        a.invoke_customize_done(false);
                    } else if a.get_subs_open() {
                        a.set_subs_open(false);
                    } else if a.get_md_open() {
                        a.set_md_open(false);
                        crate::plugins::command(&a.get_md_owner(), "close", &[]);
                    } else if a.get_settings_open() {
                        a.set_settings_open(false);
                    }
                }
            }
            _ => {
                forward_key(&p, s, ctrl, alt);
                return false;
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
                let want = !(idle && a.get_playing() && a.get_has_media());
                if a.get_controls_visible() != want {
                    a.set_controls_visible(want);
                }
                // Lift subtitles above the bottom bar while it is shown.
                let bar = want || a.get_panel_open();
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

fn items(v: impl IntoIterator<Item = (String, String, bool)>) -> slint::ModelRc<Item> {
    let v: Vec<Item> = v
        .into_iter()
        .map(|(key, label, selected)| Item {
            key: key.into(),
            label: label.into(),
            selected,
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(v))
}

/// Chapter thumbnails from the Scene Chapters package cache (empty images where missing).
fn chapter_thumbs(a: &AppWindow, p: &Player) {
    let n = p.chapters().len();
    let path = p.get("path").unwrap_or_default();
    let files = if path.contains("://") || path.is_empty() {
        vec![None; n]
    } else {
        scenes::thumbs(&scenes::cache_root(), &path, n)
    };
    let has: Vec<bool> = files.iter().map(Option::is_some).collect();
    let imgs: Vec<slint::Image> = files
        .iter()
        .map(|f| {
            f.as_deref()
                .and_then(|f| slint::Image::load_from_path(f).ok())
                .unwrap_or_default()
        })
        .collect();
    a.set_chapter_thumbs(slint::ModelRc::new(slint::VecModel::from(imgs)));
    a.set_chapter_has_thumb(slint::ModelRc::new(slint::VecModel::from(has)));
}

/// Re-read tracks, devices and chapters from mpv into the side panel models.
fn refresh_lists(a: &AppWindow, p: &Player, log: bool) {
    let tr = |kind| {
        items(
            p.tracks(kind)
                .into_iter()
                .map(|(id, l, s)| (id.to_string(), l, s)),
        )
    };
    a.set_sub_tracks(tr("sub"));
    a.set_audio_tracks(tr("audio"));
    let cur_dev = p.get("audio-device").unwrap_or_default();
    a.set_audio_devices(items(
        p.audio_devices()
            .into_iter()
            .map(|(n, d)| (n.clone(), d, n == cur_dev)),
    ));
    let cur_ch: i64 = p.mpv.get_property("chapter").unwrap_or(-1);
    a.set_chapters(items(p.chapters().into_iter().enumerate().map(
        |(i, (t, s))| {
            let m = (s / 60.0) as i64;
            (
                i.to_string(),
                format!("{t}  ·  {m}:{:02}", (s as i64) % 60),
                i as i64 == cur_ch,
            )
        },
    )));
    chapter_thumbs(a, p);
    a.set_hwdec(p.get("hwdec").unwrap_or_default().into());
    let q = p.queue();
    a.set_queue(items(
        q.iter()
            .enumerate()
            .map(|(i, (f, cur))| (i.to_string(), store::display_name(f), *cur)),
    ));
    mini::queue_meta(a, p, &q);
    if log {
        eprintln!(
            "lists aid={:?} sid={:?} audio={:?} sub={:?} queue={:?}",
            p.get("aid"),
            p.get("sid"),
            p.tracks("audio"),
            p.tracks("sub"),
            p.queue()
        );
    }
}

fn side_panel(app: &AppWindow, player: &Rc<Player>, log: bool) {
    let p = player.clone();
    let weak = app.as_weak();
    app.on_set_prop(move |name, value| {
        p.set(&name, &value);
        if log {
            eprintln!("set {name}={value} -> {:?}", p.get(&name));
        }
        if let Some(a) = weak.upgrade() {
            if matches!(
                name.as_str(),
                "hwdec" | "chapter" | "audio-device" | "aid" | "sid"
            ) {
                refresh_lists(&a, &p, log);
            }
        }
    });
    let p = player.clone();
    let weak = app.as_weak();
    {
        let (p, w) = (p.clone(), weak.clone());
        app.on_chapters_refresh(move || {
            if let Some(a) = w.upgrade() {
                chapter_thumbs(&a, &p);
            }
        });
    }
    app.on_run(move |cmd| {
        p.run(&cmd);
        if let Some(a) = weak.upgrade() {
            let ab = |k| p.get(k).filter(|v| v != "no");
            let st = match (ab("ab-loop-a"), ab("ab-loop-b")) {
                (Some(x), Some(y)) => format!("Looping {x:.5}s → {y:.5}s"),
                (Some(x), None) => format!("A = {x:.5}s, set B"),
                _ => "Off".into(),
            };
            if log {
                eprintln!("ab-loop {st}");
            }
            a.set_ab_state(st.into());
        }
    });
    let p = player.clone();
    app.on_load_sub(move || {
        if let Some(f) = rfd::FileDialog::new()
            .set_title("Add subtitles")
            .add_filter("Subtitles", &["srt", "ass", "ssa", "vtt", "sub"])
            .pick_file()
        {
            let path = f.to_string_lossy().replace('"', "\\\"");
            p.run(&format!("sub-add \"{path}\" select"));
        }
    });
    // Sleep timer: pause after N minutes, or at end of file (-1).
    let sleep_timer = Rc::new(slint::Timer::default());
    let p = player.clone();
    let weak = app.as_weak();
    app.on_sleep(move |min| {
        let Some(a) = weak.upgrade() else { return };
        sleep_timer.stop();
        p.set("keep-open", "yes");
        match min {
            0 => a.set_sleep_state("".into()),
            -1 => {
                a.set_sleep_state("Pausing at end of file".into());
                p.set("keep-open", "always");
            }
            m => {
                a.set_sleep_state(format!("Pausing in {m} min").into());
                let p = p.clone();
                let w = weak.clone();
                sleep_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_secs(m as u64 * 60),
                    move || {
                        p.set("pause", "yes");
                        if let Some(a) = w.upgrade() {
                            a.set_sleep_state("".into());
                        }
                    },
                );
            }
        }
    });
    // In-memory bookmarks (persisted later with settings).
    let marks: Rc<slint::VecModel<Item>> = Rc::new(slint::VecModel::default());
    app.set_bookmarks(marks.clone().into());
    let p = player.clone();
    app.on_add_bookmark(move || {
        let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
        let s = t as i64;
        marks.push(Item {
            key: format!("{t:.3}").into(),
            label: format!(
                "Bookmark {}  ·  {}:{:02}",
                marks.row_count() + 1,
                s / 60,
                s % 60
            )
            .into(),
            selected: false,
        });
        if log {
            eprintln!("bookmark at {t:.2}");
        }
    });
    // Debug/screenshot helpers.
    if let Ok(t) = std::env::var("VIVI_PANEL") {
        app.set_panel_tab(t.parse().unwrap_or(0));
        app.set_panel_open(true);
        if let Some(y) = std::env::var("VIVI_PANEL_SCROLL")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            let w = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(2500), move || {
                if let Some(a) = w.upgrade() {
                    a.set_panel_scroll(-y);
                }
            });
        }
    }
    if screenshot_mode() {
        if let Some(v) = std::env::var("VIVI_VOLUME")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
        {
            let p = player.clone();
            slint::Timer::single_shot(Duration::from_millis(2000), move || p.set_volume(v));
        }
        if let Ok(m) = std::env::var("VIVI_MINI") {
            let weak = app.as_weak();
            slint::Timer::single_shot(Duration::from_millis(2500), move || {
                if let Some(a) = weak.upgrade() {
                    a.invoke_toggle_mini();
                    if m == "queue" {
                        a.invoke_toggle_mini_queue();
                    }
                }
            });
        }
    }
    // Scripted self-test, debug builds only.
    if cfg!(debug_assertions) && std::env::var_os("VIVI_SELFTEST").is_some() {
        let weak = app.as_weak();
        let t = Box::leak(Box::new(slint::Timer::default()));
        let step = Cell::new(0);
        t.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(700),
            move || {
                let Some(a) = weak.upgrade() else { return };
                let n = step.get();
                step.set(n + 1);
                let ops: &[(&str, &str)] = &[
                    ("aid", "2"),
                    ("sid", "2"),
                    ("sub-delay", "0.5"),
                    ("audio-delay", "-0.25"),
                    ("af", "lavfi=[bass=g=8]"),
                    ("video-aspect-override", "4:3"),
                    ("video-rotate", "90"),
                    ("video-rotate", "0"),
                    ("brightness", "20"),
                    ("speed", "1.5"),
                    ("sid", "1"),
                    ("aid", "1"),
                ];
                if let Some((k, v)) = ops.get(n) {
                    a.invoke_set_prop((*k).into(), (*v).into());
                } else if n == ops.len() {
                    a.invoke_add_bookmark();
                    a.invoke_playlist_save("Selftest".into());
                    a.invoke_save_preset();
                    a.invoke_run("ab-loop".into());
                    a.invoke_run("ab-loop".into());
                }
            },
        );
    }
}

/// Debug builds, or release builds started with `VIVI_SCREENSHOT=1`: the env-driven UI
/// helpers below (used to capture docs/screenshots) are active.
fn screenshot_mode() -> bool {
    cfg!(debug_assertions) || std::env::var_os("VIVI_SCREENSHOT").is_some()
}

/// Debug builds (or `VIVI_SCREENSHOT=1`): open dialogs from env vars for screenshots/tests.
/// VIVI_SETTINGS=<category> [VIVI_ADVANCED=1] [VIVI_SEARCH=text], VIVI_CUSTOMIZE=<profile>,
/// VIVI_ONBOARDING=<step>, VIVI_LIB=<nav>, VIVI_LIBQ=<search>, VIVI_NETWORK [VIVI_NET_URLS=a|b VIVI_NET_NAME],
/// VIVI_NEW_PLAYLIST=<name>, VIVI_URL=<url>, VIVI_LIBRARY,
/// VIVI_MENU, VIVI_SUB, VIVI_SUBS[=folder|search] (Add Subtitles dialog), VIVI_MUSIC=<plugin id>
/// [VIVI_MUSICQ=query] (Auto-tagger / Free Music dialog, searched when a query is given).
fn debug_ui(app: &AppWindow) {
    let var = |k: &str| std::env::var(k).ok();
    if let Some(n) = var("VIVI_SETTINGS").and_then(|v| v.parse().ok()) {
        let adv = var("VIVI_ADVANCED").is_some();
        let q = var("VIVI_SEARCH").unwrap_or_default();
        app.set_settings_open(true);
        app.set_settings_category(n);
        app.set_settings_advanced(adv);
        app.set_settings_search(q.clone().into());
        app.invoke_settings_refresh(n, q.into(), adv);
    }
    if let Some(p) = var("VIVI_CUSTOMIZE") {
        app.invoke_customize_profile(p.into());
    }
    if let Some(n) = var("VIVI_LIB") {
        app.invoke_lib_navigate(n.into());
    }
    if let Some(q) = var("VIVI_LIBQ") {
        app.set_lib_query(q.clone().into());
        app.invoke_lib_search(q.into());
    }
    if let Some(u) = var("VIVI_URL") {
        // After the render context exists (files must not load before it).
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1500), move || {
            if let Some(a) = w.upgrade() {
                a.invoke_open_url(u.into());
            }
        });
    }
    if let Some(id) = var("VIVI_MUSIC") {
        let q = var("VIVI_MUSICQ");
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2000), move || {
            plugins::command(&id, "open", &[]);
            if let (Some(a), Some(q)) = (w.upgrade(), q) {
                a.set_md_query(q.into());
                plugins::command(&id, "search", &[]);
            }
        });
    }
    if var("VIVI_NETWORK").is_some() {
        if let Some(u) = var("VIVI_NET_URLS") {
            app.set_net_urls(u.replace('|', "\n").into());
        }
        if let Some(n) = var("VIVI_NET_NAME") {
            app.set_net_name(n.into());
        }
        app.set_network_open(true);
    }
    if let Some(n) = var("VIVI_NEW_PLAYLIST") {
        app.set_np_name(n.into());
        app.set_np_open(true);
    }
    if var("VIVI_LIBRARY").is_some() {
        app.set_library_open(true);
    }
    // VIVI_MENU="kind;x;y;ctx" (ctx lines joined by '|'), VIVI_SUB="row-id;y": open a menu.
    if let Some(m) = var("VIVI_MENU") {
        let sub = var("VIVI_SUB");
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2500), move || {
            let Some(a) = w.upgrade() else { return };
            let f: Vec<&str> = m.splitn(4, ';').collect();
            let num = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(100.0);
            let ctx = f.get(3).copied().unwrap_or("").replace('|', "\n");
            a.invoke_menu_request(f[0].into(), num(1), num(2), ctx.into());
            if let Some((id, y)) = sub.as_deref().and_then(|s| s.split_once(';')) {
                a.set_sub_y(y.parse().unwrap_or(0.0));
                a.set_sub_open(id.into());
                a.invoke_menu_sub(id.into());
            }
        });
    }
    // VIVI_SUBS=folder: "To This Folder"; =search: "From OpenSubtitles".
    if let Some(m) = var("VIVI_SUBS") {
        app.set_subs_target(i32::from(m == "folder"));
        app.set_subs_source(i32::from(m != "search"));
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2500), move || {
            if let Some(a) = w.upgrade() {
                a.invoke_add_subs();
            }
        });
    }
    if let Some(n) = var("VIVI_ONBOARDING").and_then(|v| v.parse().ok()) {
        app.set_onboarding_step(n);
        app.set_onboarding_open(true);
    }
}

fn recent_items() -> slint::ModelRc<Item> {
    items(
        store::recent()
            .into_iter()
            .take(8)
            .map(|p| (p.clone(), store::display_name(&p), false)),
    )
}

pub(crate) fn playlist_items() -> slint::ModelRc<Item> {
    items(
        store::playlists()
            .into_iter()
            .map(|(n, p)| (p.to_string_lossy().into_owned(), n, false)),
    )
}

/// Remember the file in Recent and offer "Start over" when mpv resumed mid-file.
fn on_file_loaded(a: &AppWindow, p: &Rc<Player>, log: bool) {
    if let Some(path) = p.get("path") {
        let abs = std::fs::canonicalize(&path)
            .map(|x| x.to_string_lossy().into_owned())
            .unwrap_or(path);
        store::push_recent(&abs);
        a.set_recent(recent_items());
    }
    let p = p.clone();
    let w = a.as_weak();
    slint::Timer::single_shot(Duration::from_millis(300), move || {
        let Some(a) = w.upgrade() else { return };
        let t: f64 = p.mpv.get_property("time-pos").unwrap_or(0.0);
        if log {
            eprintln!("loaded, start at {t:.2}");
        }
        if t > 5.0 {
            let s = t as i64;
            a.set_resume_text(
                format!("Resumed from {}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60).into(),
            );
            let w = a.as_weak();
            slint::Timer::single_shot(Duration::from_secs(8), move || {
                if let Some(a) = w.upgrade() {
                    a.set_resume_text("".into());
                }
            });
        } else {
            a.set_resume_text("".into());
        }
    });
}

fn queue_and_resume(app: &AppWindow, player: &Rc<Player>, log: bool) {
    app.set_recent(recent_items());
    app.set_playlists(playlist_items());

    let p = player.clone();
    app.on_queue_play(move |i| p.play_index(i));
    let p = player.clone();
    app.on_queue_move(move |i, d| p.move_index(i, d));
    let p = player.clone();
    app.on_queue_remove(move |i| p.remove_index(i));
    let p = player.clone();
    app.on_queue_clear(move || p.clear_queue());
    let p = player.clone();
    app.on_queue_sort(move || p.sort_queue());
    let p = player.clone();
    app.on_queue_add(move || {
        if let Some(files) = rfd::FileDialog::new()
            .set_title("Add to queue")
            .add_filter(
                "Media",
                &[
                    "mkv", "mp4", "webm", "avi", "mov", "m4v", "ts", "mp3", "flac", "ogg", "opus",
                    "m4a", "wav",
                ],
            )
            .add_filter("All files", &["*"])
            .pick_files()
        {
            for f in files {
                p.enqueue(&f.to_string_lossy());
            }
        }
    });
    let p = player.clone();
    let weak = app.as_weak();
    app.on_playlist_save(move |name| {
        let files: Vec<String> = p
            .queue()
            .into_iter()
            .map(|(f, _)| {
                std::fs::canonicalize(&f)
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or(f)
            })
            .collect();
        if files.is_empty() {
            return;
        }
        match store::save_playlist(&name, &files) {
            Ok(path) if log => eprintln!("saved playlist {}", path.display()),
            Err(e) => eprintln!("vivi: save playlist: {e}"),
            _ => {}
        }
        if let Some(a) = weak.upgrade() {
            a.set_playlists(playlist_items());
        }
    });
    let p = player.clone();
    app.on_playlist_load(move |path| p.open(&path));
    let weak = app.as_weak();
    app.on_playlist_delete(move |path| {
        store::delete_playlist(std::path::Path::new(path.as_str()));
        if let Some(a) = weak.upgrade() {
            a.set_playlists(playlist_items());
        }
    });
    let p = player.clone();
    app.on_open_recent(move |path| p.open(&path));
    let weak = app.as_weak();
    app.on_clear_recent(move || {
        store::clear_recent();
        if let Some(a) = weak.upgrade() {
            a.set_recent(recent_items());
        }
    });
    let p = player.clone();
    let weak = app.as_weak();
    app.on_start_over(move || {
        p.seek_abs(0.0);
        if let Some(a) = weak.upgrade() {
            a.set_resume_text("".into());
        }
    });
    let weak = app.as_weak();
    app.on_dismiss_resume(move || {
        if let Some(a) = weak.upgrade() {
            a.set_resume_text("".into());
        }
    });
    // Save the resume position periodically so a crash or kill keeps it.
    let p = player.clone();
    let t = Box::leak(Box::new(slint::Timer::default()));
    t.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(5),
        move || {
            if p.get("pause").as_deref() == Some("no") {
                p.save_position();
            }
        },
    );
}

/// Media files under a folder (recursive), for "Open Folder".
fn walk_media(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_media(&p));
        } else if library::kind_of(&p).is_some() {
            out.push(p.to_string_lossy().into_owned());
        }
    }
    out
}

/// Send a key Vivi does not use to mpv (`keypress`), so key bindings of mpv scripts and
/// plugin packages work. Returns whether it was sent.
fn forward_key(p: &player::Player, text: &str, ctrl: bool, alt: bool) -> bool {
    let phys = LAST_PHYSICAL.take();
    let k = phys
        .filter(|_| ctrl || alt)
        .and_then(|pk| physical_key(pk, text, ctrl, alt))
        .or_else(|| mpv_key(text, ctrl, alt));
    let Some(k) = k else {
        if key_debug() {
            eprintln!(
                "vivi: key {text:?} (ctrl={ctrl} alt={alt} physical={phys:?}) has no mpv name"
            );
        }
        return false;
    };
    if key_debug() {
        eprintln!("vivi: keypress {k} (physical={phys:?})");
    }
    p.cmd("keypress", &[&k]);
    true
}

thread_local! {
    /// Physical key of the latest winit key press (set in the winit event hook).
    static LAST_PHYSICAL: std::cell::Cell<Option<slint::winit_030::winit::keyboard::PhysicalKey>> =
        const { std::cell::Cell::new(None) };
}

/// US-QWERTY character printed on a physical key (letters lowercase), the layout mpv
/// key bindings are written for.
fn physical_char(code: slint::winit_030::winit::keyboard::KeyCode) -> Option<char> {
    use slint::winit_030::winit::keyboard::KeyCode as K;
    const LETTERS: [K; 26] = [
        K::KeyA,
        K::KeyB,
        K::KeyC,
        K::KeyD,
        K::KeyE,
        K::KeyF,
        K::KeyG,
        K::KeyH,
        K::KeyI,
        K::KeyJ,
        K::KeyK,
        K::KeyL,
        K::KeyM,
        K::KeyN,
        K::KeyO,
        K::KeyP,
        K::KeyQ,
        K::KeyR,
        K::KeyS,
        K::KeyT,
        K::KeyU,
        K::KeyV,
        K::KeyW,
        K::KeyX,
        K::KeyY,
        K::KeyZ,
    ];
    const DIGITS: [K; 10] = [
        K::Digit0,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
    ];
    if let Some(i) = LETTERS.iter().position(|k| *k == code) {
        return Some((b'a' + i as u8) as char);
    }
    if let Some(i) = DIGITS.iter().position(|k| *k == code) {
        return Some((b'0' + i as u8) as char);
    }
    Some(match code {
        K::Minus => '-',
        K::Equal => '=',
        K::BracketLeft => '[',
        K::BracketRight => ']',
        K::Backslash => '\\',
        K::Semicolon => ';',
        K::Quote => '\'',
        K::Backquote => '`',
        K::Comma => ',',
        K::Period => '.',
        K::Slash => '/',
        _ => return None,
    })
}

/// mpv name of a modifier combo from the physical key (layout-independent). Only
/// character keys are handled here; named keys (arrows, …) keep the Slint text path.
/// `text` decides the case: an upper-case character means Shift was held.
fn physical_key(
    pk: slint::winit_030::winit::keyboard::PhysicalKey,
    text: &str,
    ctrl: bool,
    alt: bool,
) -> Option<String> {
    let slint::winit_030::winit::keyboard::PhysicalKey::Code(code) = pk else {
        return None;
    };
    let ch = physical_char(code)?;
    let upper = text.chars().next().is_some_and(char::is_uppercase);
    let k = if upper && ch.is_ascii_lowercase() {
        ch.to_ascii_uppercase().to_string()
    } else {
        ch.to_string()
    };
    let k = if k == "#" { "SHARP".into() } else { k };
    let mut k = k;
    if alt {
        k = format!("Alt+{k}");
    }
    if ctrl {
        k = format!("Ctrl+{k}");
    }
    Some(k)
}

/// `VIVI_KEY_DEBUG=1` logs every key event and what is forwarded to mpv.
fn key_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIVI_KEY_DEBUG").is_some())
}

/// Fallback when no physical key code is known: QWERTY letter on the same physical key
/// as a Cyrillic (Ukrainian/Russian ЙЦУКЕН) letter.
fn latin_of(ch: char) -> Option<char> {
    const CYR: &str = "йцукенгшщзхїфівапролджєячсмитьбюыэъё";
    const LAT: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.s']`";
    let lower = ch.to_lowercase().next()?;
    let i = CYR.chars().position(|c| c == lower)?;
    let l = LAT.chars().nth(i)?;
    Some(if ch.is_uppercase() {
        l.to_ascii_uppercase()
    } else {
        l
    })
}

/// mpv key name (`Alt+n`, `Ctrl+LEFT`, `SPACE`, …) for a Slint key event text.
fn mpv_key(text: &str, ctrl: bool, alt: bool) -> Option<String> {
    use slint::platform::Key;
    let named = [
        (Key::LeftArrow, "LEFT"),
        (Key::RightArrow, "RIGHT"),
        (Key::UpArrow, "UP"),
        (Key::DownArrow, "DOWN"),
        (Key::Home, "HOME"),
        (Key::End, "END"),
        (Key::PageUp, "PGUP"),
        (Key::PageDown, "PGDWN"),
    ];
    let mut k = named
        .iter()
        .find(|(k, _)| text == slint::SharedString::from(*k).as_str())
        .map(|(_, n)| n.to_string())
        .or_else(|| {
            let mut c = text.chars();
            match (c.next(), c.next()) {
                (Some(' '), None) => Some("SPACE".into()),
                (Some('#'), None) => Some("SHARP".into()),
                (Some(ch), None) if ch.is_ascii_graphic() => Some(ch.to_string()),
                // Ctrl+letter may arrive as a control character (Ctrl+A = U+0001).
                (Some(ch @ '\u{1}'..='\u{1a}'), None) if ctrl => {
                    Some(((ch as u8 - 1 + b'a') as char).to_string())
                }
                (Some(ch), None) if alt || ctrl => latin_of(ch).map(|l| l.to_string()),
                _ => None,
            }
        })?;
    if alt {
        k = format!("Alt+{k}");
    }
    if ctrl {
        k = format!("Ctrl+{k}");
    }
    Some(k)
}

#[cfg(test)]
mod key_tests {
    use super::mpv_key;

    #[test]
    fn alt_combos_get_mpv_names() {
        assert_eq!(mpv_key("n", false, true).as_deref(), Some("Alt+n"));
        assert_eq!(mpv_key("a", false, true).as_deref(), Some("Alt+a"));
        assert_eq!(mpv_key("A", false, true).as_deref(), Some("Alt+A"));
        let left = slint::SharedString::from(slint::platform::Key::LeftArrow);
        assert_eq!(mpv_key(&left, false, true).as_deref(), Some("Alt+LEFT"));
        assert_eq!(mpv_key(",", false, false).as_deref(), Some(","));
        assert_eq!(mpv_key(" ", true, false).as_deref(), Some("Ctrl+SPACE"));
    }

    #[test]
    fn cyrillic_layout_maps_to_qwerty() {
        // Ukrainian layout: N key types "т", A "ф", B "и", C "с", S "і".
        assert_eq!(mpv_key("т", false, true).as_deref(), Some("Alt+n"));
        assert_eq!(mpv_key("ф", false, true).as_deref(), Some("Alt+a"));
        assert_eq!(mpv_key("и", false, true).as_deref(), Some("Alt+b"));
        assert_eq!(mpv_key("с", false, true).as_deref(), Some("Alt+c"));
        assert_eq!(mpv_key("і", false, true).as_deref(), Some("Alt+s"));
        assert_eq!(mpv_key("т", false, false), None);
    }

    #[test]
    fn physical_keys_are_layout_independent() {
        use super::physical_key;
        use slint::winit_030::winit::keyboard::{KeyCode, NativeKeyCode, PhysicalKey};
        let n = PhysicalKey::Code(KeyCode::KeyN);
        // Ukrainian layout types "т" on N, Greek "ν", German "n": all map to Alt+n.
        for t in ["т", "ν", "n"] {
            assert_eq!(physical_key(n, t, false, true).as_deref(), Some("Alt+n"));
        }
        assert_eq!(physical_key(n, "Т", false, true).as_deref(), Some("Alt+N"));
        // German QWERTZ: the key labelled Z sits on US Y.
        let y = PhysicalKey::Code(KeyCode::KeyY);
        assert_eq!(physical_key(y, "z", true, false).as_deref(), Some("Ctrl+y"));
        let d1 = PhysicalKey::Code(KeyCode::Digit1);
        assert_eq!(
            physical_key(d1, "&", true, true).as_deref(),
            Some("Ctrl+Alt+1")
        );
        assert_eq!(
            physical_key(PhysicalKey::Code(KeyCode::ArrowLeft), "", false, true),
            None
        );
        assert_eq!(
            physical_key(
                PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
                "т",
                false,
                true
            ),
            None
        );
    }

    #[test]
    fn ctrl_control_chars() {
        assert_eq!(mpv_key("\u{1}", true, false).as_deref(), Some("Ctrl+a"));
    }
}
