//! The mouse on the video (Settings → Controls → Mouse, #92): what each button and the
//! scroll wheel do. The window reports `Playback.mouse(button)` / `Playback.wheel(delta)`;
//! the mapping from the settings to an action is pure ([`button_action`], [`resolve`],
//! [`wheel`]) and unit-tested.

use crate::config::Controls;
use crate::*;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;

/// Button actions as (stored id, label).
pub const ACTIONS: [(&str, &str); 10] = [
    ("play-pause", "Play / pause"),
    ("mute", "Mute"),
    ("fullscreen", "Fullscreen"),
    ("next", "Next in queue"),
    ("previous", "Previous in queue"),
    ("skip-forward", "Short skip forward"),
    ("skip-back", "Short skip backward"),
    ("long-skip-forward", "Long skip forward"),
    ("long-skip-back", "Long skip backward"),
    ("none", "Nothing"),
];

/// Scroll wheel modes as (stored id, label).
pub const WHEEL: [(&str, &str); 4] = [
    ("volume", "Volume"),
    ("speed", "Speed"),
    ("seek", "Seek"),
    ("none", "Nothing"),
];

/// Buttons reported by the window, with their setting id and row title.
pub const BUTTONS: [(&str, &str, &str); 5] = [
    ("click", "controls.mouse_click", "Left click"),
    (
        "double-click",
        "controls.mouse_double_click",
        "Left double-click",
    ),
    ("middle", "controls.mouse_middle", "Middle click"),
    ("back", "controls.mouse_back", "Back button (4)"),
    ("forward", "controls.mouse_forward", "Forward button (5)"),
];

/// The id of a stored id or a label (any case), if it is one of `table`.
pub fn id_of(table: &[(&'static str, &'static str)], v: &str) -> Option<&'static str> {
    let v = v.trim();
    table
        .iter()
        .find(|(id, label)| id.eq_ignore_ascii_case(v) || label.eq_ignore_ascii_case(v))
        .map(|(id, _)| *id)
}

/// The label of an id (the last entry, "Nothing", when unknown).
pub fn label_of(table: &[(&'static str, &'static str)], id: &str) -> &'static str {
    let found = id_of(table, id).unwrap_or("none");
    table
        .iter()
        .find(|(i, _)| *i == found)
        .map(|(_, l)| *l)
        .unwrap_or("Nothing")
}

/// The stored action of a button as reported by the window.
pub fn button_action<'a>(c: &'a Controls, button: &str) -> &'a str {
    match button {
        "click" => &c.mouse_click,
        "double-click" => &c.mouse_double_click,
        "middle" => &c.mouse_middle,
        "back" => &c.mouse_back,
        "forward" => &c.mouse_forward,
        _ => "none",
    }
}

/// What a button or wheel turn does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Do {
    PlayPause,
    Mute,
    Fullscreen,
    /// Next (`true`) or previous queue item.
    Queue(bool),
    /// Relative seek in seconds.
    Seek(f64),
    /// Relative volume change in percent.
    Volume(f64),
    /// Relative speed change.
    Speed(f64),
    Nothing,
}

/// A button action id with the short and long skip in seconds.
pub fn resolve(action: &str, short: f64, long: f64) -> Do {
    match id_of(&ACTIONS, action).unwrap_or("none") {
        "play-pause" => Do::PlayPause,
        "mute" => Do::Mute,
        "fullscreen" => Do::Fullscreen,
        "next" => Do::Queue(true),
        "previous" => Do::Queue(false),
        "skip-forward" => Do::Seek(short),
        "skip-back" => Do::Seek(-short),
        "long-skip-forward" => Do::Seek(long),
        "long-skip-back" => Do::Seek(-long),
        _ => Do::Nothing,
    }
}

/// One wheel notch (`up` = away from the user) with the short skip (s) and volume step (%).
pub fn wheel(c: &Controls, up: bool, short: f64, volume_step: f64) -> Do {
    let sign = if up != c.wheel_reverse { 1.0 } else { -1.0 };
    match id_of(&WHEEL, &c.wheel).unwrap_or("none") {
        "volume" => Do::Volume(sign * volume_step),
        "speed" => Do::Speed(sign * c.speed_step),
        "seek" => Do::Seek(sign * short),
        _ => Do::Nothing,
    }
}

/// The new speed after a step: kept within 0.25×–4× and to two decimals, so repeated
/// 0.05 / 0.1 steps land on round values.
pub fn speed_after(current: f64, step: f64) -> f64 {
    (((current + step) * 100.0).round() / 100.0).clamp(0.25, 4.0)
}

thread_local! {
    static CONTROLS: RefCell<Controls> = RefCell::new(Controls::default());
}

/// Use these settings from now on (called whenever the config is applied).
pub fn configure(c: &Controls) {
    CONTROLS.with(|s| *s.borrow_mut() = c.clone());
}

fn steps() -> (f64, f64, f64) {
    let n = |a: &std::sync::atomic::AtomicU32| a.load(Ordering::Relaxed) as f64;
    (
        n(&settings::SHORT_SKIP),
        n(&settings::LONG_SKIP),
        n(&settings::VOLUME_STEP),
    )
}

