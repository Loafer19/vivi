//! Keyboard: keys Vivi does not bind are translated to mpv key names (layout-independent via the physical key) and forwarded.

use crate::*;

/// Send a key Vivi does not use to mpv (`keypress`), so key bindings of mpv scripts and
/// plugin packages work. Returns whether it was sent.
pub(crate) fn forward_key(p: &player::Player, text: &str, ctrl: bool, alt: bool) -> bool {
    let phys = LAST_PHYSICAL.take();
    let k = phys
        .filter(|_| ctrl || alt)
        .and_then(|pk| physical_key(pk, text, ctrl, alt))
        .or_else(|| mpv_key(text, ctrl, alt));
    let Some(k) = k else {
        if key_debug() {
            eprintln!(
                "vivi: key {text:?} (ctrl={ctrl} alt={alt} physical={phys:?}) has no mpv name"
            );
        }
        return false;
    };
    if key_debug() {
        eprintln!("vivi: keypress {k} (physical={phys:?})");
    }
    p.cmd("keypress", &[&k]);
    true
}

thread_local! {
    /// Physical key of the latest winit key press (set in the winit event hook).
    pub(crate) static LAST_PHYSICAL: std::cell::Cell<Option<slint::winit_030::winit::keyboard::PhysicalKey>> =
        const { std::cell::Cell::new(None) };
}

/// US-QWERTY character printed on a physical key (letters lowercase), the layout mpv
/// key bindings are written for.
pub(crate) fn physical_char(code: slint::winit_030::winit::keyboard::KeyCode) -> Option<char> {
    use slint::winit_030::winit::keyboard::KeyCode as K;
    const LETTERS: [K; 26] = [
        K::KeyA,
        K::KeyB,
        K::KeyC,
        K::KeyD,
        K::KeyE,
        K::KeyF,
        K::KeyG,
        K::KeyH,
        K::KeyI,
        K::KeyJ,
        K::KeyK,
        K::KeyL,
        K::KeyM,
        K::KeyN,
        K::KeyO,
        K::KeyP,
        K::KeyQ,
        K::KeyR,
        K::KeyS,
        K::KeyT,
        K::KeyU,
        K::KeyV,
        K::KeyW,
        K::KeyX,
        K::KeyY,
        K::KeyZ,
    ];
    const DIGITS: [K; 10] = [
        K::Digit0,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
    ];
    if let Some(i) = LETTERS.iter().position(|k| *k == code) {
        return Some((b'a' + i as u8) as char);
    }
    if let Some(i) = DIGITS.iter().position(|k| *k == code) {
        return Some((b'0' + i as u8) as char);
    }
    Some(match code {
        K::Minus => '-',
        K::Equal => '=',
        K::BracketLeft => '[',
        K::BracketRight => ']',
        K::Backslash => '\\',
        K::Semicolon => ';',
        K::Quote => '\'',
        K::Backquote => '`',
        K::Comma => ',',
        K::Period => '.',
        K::Slash => '/',
        _ => return None,
    })
}

/// mpv name of a modifier combo from the physical key (layout-independent). Only
/// character keys are handled here; named keys (arrows, …) keep the Slint text path.
/// `text` decides the case: an upper-case character means Shift was held.
pub(crate) fn physical_key(
    pk: slint::winit_030::winit::keyboard::PhysicalKey,
    text: &str,
    ctrl: bool,
    alt: bool,
) -> Option<String> {
    let slint::winit_030::winit::keyboard::PhysicalKey::Code(code) = pk else {
        return None;
    };
    let ch = physical_char(code)?;
    let upper = text.chars().next().is_some_and(char::is_uppercase);
    let k = if upper && ch.is_ascii_lowercase() {
        ch.to_ascii_uppercase().to_string()
    } else {
        ch.to_string()
    };
    let k = if k == "#" { "SHARP".into() } else { k };
    let mut k = k;
    if alt {
        k = format!("Alt+{k}");
    }
    if ctrl {
        k = format!("Ctrl+{k}");
    }
    Some(k)
}

/// `VIVI_KEY_DEBUG=1` logs every key event and what is forwarded to mpv.
pub(crate) fn key_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VIVI_KEY_DEBUG").is_some())
}

/// Fallback when no physical key code is known: QWERTY letter on the same physical key
/// as a Cyrillic (Ukrainian/Russian ЙЦУКЕН) letter.
pub(crate) fn latin_of(ch: char) -> Option<char> {
    const CYR: &str = "йцукенгшщзхїфівапролджєячсмитьбюыэъё";
    const LAT: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.s']`";
    let lower = ch.to_lowercase().next()?;
    let i = CYR.chars().position(|c| c == lower)?;
    let l = LAT.chars().nth(i)?;
    Some(if ch.is_uppercase() {
        l.to_ascii_uppercase()
    } else {
        l
    })
}

