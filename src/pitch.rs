//! Per-session audio pitch shift in semitones (`-12..=+12`) without changing tempo.
//!
//! Uses mpv's `rubberband` audio filter (`@vivi-pitch:rubberband=pitch-scale=…`) when
//! libmpv was built with `librubberband`, and automatically falls back to FFmpeg's
//! built-in `lavfi` graph (`aresample`, `asetrate`, `aresample`, `atempo`) when it is
//! absent (as in AppImage/Flatpak builds). Resets on track change unless
//! `karaoke.keep_pitch` is enabled.

use crate::player::Player;
use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::Cell;
use std::rc::Rc;

pub const MIN: i32 = -12;
pub const MAX: i32 = 12;
pub const LABEL: &str = "@vivi-pitch";

thread_local! {
    static SEMIS: Cell<i32> = const { Cell::new(0) };
    /// `None` = untried, `Some(true)` = works, `Some(false)` = missing/broken -> use lavfi.
    static RUBBERBAND: Cell<Option<bool>> = const { Cell::new(None) };
    static APPLIED: Cell<bool> = const { Cell::new(false) };
}

pub fn clamp(st: i32) -> i32 {
    st.clamp(MIN, MAX)
}

pub fn ratio(st: i32) -> f64 {
    2.0_f64.powf(clamp(st) as f64 / 12.0)
}

/// "+2 st", "−3 st", "0 st".
pub fn label(st: i32) -> String {
    let st = clamp(st);
    if st == 0 {
        "0 st".into()
    } else if st > 0 {
        format!("+{st} st")
    } else {
        format!("−{} st", -st)
    }
}

pub fn rubberband_filter(st: i32) -> String {
    format!("rubberband=pitch-scale={:.6}", ratio(st))
}

/// Built-in FFmpeg filter fallback when `librubberband` is not linked into libmpv.
pub fn lavfi_filter(st: i32, sample_rate: u32) -> String {
    let sr = sample_rate.clamp(8_000, 192_000);
    let new_sr = ((sr as f64) * ratio(st)).round().clamp(4_000.0, 384_000.0) as u32;
    let tempo = ((sr as f64) / (new_sr as f64)).clamp(0.5, 2.0);
    format!("lavfi=[aresample={sr},asetrate={new_sr},aresample={sr},atempo={tempo:.6}]")
}

pub fn current() -> i32 {
    SEMIS.with(|s| s.get())
}

pub fn is_rubberband_error(prefix: &str, text: &str) -> bool {
    let p = prefix.to_ascii_lowercase();
    let t = text.to_ascii_lowercase();
    (p.contains("rubberband") || t.contains("rubberband"))
        && (t.contains("error")
            || t.contains("failed")
            || t.contains("not found")
            || t.contains("no such")
            || t.contains("couldn't")
            || t.contains("cannot")
            || t.contains("unable"))
}

pub fn sync_ui(app: &AppWindow) {
    let st = current();
    let text: slint::SharedString = label(st).into();
    let l = app.global::<crate::Lyrics>();
    l.set_pitch(st);
    l.set_pitch_label(text.clone());
    let p = app.global::<crate::Panel>();
    p.set_pitch(st);
    p.set_pitch_label(text);
}

fn apply(app: &AppWindow, player: &Player, st: i32) {
    let st = clamp(st);
    SEMIS.with(|s| s.set(st));
    sync_ui(app);

    if st == 0 {
        if APPLIED.with(|a| a.get())
            || player
                .get("af")
                .unwrap_or_default()
                .contains(&format!("{LABEL}:"))
        {
            let _ = player.try_cmd("af", &["remove", LABEL]);
        }
        APPLIED.with(|a| a.set(false));
        return;
    }

    if RUBBERBAND.with(|r| r.get()) != Some(false) {
        let spec = format!("{LABEL}:{}", rubberband_filter(st));
        if player.try_cmd("af", &["add", &spec]).is_ok() {
            APPLIED.with(|a| a.set(true));
            crate::viz::reassert_tap();
            return;
        }
        RUBBERBAND.with(|r| r.set(Some(false)));
    }

    let sr = player
        .mpv
        .get_property::<i64>("audio-params/samplerate")
        .ok()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(48_000);
    let spec = format!("{LABEL}:{}", lavfi_filter(st, sr));
    if player.try_cmd("af", &["add", &spec]).is_ok() {
        APPLIED.with(|a| a.set(true));
        crate::viz::reassert_tap();
    }
}

