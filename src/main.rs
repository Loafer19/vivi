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
    app::wiring::events::wire(&app, &player, &cfg, events, log);

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

    let (toggle_fs, open_file) = app::wiring::transport::wire(&app, &player);

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
