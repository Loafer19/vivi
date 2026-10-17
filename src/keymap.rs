//! The one keymap: every player shortcut, in match order.
//!
//! [`BINDINGS`] drives both the key handler (`main.rs`, via [`lookup`]) and every place
//! that lists shortcuts: Settings → Controls → Keyboard Shortcuts, the `?` overlay, the
//! onboarding page and the menu hints ([`hint`]). `docs/features.md` is checked against
//! [`markdown_table`] by a test, so the docs cannot drift either.
//!
//! Keys that match no binding go to mpv (`forward_key`), so mpv's own defaults and the
//! key bindings of plugin packages keep working.

use slint::platform::Key;

/// What a shortcut does. Parameters (seek step, offset size, direction) are worked out
/// by the handler from the key text and Shift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    PlayPause,
    SeekForward,
    SeekBack,
    VolumeUp,
    VolumeDown,
    Fullscreen,
    Miniplayer,
    Mute,
    Stop,
    /// N next, Shift+N previous.
    QueueStep,
    AbLoop,
    Karaoke,
    FindLyrics,
    /// `[` `]` (Shift or `{` `}`: 0.5 s).
    LyricsOffset,
    /// `z` `x` (Shift: 1 s).
    SubtitleOffset,
    Record,
    HideUi,
    Quit,
    OpenFile,
    Settings,
    Bookmark,
    /// Audio-only files with the visualizer on; otherwise the key goes to mpv.
    Visualizer,
    Shortcuts,
    Escape,
}

/// A key as Slint reports it: the produced text, or a named key.
#[derive(Clone, Copy, Debug)]
pub enum K {
    Text(&'static str),
    Named(Key),
}

/// Whether a modifier must be held, must not be held, or does not matter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mod {
    Any,
    On,
    Off,
}

impl Mod {
    fn ok(self, held: bool) -> bool {
        match self {
            Mod::Any => true,
            Mod::On => held,
            Mod::Off => !held,
        }
    }
}

pub struct Binding {
    pub action: Action,
    pub keys: &'static [K],
    pub ctrl: Mod,
    pub shift: Mod,
    /// Shortcut list label.
    pub label: &'static str,
    /// Keys as shown in the shortcut list.
    pub shown: &'static str,
}

const fn b(
    action: Action,
    keys: &'static [K],
    ctrl: Mod,
    shift: Mod,
    label: &'static str,
    shown: &'static str,
) -> Binding {
    Binding {
        action,
        keys,
        ctrl,
        shift,
        label,
        shown,
    }
}

use Action as A;
use Mod::{Any, Off, On};
use K::{Named as N, Text as T};