pub fn set(app: &AppWindow, player: &Player, st: i32, toast: bool) {
    let st = clamp(st);
    apply(app, player, st);
    if toast {
        crate::plugins::toast(app, &format!("Pitch: {}", label(st)));
    }
}

pub fn step(app: &AppWindow, player: &Player, delta: i32) {
    set(app, player, current() + delta, true);
}

pub fn reset(app: &AppWindow, player: &Player, toast: bool) {
    set(app, player, 0, toast);
}

pub fn file_loaded(app: &AppWindow, player: &Player, keep_pitch: bool) {
    let cur = current();
    if !keep_pitch {
        if cur != 0 {
            apply(app, player, 0);
        } else {
            sync_ui(app);
        }
    } else if cur != 0 {
        apply(app, player, cur);
    } else {
        sync_ui(app);
    }
}

pub fn mpv_log(app: &AppWindow, player: &Player, prefix: &str, text: &str) {
    if RUBBERBAND.with(|r| r.get()) != Some(false)
        && current() != 0
        && is_rubberband_error(prefix, text)
    {
        RUBBERBAND.with(|r| r.set(Some(false)));
        let _ = player.try_cmd("af", &["remove", LABEL]);
        APPLIED.with(|a| a.set(false));
        apply(app, player, current());
    }
}

pub fn setup(app: &AppWindow, player: &Rc<Player>) {
    sync_ui(app);
    {
        let w = app.as_weak();
        let p = player.clone();
        app.global::<crate::Lyrics>().on_step_pitch(move |d| {
            let Some(a) = w.upgrade() else { return };
            step(&a, &p, d);
        });
    }
    {
        let w = app.as_weak();
        let p = player.clone();
        app.global::<crate::Panel>().on_step_pitch(move |d| {
            let Some(a) = w.upgrade() else { return };
            step(&a, &p, d);
        });
    }
    {
        let w = app.as_weak();
        let p = player.clone();
        app.global::<crate::Panel>().on_reset_pitch(move || {
            let Some(a) = w.upgrade() else { return };
            reset(&a, &p, true);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_and_ratio_work() {
        assert_eq!(clamp(-20), -12);
        assert_eq!(clamp(20), 12);
        assert_eq!(clamp(3), 3);
        assert!((ratio(0) - 1.0).abs() < 1e-9);
        assert!((ratio(12) - 2.0).abs() < 1e-9);
        assert!((ratio(-12) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn labels_use_unicode_minus() {
        assert_eq!(label(0), "0 st");
        assert_eq!(label(3), "+3 st");
        assert_eq!(label(-4), "−4 st");
    }

    #[test]
    fn lavfi_fallback_keeps_tempo_within_atempo_bounds() {
        let up = lavfi_filter(12, 48_000);
        assert!(up.contains("aresample=48000"));
        assert!(up.contains("asetrate=96000"));
        assert!(up.contains("atempo=0.500000"));

        let down = lavfi_filter(-12, 44_100);
        assert!(down.contains("aresample=44100"));
        assert!(down.contains("asetrate=22050"));
        assert!(down.contains("atempo=2.000000"));
    }

    #[test]
    fn detects_rubberband_log_errors() {
        assert!(is_rubberband_error("lavfi", "No such filter: 'rubberband'"));
        assert!(is_rubberband_error(
            "af",
            "Error initializing filter 'rubberband'"
        ));
        assert!(!is_rubberband_error("lavfi", "Initialized audio filter"));
    }
}
