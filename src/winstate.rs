//! Main window placement (#97, Settings → Interface → Window): remember the size and
//! position, keep the window above others, resize it to the video.
//!
//! Wayland lets compositors place windows and has no always-on-top for apps, so there
//! only the size is remembered and *Keep window above* has no effect; tiling window
//! managers also ignore size requests for tiled windows.

use crate::config::{self, Config};
use crate::player::Player;
use crate::AppWindow;
use slint::winit_030::{winit::window::WindowLevel, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, PhysicalSize};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};

/// Whether the window can read its position: 0 = not known yet, 1 = yes, 2 = no.
static POSITION: AtomicU8 = AtomicU8::new(0);
/// A video was fitted this session (*Resize window to fit video: First video only*).
static FITTED: AtomicBool = AtomicBool::new(false);

/// Windows cannot place themselves or stay on top (Wayland). Known for sure once the
/// window is shown ([`shown`]); before that, from the environment.
pub fn wayland() -> bool {
    match POSITION.load(Relaxed) {
        1 => false,
        2 => true,
        _ => cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

/// Smallest remembered size that is restored (anything smaller was a glitch).
const MIN: [i32; 2] = [480, 270];

/// Before the window is shown: its remembered size.
pub fn restore(app: &AppWindow, c: &Config) {
    if !c.stub(config::REMEMBER_WINDOW) {
        return;
    }
    if let Some([_, _, w, h]) = c.interface.window {
        if w >= MIN[0] && h >= MIN[1] {
            app.window().set_size(LogicalSize::new(w as f32, h as f32));
        }
    }
}

/// After the window is shown: probe what the platform allows, then restore the
/// remembered position (if it is on a connected screen) and apply *Keep window above*.
pub fn shown(app: &AppWindow, c: &Config) {
    let probe = app
        .window()
        .with_winit_window(|w| w.outer_position().is_ok());
    if let Some(ok) = probe {
        POSITION.store(if ok { 1 } else { 2 }, Relaxed);
    }
    if c.stub(config::REMEMBER_WINDOW) && !wayland() {
        if let Some([x, y, ..]) = c.interface.window {
            let sf = app.window().scale_factor();
            let on_screen = app.window().with_winit_window(|w| {
                let (px, py) = ((x as f32 * sf) as i32, (y as f32 * sf) as i32);
                w.available_monitors().any(|m| {
                    let (p, s) = (m.position(), m.size());
                    px >= p.x
                        && py >= p.y
                        && px < p.x + s.width as i32 - 64
                        && py < p.y + s.height as i32 - 64
                })
            });
            if on_screen == Some(true) {
                app.window()
                    .set_position(LogicalPosition::new(x as f32, y as f32));
            }
        }
    }
    if c.stub(config::ON_TOP) {
        apply_on_top(app, true);
    }
}

/// Keep the window above others or not (no effect on Wayland). The miniplayer is always
/// on top; leaving it restores the setting.
pub fn apply_on_top(app: &AppWindow, on: bool) {
    app.window().with_winit_window(|w| {
        w.set_window_level(if on {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        });
    });
}

/// On exit: remember the size (and position where the platform reports it) of the
/// normal window; full screen, maximized and the miniplayer keep the last one.
/// True when the config changed.
pub fn save(app: &AppWindow, c: &mut Config) -> bool {
    let w = app.window();
    if !c.stub(config::REMEMBER_WINDOW)
        || app.get_mini()
        || w.is_fullscreen()
        || w.is_maximized()
        || w.is_minimized()
    {
        return false;
    }
    let sf = w.scale_factor();
    let size = w.size().to_logical(sf);
    let [mut x, mut y] = c.interface.window.map_or([0, 0], |g| [g[0], g[1]]);
    if !wayland() {
        let p = w.position().to_logical(sf);
        (x, y) = (p.x as i32, p.y as i32);
    }
    let g = [x, y, size.width as i32, size.height as i32];
    if g[2] < MIN[0] || g[3] < MIN[1] || c.interface.window == Some(g) {
        return false;
    }
    c.interface.window = Some(g);
    true
}

/// Playback (re)started after a file loaded: size the window to the video
/// (*Resize window to fit video*), at most 90 % of the screen.
pub fn fit_video(app: &AppWindow, p: &Player, c: &Config) {
    let mode = c.fit();
    if mode == config::FIT_CHOICES[0]
        || (mode == config::FIT_CHOICES[2] && FITTED.load(Relaxed))
        || crate::viz::audio_only(p)
    {
        return;
    }
    let w = app.window();
    if app.get_mini() || app.get_fullscreen() || w.is_fullscreen() || w.is_maximized() {
        return;
    }
    let (Ok(vw), Ok(vh)) = (
        p.mpv.get_property::<i64>("dwidth"),
        p.mpv.get_property::<i64>("dheight"),
    ) else {
        return;
    };
    if vw <= 0 || vh <= 0 {
        return;
    }
    FITTED.store(true, Relaxed);
    let screen = w
        .with_winit_window(|ww| ww.current_monitor().map(|m| m.size()))
        .flatten();
    let (vw, vh) = (vw as f64, vh as f64);
    let k = screen.map_or(1.0, |s| {
        (s.width as f64 * 0.9 / vw)
            .min(s.height as f64 * 0.9 / vh)
            .min(1.0)
    });
    w.set_size(PhysicalSize::new(
        (vw * k).round() as u32,
        (vh * k).round() as u32,
    ));
}