/// All shortcuts. The first match wins, so order matters (Shift+L before L).
/// Alt combinations are not here: they belong to mpv scripts and plugin packages.
#[rustfmt::skip]
pub const BINDINGS: &[Binding] = &[
    b(A::PlayPause, &[T(" "), T("k")], Any, Any, "Play / pause", "Space  K"),
    b(A::SeekForward, &[N(Key::RightArrow)], Any, Any, "Seek forward (Shift: long skip)", "→"),
    b(A::SeekBack, &[N(Key::LeftArrow)], Any, Any, "Seek back (Shift: long skip)", "←"),
    b(A::VolumeUp, &[N(Key::UpArrow)], Any, Any, "Volume up", "↑"),
    b(A::VolumeDown, &[N(Key::DownArrow)], Any, Any, "Volume down", "↓"),
    b(A::Fullscreen, &[T("f")], Any, Any, "Fullscreen", "F"),
    b(A::Miniplayer, &[T("p")], Off, Any, "Miniplayer on / off", "P"),
    b(A::Mute, &[T("m")], Any, Any, "Mute", "M"),
    b(A::Stop, &[T("s"), T("S")], Off, Any, "Stop", "S"),
    b(A::QueueStep, &[T("n"), T("N")], Off, Any, "Next / previous in queue", "N  Shift+N"),
    b(A::AbLoop, &[T("l"), T("L")], Off, On, "A-B loop: set A / set B / clear", "Shift+L"),
    b(A::Karaoke, &[T("l"), T("L")], Off, Any, "Karaoke lyrics view", "L"),
    b(A::FindLyrics, &[T("l"), T("L")], On, Any, "Find lyrics", "Ctrl+L"),
    b(A::LyricsOffset, &[T("["), T("]"), T("{"), T("}")], Off, Any,
        "Lyrics timing offset ±0.1 s (Shift: 0.5 s)", "[  ]"),
    b(A::SubtitleOffset, &[T("z"), T("x"), T("Z"), T("X")], Off, Any,
        "Subtitle offset ±0.1 s (Shift: 1 s), per file", "Z  X"),
    b(A::Visualizer, &[T("v")], Off, Any, "Next visualizer mode (audio files)", "V"),
    b(A::Record, &[T("r"), T("R")], Off, Any, "Start / stop recording", "R"),
    b(A::HideUi, &[T("\\")], On, Any, "Hide / show the interface", "Ctrl+\\"),
    b(A::Quit, &[T("q")], On, Any, "Quit", "Ctrl+Q"),
    b(A::OpenFile, &[T("o")], On, Any, "Open file", "Ctrl+O"),
    b(A::Settings, &[T(",")], On, Any, "Settings", "Ctrl+,"),
    b(A::Bookmark, &[T("b")], On, Any, "Add bookmark", "Ctrl+B"),
    b(A::Shortcuts, &[T("?")], Off, Any, "Keyboard shortcuts", "?"),
    b(A::Escape, &[N(Key::Escape)], Any, Any,
        "Close the top dialog or panel, then leave miniplayer / fullscreen", "Esc"),
];

/// Mouse and drag & drop as (action, input), listed after the keys.
pub const MOUSE: &[(&str, &str)] = &[
    ("Fullscreen", "Double-click"),
    ("Play / pause", "Click"),
    ("Context menu", "Right-click"),
    ("Open files or add them to the queue", "Drop files"),
];

fn matches(k: &K, text: &str) -> bool {
    match k {
        K::Text(t) => *t == text,
        K::Named(n) => slint::SharedString::from(*n).as_str() == text,
    }
}

/// The binding for a key press (Slint key text plus Shift/Ctrl), if any.
pub fn lookup(text: &str, shift: bool, ctrl: bool) -> Option<Action> {
    BINDINGS
        .iter()
        .find(|b| b.ctrl.ok(ctrl) && b.shift.ok(shift) && b.keys.iter().any(|k| matches(k, text)))
        .map(|b| b.action)
}

fn binding(a: Action) -> &'static Binding {
    BINDINGS
        .iter()
        .find(|b| b.action == a)
        .expect("every action has a binding")
}

/// Keys of an action as shown in lists ("Ctrl+O").
pub fn shown(a: Action) -> &'static str {
    binding(a).shown
}

/// Menu hint style of an action's first key ("Ctrl + O", "Shift + N").
pub fn hint(a: Action) -> String {
    let first = binding(a).shown.split("  ").next().unwrap_or_default();
    if first.len() > 1 {
        first.replacen('+', " + ", 1)
    } else {
        first.to_string()
    }
}

/// Menu hint of the Shift variant of a two-way binding (N → "Shift + N").
pub fn hint_alt(a: Action) -> String {
    let alt = binding(a).shown.split("  ").nth(1).unwrap_or_default();
    alt.replacen('+', " + ", 1)
}

/// (label, keys) rows of every binding, in list order.
pub fn rows() -> impl Iterator<Item = (&'static str, &'static str)> {
    BINDINGS.iter().map(|b| (b.label, b.shown))
}

/// (label, keys) rows of a few actions (onboarding).
pub fn rows_of(actions: &[Action]) -> Vec<(&'static str, &'static str)> {
    actions
        .iter()
        .map(|a| {
            let b = binding(*a);
            (b.label, b.shown)
        })
        .collect()
}

