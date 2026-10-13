mod player;
mod render;

use player::{Player, PlayerEvent};
use slint::{ComponentHandle, Model};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Drain player events on the UI thread.
    {
        let weak = app.as_weak();
        let player = player.clone();
        let timer = Box::leak(Box::new(slint::Timer::default()));
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(30),
            move || {
                let Some(a) = weak.upgrade() else { return };
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
                        }
                        PlayerEvent::ListsChanged => refresh_lists(&a, &player, log),
                        PlayerEvent::TrackCount(n) => {
                            if log {
                                eprintln!("tracks {n}");
                            }
                        }
                        PlayerEvent::EndFile => {}
                    }
                }
            },
        );
    }

    let cli_path = std::env::args().nth(1);
    let p = player.clone();
    render::attach(&app, player.mpv.clone(), move || {
        if let Some(path) = cli_path {
            p.open(&path);
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

    let p = player.clone();
    let weak = app.as_weak();
    app.on_key(move |text, shift, ctrl| {
        use slint::platform::Key;
        let s = text.as_str();
        let is = |k: Key| s == slint::SharedString::from(k).as_str();
        let step = if shift { 60.0 } else { 5.0 };
        match () {
            _ if s == " " || s == "k" => p.toggle_pause(),
            _ if is(Key::RightArrow) => p.seek_rel(step),
            _ if is(Key::LeftArrow) => p.seek_rel(-step),
            _ if is(Key::UpArrow) => p.add_volume(5.0),
            _ if is(Key::DownArrow) => p.add_volume(-5.0),
            _ if s == "f" => toggle_fs(),
            _ if s == "m" => p.toggle_mute(),
            _ if ctrl && s == "o" => open_file(),
            _ if ctrl && s == "b" => {
                if let Some(a) = weak.upgrade() {
                    a.invoke_add_bookmark();
                }
            }
            _ if is(Key::Escape) => {
                if let Some(a) = weak.upgrade() {
                    if a.window().is_fullscreen() {
                        a.window().set_fullscreen(false);
                        a.set_fullscreen(false);
                    }
                }
            }
            _ => return false,
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
        let weak = app.as_weak();
        hide_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(250),
            move || {
                let Some(a) = weak.upgrade() else { return };
                let idle = last.get().elapsed() > Duration::from_secs(2);
                let want = !(idle && a.get_playing() && a.get_has_media());
                if a.get_controls_visible() != want {
                    a.set_controls_visible(want);
                }
            },
        );
    }

    // Drag & drop (winit delivers DroppedFile on X11/Windows/macOS; not on Wayland yet).
    {
        use slint::winit_030::{winit::event::WindowEvent, EventResult, WinitWindowAccessor};
        let p = player.clone();
        app.window().on_winit_window_event(move |_w, ev| {
            if let WindowEvent::DroppedFile(path) = ev {
                p.open(&path.to_string_lossy());
                return EventResult::PreventDefault;
            }
            EventResult::Propagate
        });
    }

    side_panel(&app, &player, log);

    app.run()?;
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
    a.set_hwdec(p.get("hwdec").unwrap_or_default().into());
    if log {
        eprintln!(
            "lists aid={:?} sid={:?} audio={:?} sub={:?}",
            p.get("aid"),
            p.get("sid"),
            p.tracks("audio"),
            p.tracks("sub")
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
    }
    if std::env::var_os("VIVI_SELFTEST").is_some() {
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
                    a.invoke_run("ab-loop".into());
                    a.invoke_run("ab-loop".into());
                }
            },
        );
    }
}
