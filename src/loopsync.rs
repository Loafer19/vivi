//! Vivi's repeat and shuffle buttons follow mpv, whoever changes it (mpv keys, scripts,
//! the Remote Control phone page, IPC).
//!
//! * Repeat: `loop-file` is observed; the repeat button (`Playback.repeat-on`) is on while
//!   it is not `no`. `loop-playlist` is observed too (Settings → Playback → Repeat "All").
//! * Shuffle: mpv has no shuffle state, only the `playlist-shuffle` / `playlist-unshuffle`
//!   commands, so Vivi publishes it as `user-data/vivi/shuffle` (`yes` / `no`). Writing
//!   that property asks Vivi to shuffle or unshuffle the queue, and the button follows.
//!   Packages check that the property exists before offering repeat / shuffle.

use crate::player::Player;
use crate::AppWindow;
use slint::ComponentHandle;

/// Shuffle state published for packages (and the request they write).
pub const SHUFFLE: &str = "user-data/vivi/shuffle";

fn flag(on: bool) -> &'static str {
    if on {
        "yes"
    } else {
        "no"
    }
}

/// `loop-file` value → repeat button state (`inf`, a count or `yes` are on).
pub fn looping(v: Option<&str>) -> bool {
    v.is_some_and(|v| !v.is_empty() && v != "no" && v != "0")
}

/// Startup: publish the shuffle state (also the capability check for packages).
pub fn setup(a: &AppWindow, p: &Player) {
    p.set(
        SHUFFLE,
        flag(a.global::<crate::Playback>().get_shuffle_on()),
    );
}

/// Shuffle or unshuffle the queue (when it changes), update the button and publish it.
pub fn set_shuffle(a: &AppWindow, p: &Player, on: bool) {
    if on != a.global::<crate::Playback>().get_shuffle_on() {
        p.run(if on {
            "playlist-shuffle"
        } else {
            "playlist-unshuffle"
        });
        a.global::<crate::Playback>().set_shuffle_on(on);
    }
    if p.get(SHUFFLE).as_deref() != Some(flag(on)) {
        p.set(SHUFFLE, flag(on));
    }
}

/// `loop-file`, `loop-playlist` or `user-data/vivi/shuffle` changed.
pub fn sync(a: &AppWindow, p: &Player) {
    let pb = a.global::<crate::Playback>();
    let rep = looping(p.get("loop-file").as_deref());
    if rep != pb.get_repeat_on() {
        pb.set_repeat_on(rep);
    }
    match p.get(SHUFFLE).as_deref() {
        Some("yes") => set_shuffle(a, p, true),
        Some("no") => set_shuffle(a, p, false),
        // Unset or garbage: put the real state back.
        _ => p.set(SHUFFLE, flag(pb.get_shuffle_on())),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn loop_file_values() {
        use super::looping;
        assert!(looping(Some("inf")) && looping(Some("yes")) && looping(Some("3")));
        assert!(!looping(Some("no")) && !looping(Some("0")) && !looping(None));
        assert_eq!((super::flag(true), super::flag(false)), ("yes", "no"));
    }
}
