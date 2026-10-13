mod player;
mod render;

use player::{Player, PlayerEvent};
use slint::ComponentHandle;
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
                        PlayerEvent::FileLoaded => a.set_has_media(true),
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

    app.run()?;
    Ok(())
}
