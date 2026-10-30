//! Auto-hide of the bars and the cursor after inactivity (Settings → Interface →
//! Auto-hide, #98), and the subtitle margin that keeps subtitles above the bottom bar.
//!
//! Rust decides when the controls are awake (`controls-visible`); the `AutoHide` global
//! tells the window which of the top bar, bottom bar and pointer actually hide, whether
//! open panels or a hovered bar keep them shown, and how they animate.

use crate::*;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Pointer movement (logical px) ignored by the "nudge" wake mode.
const NUDGE_PX: f32 = 12.0;
/// How long seek and volume keys show the controls in the "seek" key mode.
const BRIEF: Duration = Duration::from_millis(1500);
/// Height of the edge zones (logical px) of the "edge" wake mode.
const EDGE_PX: f32 = 120.0;

#[derive(Clone)]
struct Cfg {
    delay: Duration,
    /// "delay" | "fullscreen" | "never"
    mode: String,
    keep_paused: bool,
    keep_audio: bool,
    /// "move" | "nudge" | "edge"
    wake: String,
    /// "any" | "seek" | "none"
    keys: String,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            delay: Duration::from_secs(2),
            mode: "delay".into(),
            keep_paused: true,
            keep_audio: false,
            wake: "move".into(),
            keys: "any".into(),
        }
    }
}

thread_local! {
    static CFG: RefCell<Cfg> = RefCell::new(Cfg::default());
}

/// The delay in seconds: 0.5–30 in half seconds; anything unreadable is 2 s.
pub(crate) fn delay_secs(v: f64) -> f64 {
    if v.is_finite() && v > 0.0 {
        ((v * 2.0).round() / 2.0).clamp(0.5, 30.0)
    } else {
        2.0
    }
}

/// Apply Settings → Interface → Auto-hide.
pub(crate) fn configure(a: &AppWindow, i: &config::Interface) {
    CFG.with(|c| {
        *c.borrow_mut() = Cfg {
            delay: Duration::from_secs_f64(delay_secs(i.autohide_secs)),
            mode: i.autohide_mode.clone(),
            keep_paused: i.autohide_keep_paused,
            keep_audio: i.autohide_keep_audio,
            wake: i.autohide_wake.clone(),
            keys: i.autohide_keys.clone(),
        }
    });
    let g = a.global::<crate::AutoHide>();
    g.set_top(i.autohide_top);
    g.set_bottom(i.autohide_bottom);
    g.set_cursor(i.autohide_cursor);
    g.set_keep_panel(i.autohide_keep_panel);
    g.set_keep_hover(i.autohide_keep_hover);
    g.set_anim(match i.autohide_anim.as_str() {
        "fade" => 1,
        "slide" => 2,
        _ => 0,
    });
}

/// The returned timer drives the auto-hide; keep it alive for the app's lifetime.
#[must_use]
pub(crate) fn wire(app: &AppWindow, player: &Rc<Player>) -> slint::Timer {
    // When the controls were last woken; `brief` shortens the next idle wait.
    let last = Rc::new(Cell::new(Instant::now()));
    let brief = Rc::new(Cell::new(false));
    // Where the pointer last was while the controls were shown (the "nudge" mode
    // measures from it).
    let anchor = Rc::new(Cell::new((f32::NAN, f32::NAN)));
    let wake = {
        let last = last.clone();
        let brief = brief.clone();
        let weak = app.as_weak();
        Rc::new(move |short: bool| {
            let Some(a) = weak.upgrade() else { return };
            let hidden = !a.get_controls_visible();
            last.set(Instant::now());
            // A seek key only shows hidden controls briefly; it does not cut short
            // controls the mouse woke.
            brief.set(short && (hidden || brief.get()));
            if hidden {
                a.set_controls_visible(true);
            }
        })
    };
    {
        let wake = wake.clone();
        app.on_activity(move || wake(false));
    }
    {
        let wake = wake.clone();
        let weak = app.as_weak();
        app.on_pointer_moved(move |x, y| {
            let Some(a) = weak.upgrade() else { return };
            let shown = a.get_controls_visible();
            let (ax, ay) = anchor.get();
            if shown || ax.is_nan() {
                anchor.set((x, y));
            }
            if shown {
                wake(false);
                return;
            }
            let ok = match CFG.with(|c| c.borrow().wake.clone()).as_str() {
                "nudge" => (x - ax).hypot(y - ay) >= NUDGE_PX,
                "edge" => {
                    let h = a
                        .window()
                        .size()
                        .to_logical(a.window().scale_factor())
                        .height;
                    y <= EDGE_PX || y >= h - EDGE_PX
                }
                _ => true,
            };
            if ok {
                anchor.set((x, y));
                wake(false);
            }
        });
    }
    {
        let wake = wake.clone();
        app.on_key_activity(move |text, shift, ctrl, alt| {
            match CFG.with(|c| c.borrow().keys.clone()).as_str() {
                "none" => {}
                "seek" => {
                    use keymap::Action as A;
                    if matches!(
                        keymap::lookup(&text, shift, ctrl, alt),
                        Some(A::SeekForward | A::SeekBack | A::VolumeUp | A::VolumeDown)
                    ) {
                        wake(true);
                    }
                }
                _ => wake(false),
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
                let cfg = CFG.with(|c| c.borrow().clone());
                let wait = if brief.get() {
                    cfg.delay.min(BRIEF)
                } else {
                    cfg.delay
                };
                let pb = a.global::<crate::Playback>();
                let hideable = match cfg.mode.as_str() {
                    "never" => false,
                    "fullscreen" => a.get_fullscreen(),
                    _ => true,
                } && pb.get_has_media()
                    && (pb.get_playing() || !cfg.keep_paused);
                let want = !(hideable
                    && last.get().elapsed() > wait
                    && !(cfg.keep_audio && crate::viz::audio_only(&hp)));
                if a.get_controls_visible() != want {
                    a.set_controls_visible(want);
                }
                // Lift subtitles above the bottom bar while it is shown.
                let bar = want
                    || !a.global::<crate::AutoHide>().get_bottom()
                    || a.global::<crate::Overlays>().get_panel_open();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_is_half_seconds_within_range() {
        assert_eq!(delay_secs(2.0), 2.0);
        assert_eq!(delay_secs(0.2), 0.5);
        assert_eq!(delay_secs(0.74), 0.5);
        assert_eq!(delay_secs(0.76), 1.0);
        assert_eq!(delay_secs(99.0), 30.0);
        assert_eq!(delay_secs(0.0), 2.0);
        assert_eq!(delay_secs(f64::NAN), 2.0);
    }
}
