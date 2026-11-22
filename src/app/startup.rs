//! Process start: environment for bundled files, backend selection and the per-feature setup calls.

use crate::*;
use std::rc::Rc;

/// Environment and Slint backend; call before any thread or window is created.
pub(crate) fn init() -> Result<(), slint::PlatformError> {
    // winit 0.30 has no Wayland drag & drop; VIVI_X11=1 runs under XWayland instead,
    // where dropping files works (#5). Done before any thread or backend starts.
    if std::env::var_os("VIVI_X11").is_some_and(|v| v != "0") {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    bundled_env();
    // Prefer winit + femtovg (OpenGL): works on Wayland (Hyprland) and X11. Without an
    // OpenGL 2 driver (Windows in a VM: only GDI's OpenGL 1.1) Slint falls back to its
    // software renderer by itself; VIVI_RENDERER=software asks for it directly, and
    // `relaunch_software` restarts with it when OpenGL fails later (Linux: EGL errors).
    let renderer = if software_forced() {
        crate::app::fatal::note("renderer: software (VIVI_RENDERER)");
        "software"
    } else {
        "femtovg"
    };
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        .select()?;
    Ok(())
}

/// `VIVI_RENDERER=software` (or `sw`): use Slint's software renderer, no OpenGL.
pub(crate) fn software_forced() -> bool {
    std::env::var("VIVI_RENDERER")
        .is_ok_and(|v| v.eq_ignore_ascii_case("software") || v.eq_ignore_ascii_case("sw"))
}

/// Start a new Vivi with the same arguments and `VIVI_RENDERER=software`, after the
/// OpenGL window could not be created. Returns false when it already ran in software
/// mode or the new process could not start.
pub(crate) fn relaunch_software(why: &str) -> bool {
    if software_forced() {
        return false;
    }
    crate::app::fatal::note(&format!(
        "OpenGL rendering failed ({why}); restarting with the software renderer"
    ));
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("VIVI_RENDERER", "software")
        .spawn()
        .is_ok()
}

/// Per-feature setup (each wires its own globals and callbacks).
pub(crate) fn setup_features(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &crate::app::state::AppState,
    log: bool,
) {
    settings::setup(app, player, cfg.clone(), log);
    crate::homeview::setup(app, cfg.clone());
    libview::setup(app, player, cfg.clone(), log);
    menus::setup(app, player, cfg.clone());
    crate::nowplaying::setup(player);
    desktop::setup(app, player, log);
    mini::setup(app, cfg.clone());
    viz::setup(app, player);
    plugins::setup(app, player, cfg.clone());
    crate::extwin::setup(app, cfg.clone());
    crate::extwin::refresh();
    crate::streams::setup(app, player);
    crate::palette::setup(app);
    crate::homeview::repack();
    crate::drawer::setup(app);
    crate::downloads::setup(app, player, cfg.clone());
    fx::setup(app, player);
    dialogs::setup(app, player);
    ipc::sweep_stale();
    ipc::setup(app, player, cfg.config().general.remote_control);
    crate::bookmarks::setup(app, player, log);
    crate::sleep::setup(app, player, cfg.clone());
    crate::transitions::setup(player, cfg.clone(), log);
    crate::status::setup(app, player);
    crate::tray::setup(app, player, cfg.clone());
    if screenshot_mode() {
        debug_ui(app);
    }
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
