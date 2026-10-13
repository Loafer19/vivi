//! Miniplayer: a compact, always-on-top window mode (Figma "Miniplayer": 391x339,
//! 391x690 with the queue). The full-size geometry is restored when leaving it.

use crate::player::Player;
use crate::AppWindow;
use slint::winit_030::{winit::window::WindowLevel, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalSize};
use std::cell::Cell;
use std::rc::Rc;

const W: f32 = 391.0;
const H: f32 = 339.0;
const H_QUEUE: f32 = 690.0;

pub fn setup(app: &AppWindow, player: &Rc<Player>) {
    let saved: Rc<Cell<Option<LogicalSize>>> = Rc::new(Cell::new(None));

    let apply = {
        let p = player.clone();
        move |a: &AppWindow| {
            let (mini, queue) = (a.get_mini(), a.get_mini_queue());
            let h = if queue { H_QUEUE } else { H };
            if mini {
                a.window().set_size(LogicalSize::new(W, h));
            }
            // Keep the video in the top 339px while the queue is shown below it.
            let margin = if mini && queue {
                (H_QUEUE - H) / H_QUEUE
            } else {
                0.0
            };
            p.set("video-margin-ratio-bottom", &format!("{margin:.4}"));
            a.window().with_winit_window(|w| {
                w.set_window_level(if mini {
                    WindowLevel::AlwaysOnTop
                } else {
                    WindowLevel::Normal
                });
            });
        }
    };

    let weak = app.as_weak();
    let s = saved.clone();
    let ap = apply.clone();
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
            s.set(Some(w.size().to_logical(w.scale_factor())));
            a.set_panel_open(false);
            a.set_settings_open(false);
            a.set_library_open(false);
        }
        a.set_mini(mini);
        ap(&a);
        if !mini {
            if let Some(size) = s.take() {
                w.set_size(size);
            }
        }
    });

    let weak = app.as_weak();
    app.on_toggle_mini_queue(move || {
        let Some(a) = weak.upgrade() else { return };
        a.set_mini_queue(!a.get_mini_queue());
        apply(&a);
    });
}
