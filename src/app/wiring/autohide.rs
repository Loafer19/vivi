//! Auto-hide of controls / cursor after inactivity while playing, and the subtitle
//! margin that keeps subtitles above the bottom bar.

use crate::*;
use slint::ComponentHandle;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The returned timer drives the auto-hide; keep it alive for the app's lifetime.
#[must_use]
pub(crate) fn wire(app: &AppWindow, player: &Rc<Player>) -> slint::Timer {
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
    hide_timer
}