/// Run an action; `false` when it does nothing (the event is left to others).
fn run(a: &AppWindow, p: &Player, d: Do) -> bool {
    let pb = a.global::<crate::Playback>();
    match d {
        Do::Nothing => return false,
        // Fullscreen works without media, as double-click always did.
        Do::Fullscreen => pb.invoke_toggle_fullscreen(),
        _ if !pb.get_has_media() => return false,
        Do::PlayPause => pb.invoke_toggle_play(),
        Do::Mute => pb.invoke_toggle_mute(),
        Do::Queue(true) => pb.invoke_next(),
        Do::Queue(false) => pb.invoke_prev(),
        Do::Seek(s) => p.seek_rel(s),
        Do::Volume(v) => p.add_volume(v),
        Do::Speed(step) => {
            let cur = p.get("speed").and_then(|s| s.parse().ok()).unwrap_or(1.0);
            p.set("speed", &speed_after(cur, step).to_string());
        }
    }
    true
}

/// Handle the window's `Playback.mouse` / `Playback.wheel` callbacks.
pub(crate) fn wire(app: &AppWindow, player: &Rc<Player>) {
    let w = app.as_weak();
    let p = player.clone();
    app.global::<crate::Playback>().on_mouse(move |button| {
        let Some(a) = w.upgrade() else { return };
        let (short, long, _) = steps();
        let d = CONTROLS.with(|c| resolve(button_action(&c.borrow(), &button), short, long));
        run(&a, &p, d);
    });
    let w = app.as_weak();
    let p = player.clone();
    app.global::<crate::Playback>().on_wheel(move |dy| {
        let Some(a) = w.upgrade() else { return false };
        if dy == 0.0 {
            return false;
        }
        let (short, _, vstep) = steps();
        let d = CONTROLS.with(|c| wheel(&c.borrow(), dy > 0.0, short, vstep));
        run(&a, &p, d)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_old_mouse() {
        let c = Controls::default();
        assert_eq!(
            resolve(button_action(&c, "click"), 5.0, 60.0),
            Do::PlayPause
        );
        assert_eq!(
            resolve(button_action(&c, "double-click"), 5.0, 60.0),
            Do::Fullscreen
        );
        for b in ["middle", "back", "forward", "right", ""] {
            assert_eq!(resolve(button_action(&c, b), 5.0, 60.0), Do::Nothing, "{b}");
        }
        assert_eq!(wheel(&c, true, 5.0, 5.0), Do::Nothing);
        assert_eq!(wheel(&c, false, 5.0, 5.0), Do::Nothing);
    }

    #[test]
    fn every_action_dispatches() {
        let want = [
            Do::PlayPause,
            Do::Mute,
            Do::Fullscreen,
            Do::Queue(true),
            Do::Queue(false),
            Do::Seek(5.0),
            Do::Seek(-5.0),
            Do::Seek(60.0),
            Do::Seek(-60.0),
            Do::Nothing,
        ];
        for ((id, label), d) in ACTIONS.iter().zip(want) {
            assert_eq!(resolve(id, 5.0, 60.0), d, "{id}");
            assert_eq!(resolve(label, 5.0, 60.0), d, "{label}");
        }
        assert_eq!(resolve("bogus", 5.0, 60.0), Do::Nothing);
        let c = Controls {
            mouse_middle: "mute".into(),
            mouse_back: "skip-back".into(),
            mouse_forward: "next".into(),
            ..Default::default()
        };
        assert_eq!(resolve(button_action(&c, "middle"), 5.0, 60.0), Do::Mute);
        assert_eq!(
            resolve(button_action(&c, "back"), 10.0, 60.0),
            Do::Seek(-10.0)
        );
        assert_eq!(
            resolve(button_action(&c, "forward"), 5.0, 60.0),
            Do::Queue(true)
        );
    }

    #[test]
    fn wheel_modes_and_reverse() {
        let mut c = Controls {
            wheel: "volume".into(),
            ..Default::default()
        };
        assert_eq!(wheel(&c, true, 5.0, 2.0), Do::Volume(2.0));
        assert_eq!(wheel(&c, false, 5.0, 2.0), Do::Volume(-2.0));
        c.wheel = "seek".into();
        assert_eq!(wheel(&c, true, 10.0, 2.0), Do::Seek(10.0));
        c.wheel = "speed".into();
        c.speed_step = 0.25;
        assert_eq!(wheel(&c, true, 5.0, 2.0), Do::Speed(0.25));
        c.wheel_reverse = true;
        assert_eq!(wheel(&c, true, 5.0, 2.0), Do::Speed(-0.25));
        assert_eq!(wheel(&c, false, 5.0, 2.0), Do::Speed(0.25));
    }

    #[test]
    fn speed_steps_stay_round_and_in_range() {
        assert_eq!(speed_after(1.0, 0.05), 1.05);
        let mut s = 1.0;
        for _ in 0..3 {
            s = speed_after(s, 0.1);
        }
        assert_eq!(s, 1.3);
        assert_eq!(speed_after(0.3, -0.25), 0.25);
        assert_eq!(speed_after(3.9, 0.25), 4.0);
    }

    #[test]
    fn labels_and_ids() {
        assert_eq!(label_of(&ACTIONS, "play-pause"), "Play / pause");
        assert_eq!(label_of(&ACTIONS, "bogus"), "Nothing");
        assert_eq!(id_of(&WHEEL, "Volume"), Some("volume"));
        assert_eq!(id_of(&WHEEL, "seek"), Some("seek"));
        assert_eq!(id_of(&WHEEL, "Scroll"), None);
        // Every button the window reports has a setting.
        let c = Controls {
            mouse_click: "mute".into(),
            mouse_double_click: "mute".into(),
            mouse_middle: "mute".into(),
            mouse_back: "mute".into(),
            mouse_forward: "mute".into(),
            ..Default::default()
        };
        for (b, _, _) in BUTTONS {
            assert_eq!(button_action(&c, b), "mute", "{b}");
        }
    }
}
