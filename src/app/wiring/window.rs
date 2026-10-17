//! Raw winit window events: drag & drop (open or enqueue; dialogs can take a drop)
//! and the physical key of each key press for layout-independent forwarding.
//! winit delivers DroppedFile on X11/Windows/macOS; not on Wayland yet.

use crate::*;
use slint::ComponentHandle;
use std::rc::Rc;

pub(crate) fn wire(app: &AppWindow, player: &Rc<Player>) {
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
