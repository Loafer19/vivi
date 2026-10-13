//! Miniplayer: a compact, always-on-top window mode (Figma "Miniplayer": 391x339,
//! plus a 351px queue view below). It is freely resizable down to 280x160; the UI adapts
//! (see `ui/app.slint`) and mpv letterboxes, so the video keeps its aspect ratio. The
//! miniplayer geometry is saved in the config and the full-size geometry restored on exit.

use crate::config::Config;
use crate::AppWindow;
use slint::winit_030::{winit::window::WindowLevel, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const W: f32 = 391.0;
const H: f32 = 339.0;
/// Height of the queue view below the video (Figma: 690 - 339).
pub const QUEUE_H: f32 = 351.0;

type Geometry = (LogicalPosition, LogicalSize);

fn geometry(a: &AppWindow) -> Geometry {
    let w = a.window();
    let sf = w.scale_factor();
    (w.position().to_logical(sf), w.size().to_logical(sf))
}

fn set_on_top(a: &AppWindow, on: bool) {
    a.window().with_winit_window(|w| {
        w.set_window_level(if on {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        });
    });
}

pub fn setup(app: &AppWindow, cfg: Rc<RefCell<Config>>) {
    let full: Rc<Cell<Option<Geometry>>> = Rc::new(Cell::new(None));

    // Remember the miniplayer geometry (video part only) in the config.
    let remember = {
        let cfg = cfg.clone();
        move |a: &AppWindow| {
            let (pos, size) = geometry(a);
            let h = size.height - if a.get_mini_queue() { QUEUE_H } else { 0.0 };
            let g = [
                pos.x as i32,
                pos.y as i32,
                size.width as i32,
                h.max(160.0) as i32,
            ];
            if cfg.borrow().interface.mini_window != Some(g) {
                cfg.borrow_mut().interface.mini_window = Some(g);
                let _ = cfg.borrow().save();
            }
        }
    };

    let weak = app.as_weak();
    let r = remember.clone();
    app.on_toggle_mini(move || {
        let Some(a) = weak.upgrade() else { return };
        let w = a.window();
        let mini = !a.get_mini();
        if mini {
            if w.is_fullscreen() {
                w.set_fullscreen(false);
                a.set_fullscreen(false);
            }
            if w.is_maximized() {
                w.set_maximized(false);
            }
            full.set(Some(geometry(&a)));
            a.set_panel_open(false);
            a.set_settings_open(false);
            a.set_library_open(false);
            a.set_mini(true);
            let saved = cfg.borrow().interface.mini_window;
            let (mw, mh) = saved.map_or((W, H), |g| (g[2] as f32, g[3] as f32));
            let extra = if a.get_mini_queue() { QUEUE_H } else { 0.0 };
            w.set_size(LogicalSize::new(mw.max(280.0), mh.max(160.0) + extra));
            if let Some(g) = saved {
                w.set_position(LogicalPosition::new(g[0] as f32, g[1] as f32));
            }
            set_on_top(&a, true);
        } else {
            r(&a);
            a.set_mini(false);
            set_on_top(&a, false);
            if let Some((pos, size)) = full.take() {
                w.set_size(size);
                w.set_position(pos);
            }
        }
    });

    let weak = app.as_weak();
    app.on_toggle_mini_queue(move || {
        let Some(a) = weak.upgrade() else { return };
        let show = !a.get_mini_queue();
        let (_, size) = geometry(&a);
        a.set_mini_queue(show);
        let h = size.height + if show { QUEUE_H } else { -QUEUE_H };
        a.window()
            .set_size(LogicalSize::new(size.width, h.max(160.0)));
    });

    // Save geometry after the user moves/resizes the miniplayer (debounced).
    let weak = app.as_weak();
    let t = Box::leak(Box::new(slint::Timer::default()));
    let last = Cell::new(None::<(i32, i32, i32, i32)>);
    t.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(2),
        move || {
            let Some(a) = weak.upgrade() else { return };
            if !a.get_mini() {
                return;
            }
            let (p, s) = geometry(&a);
            let g = (p.x as i32, p.y as i32, s.width as i32, s.height as i32);
            if last.replace(Some(g)) == Some(g) {
                remember(&a);
            }
        },
    );
}