/// The shortcut table of `docs/features.md` (checked by a test).
#[cfg(test)]
pub fn markdown_table() -> String {
    let cell = |s: &str| {
        s.split("  ")
            .map(|k| k.replace('\\', "\\\\").replace('|', "\\|"))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let mut t = String::from("| Key | Action |\n|---|---|\n");
    for (label, keys) in rows() {
        t += &format!("| {} | {} |\n", cell(keys), label);
    }
    for (label, how) in MOUSE {
        t += &format!("| {} | {} |\n", how, label);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key) -> String {
        slint::SharedString::from(k).to_string()
    }

    #[test]
    fn same_bindings_as_before() {
        // (text, shift, ctrl) → action, as the hand-written match did.
        let cases: &[(&str, bool, bool, Option<Action>)] = &[
            (" ", false, false, Some(A::PlayPause)),
            ("k", false, true, Some(A::PlayPause)),
            ("K", true, false, None),
            ("f", false, false, Some(A::Fullscreen)),
            ("F", true, false, None),
            ("p", false, false, Some(A::Miniplayer)),
            ("p", false, true, None),
            ("m", false, false, Some(A::Mute)),
            ("s", false, false, Some(A::Stop)),
            ("S", true, false, Some(A::Stop)),
            ("s", false, true, None),
            ("n", false, false, Some(A::QueueStep)),
            ("N", true, false, Some(A::QueueStep)),
            ("L", true, false, Some(A::AbLoop)),
            ("l", false, false, Some(A::Karaoke)),
            ("L", false, false, Some(A::Karaoke)),
            ("l", false, true, Some(A::FindLyrics)),
            ("L", true, true, Some(A::FindLyrics)),
            ("[", false, false, Some(A::LyricsOffset)),
            ("}", true, false, Some(A::LyricsOffset)),
            ("[", false, true, None),
            ("z", false, false, Some(A::SubtitleOffset)),
            ("X", true, false, Some(A::SubtitleOffset)),
            ("r", false, false, Some(A::Record)),
            ("r", false, true, None),
            ("\\", false, true, Some(A::HideUi)),
            ("\\", false, false, None),
            ("q", false, true, Some(A::Quit)),
            ("q", false, false, None),
            ("o", false, true, Some(A::OpenFile)),
            (",", false, true, Some(A::Settings)),
            ("b", false, true, Some(A::Bookmark)),
            ("v", false, false, Some(A::Visualizer)),
            ("v", false, true, None),
            ("?", true, false, Some(A::Shortcuts)),
            ("i", false, false, None),
            ("j", false, false, None),
        ];
        for (t, shift, ctrl, want) in cases {
            assert_eq!(
                lookup(t, *shift, *ctrl),
                *want,
                "{t:?} shift={shift} ctrl={ctrl}"
            );
        }
        assert_eq!(
            lookup(&key(Key::RightArrow), true, false),
            Some(A::SeekForward)
        );
        assert_eq!(
            lookup(&key(Key::LeftArrow), false, false),
            Some(A::SeekBack)
        );
        assert_eq!(lookup(&key(Key::UpArrow), false, false), Some(A::VolumeUp));
        assert_eq!(
            lookup(&key(Key::DownArrow), false, true),
            Some(A::VolumeDown)
        );
        assert_eq!(lookup(&key(Key::Escape), false, false), Some(A::Escape));
    }

    #[test]
    fn one_binding_per_action_with_a_label() {
        for (i, b) in BINDINGS.iter().enumerate() {
            assert!(!b.label.is_empty() && !b.shown.is_empty());
            assert!(
                BINDINGS[..i].iter().all(|o| o.action != b.action),
                "{:?} listed twice",
                b.action
            );
        }
    }

    #[test]
    fn menu_hints() {
        assert_eq!(hint(A::PlayPause), "Space");
        assert_eq!(hint(A::Quit), "Ctrl + Q");
        assert_eq!(hint(A::HideUi), "Ctrl + \\");
        assert_eq!(hint(A::QueueStep), "N");
        assert_eq!(hint_alt(A::QueueStep), "Shift + N");
        assert_eq!(hint(A::Stop), "S");
    }

    #[test]
    fn docs_match_keymap() {
        let docs = include_str!("../docs/features.md");
        let table = markdown_table();
        assert!(
            docs.contains(&table),
            "docs/features.md shortcut table is out of date; expected:\n{table}"
        );
    }
}