/// mpv key name (`Alt+n`, `Ctrl+LEFT`, `SPACE`, …) for a Slint key event text.
pub(crate) fn mpv_key(text: &str, ctrl: bool, alt: bool) -> Option<String> {
    use slint::platform::Key;
    let named = [
        (Key::LeftArrow, "LEFT"),
        (Key::RightArrow, "RIGHT"),
        (Key::UpArrow, "UP"),
        (Key::DownArrow, "DOWN"),
        (Key::Home, "HOME"),
        (Key::End, "END"),
        (Key::PageUp, "PGUP"),
        (Key::PageDown, "PGDWN"),
    ];
    let mut k = named
        .iter()
        .find(|(k, _)| text == slint::SharedString::from(*k).as_str())
        .map(|(_, n)| n.to_string())
        .or_else(|| {
            let mut c = text.chars();
            match (c.next(), c.next()) {
                (Some(' '), None) => Some("SPACE".into()),
                (Some('#'), None) => Some("SHARP".into()),
                (Some(ch), None) if ch.is_ascii_graphic() => Some(ch.to_string()),
                // Ctrl+letter may arrive as a control character (Ctrl+A = U+0001).
                (Some(ch @ '\u{1}'..='\u{1a}'), None) if ctrl => {
                    Some(((ch as u8 - 1 + b'a') as char).to_string())
                }
                (Some(ch), None) if alt || ctrl => latin_of(ch).map(|l| l.to_string()),
                _ => None,
            }
        })?;
    if alt {
        k = format!("Alt+{k}");
    }
    if ctrl {
        k = format!("Ctrl+{k}");
    }
    Some(k)
}

#[cfg(test)]
mod key_tests {
    use super::mpv_key;

    #[test]
    fn alt_combos_get_mpv_names() {
        assert_eq!(mpv_key("n", false, true).as_deref(), Some("Alt+n"));
        assert_eq!(mpv_key("a", false, true).as_deref(), Some("Alt+a"));
        assert_eq!(mpv_key("A", false, true).as_deref(), Some("Alt+A"));
        let left = slint::SharedString::from(slint::platform::Key::LeftArrow);
        assert_eq!(mpv_key(&left, false, true).as_deref(), Some("Alt+LEFT"));
        assert_eq!(mpv_key(",", false, false).as_deref(), Some(","));
        assert_eq!(mpv_key(" ", true, false).as_deref(), Some("Ctrl+SPACE"));
    }

    #[test]
    fn cyrillic_layout_maps_to_qwerty() {
        // Ukrainian layout: N key types "т", A "ф", B "и", C "с", S "і".
        assert_eq!(mpv_key("т", false, true).as_deref(), Some("Alt+n"));
        assert_eq!(mpv_key("ф", false, true).as_deref(), Some("Alt+a"));
        assert_eq!(mpv_key("и", false, true).as_deref(), Some("Alt+b"));
        assert_eq!(mpv_key("с", false, true).as_deref(), Some("Alt+c"));
        assert_eq!(mpv_key("і", false, true).as_deref(), Some("Alt+s"));
        assert_eq!(mpv_key("т", false, false), None);
    }

    #[test]
    fn physical_keys_are_layout_independent() {
        use super::physical_key;
        use slint::winit_030::winit::keyboard::{KeyCode, NativeKeyCode, PhysicalKey};
        let n = PhysicalKey::Code(KeyCode::KeyN);
        // Ukrainian layout types "т" on N, Greek "ν", German "n": all map to Alt+n.
        for t in ["т", "ν", "n"] {
            assert_eq!(physical_key(n, t, false, true).as_deref(), Some("Alt+n"));
        }
        assert_eq!(physical_key(n, "Т", false, true).as_deref(), Some("Alt+N"));
        // German QWERTZ: the key labelled Z sits on US Y.
        let y = PhysicalKey::Code(KeyCode::KeyY);
        assert_eq!(physical_key(y, "z", true, false).as_deref(), Some("Ctrl+y"));
        let d1 = PhysicalKey::Code(KeyCode::Digit1);
        assert_eq!(
            physical_key(d1, "&", true, true).as_deref(),
            Some("Ctrl+Alt+1")
        );
        assert_eq!(
            physical_key(PhysicalKey::Code(KeyCode::ArrowLeft), "", false, true),
            None
        );
        assert_eq!(
            physical_key(
                PhysicalKey::Unidentified(NativeKeyCode::Unidentified),
                "т",
                false,
                true
            ),
            None
        );
    }

    #[test]
    fn ctrl_control_chars() {
        assert_eq!(mpv_key("\u{1}", true, false).as_deref(), Some("Ctrl+a"));
    }
}
