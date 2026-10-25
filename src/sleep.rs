//! Sleep timer: pause after some minutes, at the end of the file or chapter, or after N
//! queue items, lowering the volume over the last `playback.sleep_fade` seconds first
//! (then pausing and putting the volume back). One timer for the side panel
//! (Playback → Sleep Timer), the top-bar moon indicator (src/status.rs), IPC
//! (`sleep`) and packages (`vivi-sleep <spec>`); its state is published in mpv's
//! `user-data/vivi/sleep`.

use crate::config::Config;
use crate::player::Player;
use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Off,
    Minutes(u32),
    EndOfFile,
    EndOfChapter,
    /// Pause at the end of the Nth file, counting the current one.
    Items(u32),
}

/// Parse a timer spec: `off`, `min:<n>` (or a bare `<n>`), `eof`, `chapter`, `items:<n>`.
pub fn parse(s: &str) -> Option<Mode> {
    let s = s.trim().to_ascii_lowercase();
    let num = |v: &str| v.trim().parse::<u32>().ok().filter(|n| *n > 0);
    Some(match s.as_str() {
        "" | "off" | "0" | "cancel" => Mode::Off,
        "eof" | "end" | "end-of-file" => Mode::EndOfFile,
        "chapter" | "end-of-chapter" => Mode::EndOfChapter,
        _ => {
            if let Some(v) = s.strip_prefix("min:") {
                Mode::Minutes(num(v)?.min(24 * 60))
            } else if let Some(v) = s.strip_prefix("items:") {
                Mode::Items(num(v)?.min(999))
            } else {
                Mode::Minutes(num(&s)?.min(24 * 60))
            }
        }
    })
}

/// The spec string for `m` (inverse of [`parse`]).
pub fn spec(m: Mode) -> String {
    match m {
        Mode::Off => "off".into(),
        Mode::Minutes(n) => format!("min:{n}"),
        Mode::EndOfFile => "eof".into(),
        Mode::EndOfChapter => "chapter".into(),
        Mode::Items(n) => format!("items:{n}"),
    }
}

/// Index into the side panel's choices (`Status.sleep-options`).
pub fn choice(m: Mode) -> i32 {
    match m {
        Mode::Off => 0,
        Mode::Minutes(15) => 1,
        Mode::Minutes(30) => 2,
        Mode::Minutes(60) => 3,
        Mode::Minutes(_) => 4,
        Mode::EndOfFile => 5,
        Mode::EndOfChapter => 6,
        Mode::Items(_) => 7,
    }
}

/// Volume during the fade: full until `fade` seconds are left, then down linearly to 0.
pub fn fade_volume(base: f64, remaining: f64, fade: f64) -> f64 {
    if fade <= 0.0 || remaining >= fade {
        base
    } else {
        base * remaining.max(0.0) / fade
    }
}

/// "23:41" / "1:02:03" (remaining time, rounded up).
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).ceil() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// When a timer of mode `m` (with `left` seconds to go, if known) pauses, for the panel.
pub fn describe(m: Mode, left: Option<f64>, items_left: u32) -> String {
    match m {
        Mode::Off => String::new(),
        Mode::Minutes(_) => format!("Pausing in {}", clock(left.unwrap_or(0.0))),
        Mode::EndOfFile => "Pausing at the end of this file".into(),
        Mode::EndOfChapter => "Pausing at the end of this chapter".into(),
        Mode::Items(_) if items_left <= 1 => "Pausing at the end of this file".into(),
        Mode::Items(_) => format!("Pausing after {items_left} files (this one included)"),
    }
}

// ---------------------------------------------------------------------------------

#[derive(Default)]
struct Timer {
    mode: Mode,
    /// Minutes: when it ends.
    deadline: Option<Instant>,
    /// End of chapter: (file, end time in the file).
    chapter_end: Option<(String, f64)>,
    /// After N items: files left, counting the current one.
    items_left: u32,
    /// Volume before the fade started (restored at the end or on cancel).
    base_volume: Option<f64>,
    /// keep-open before an end-of-file mode changed it.
    keep_open: Option<String>,
    /// The last state text shown (to only touch the UI on changes).
    shown: String,
}

type Ctx = (slint::Weak<AppWindow>, Rc<Player>, Rc<RefCell<Config>>);

thread_local! {
    static T: RefCell<Timer> = RefCell::new(Timer::default());
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
    static TICK: slint::Timer = slint::Timer::default();
}

fn ctx() -> Option<(AppWindow, Rc<Player>, Rc<RefCell<Config>>)> {
    CTX.with(|c| {
        let c = c.borrow();
        let (w, p, cfg) = c.as_ref()?;
        Some((w.upgrade()?, p.clone(), cfg.clone()))
    })
}

fn f(p: &Player, name: &str) -> Option<f64> {
    p.mpv
        .get_property::<f64>(name)
        .ok()
        .filter(|v| v.is_finite())
}

fn speed(p: &Player) -> f64 {
    f(p, "speed").filter(|s| *s > 0.0).unwrap_or(1.0)
}

/// Seconds (wall clock) until the timer fires; None when not known yet.
fn remaining(t: &Timer, p: &Player) -> Option<f64> {
    let to_end = |end: f64| Some((end - f(p, "time-pos")?) / speed(p));
    match t.mode {
        Mode::Off => None,
        Mode::Minutes(_) => t
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()).as_secs_f64()),
        Mode::EndOfFile => to_end(f(p, "duration")?),
        Mode::Items(_) if t.items_left <= 1 => to_end(f(p, "duration")?),
        Mode::Items(_) => None,
        Mode::EndOfChapter => {
            let (path, end) = t.chapter_end.clone()?;
            if p.get("path").as_deref() != Some(path.as_str()) {
                return Some(0.0);
            }
            to_end(end)
        }
    }
}

/// The current chapter's end: the next chapter's start, else the file's end.
fn chapter_end(p: &Player) -> Option<f64> {
    let pos = f(p, "time-pos")?;
    p.chapters()
        .into_iter()
        .map(|c| c.1)
        .find(|s| *s > pos + 0.05)
        .or_else(|| f(p, "duration"))
}

fn fade_secs(cfg: &RefCell<Config>) -> f64 {
    cfg.borrow().playback.sleep_fade.clamp(0, 120) as f64
}

/// Hold the file open at its end (so mpv pauses there) while the timer targets it.
fn hold_end(t: &mut Timer, p: &Player, on: bool) {
    if on {
        if t.keep_open.is_none() {
            t.keep_open = Some(p.get("keep-open").unwrap_or_else(|| "yes".into()));
        }
        p.set("keep-open", "always");
    } else if let Some(k) = t.keep_open.take() {
        p.set("keep-open", if k == "always" { "yes" } else { &k });
    }
}

fn restore_volume(t: &mut Timer, p: &Player) {
    if let Some(v) = t.base_volume.take() {
        p.set_volume(v);
    }
}

/// Start (or with `Mode::Off` cancel) the timer.
pub fn set(m: Mode) {
    let Some((a, p, _)) = ctx() else { return };
    T.with(|t| {
        let mut t = t.borrow_mut();
        restore_volume(&mut t, &p);
        hold_end(&mut t, &p, false);
        t.mode = m;
        t.deadline = None;
        t.chapter_end = None;
        t.items_left = 0;
        match m {
            Mode::Minutes(n) => {
                t.deadline = Some(Instant::now() + Duration::from_secs(n as u64 * 60))
            }
            Mode::EndOfFile => hold_end(&mut t, &p, true),
            Mode::EndOfChapter => {
                t.chapter_end = chapter_end(&p).map(|e| (p.get("path").unwrap_or_default(), e))
            }
            Mode::Items(n) => {
                t.items_left = n;
                if n <= 1 {
                    hold_end(&mut t, &p, true);
                }
            }
            Mode::Off => {}
        }
        t.shown = String::from("\u{0}");
    });
    let g = a.global::<crate::Status>();
    g.set_sleep_choice(choice(m));
    match m {
        Mode::Minutes(n) if choice(m) == 4 => g.set_sleep_minutes(n as i32),
        Mode::Items(n) => g.set_sleep_items(n as i32),
        _ => {}
    }
    publish(&p);
    tick();
    crate::status::refresh();
}

pub fn mode() -> Mode {
    T.with(|t| t.borrow().mode)
}

/// Seconds left, when known.
pub fn left() -> Option<f64> {
    let (_, p, _) = ctx()?;
    T.with(|t| remaining(&t.borrow(), &p))
}

/// Top-bar indicator: (label, tooltip) while a timer runs.
pub fn indicator() -> Option<(String, String)> {
    let m = mode();
    if m == Mode::Off {
        return None;
    }
    let left = left();
    let items = T.with(|t| t.borrow().items_left);
    let label = match (m, left) {
        (Mode::Minutes(_), Some(l)) => clock(l),
        (Mode::Items(_), _) if items > 1 => format!("{items}"),
        (_, Some(l)) if l > 0.0 => clock(l),
        _ => String::new(),
    };
    Some((label, format!("Sleep timer — {}", describe(m, left, items))))
}

/// State for IPC / `user-data/vivi/sleep`.
pub fn state_json() -> serde_json::Value {
    let m = mode();
    let items = T.with(|t| t.borrow().items_left);
    let left = if m == Mode::Off { None } else { left() };
    serde_json::json!({
        "mode": spec(m),
        "active": m != Mode::Off,
        "remaining": left.map(|l| (l * 10.0).round() / 10.0),
        "items_left": if matches!(m, Mode::Items(_)) { Some(items) } else { None },
        "fade": ctx().map(|(_, _, c)| fade_secs(&c)).unwrap_or(30.0),
    })
}

fn publish(p: &Player) {
    p.set("user-data/vivi/sleep", &state_json().to_string());
}

/// Set from IPC / a package: a spec string; Err for an unknown one.
pub fn set_spec(s: &str) -> Result<(), String> {
    let m = parse(s).ok_or_else(|| {
        format!("unknown sleep timer \"{s}\" (off, min:<n>, eof, chapter, items:<n>)")
    })?;
    set(m);
    Ok(())
}

/// A new file started: count it for "after N items".
pub fn file_loaded(p: &Player) {
    let hold = T.with(|t| {
        let mut t = t.borrow_mut();
        if let Mode::Items(_) = t.mode {
            t.items_left = t.items_left.saturating_sub(1).max(1);
            t.shown = String::from("\u{0}");
            return Some(t.items_left <= 1);
        }
        None
    });
    if let Some(h) = hold {
        T.with(|t| hold_end(&mut t.borrow_mut(), p, h));
        publish(p);
    }
}

fn finish(a: &AppWindow, p: &Player) {
    p.set("pause", "yes");
    T.with(|t| {
        let mut t = t.borrow_mut();
        restore_volume(&mut t, p);
        hold_end(&mut t, p, false);
    });
    set(Mode::Off);
    crate::plugins::toast(a, "Sleep timer: paused");
}

/// Every 250 ms while a timer runs: fade, fire, refresh the panel text.
fn tick() {
    let Some((a, p, cfg)) = ctx() else { return };
    let g = a.global::<crate::Status>();
    let fade = fade_secs(&cfg);
    if g.get_sleep_fade() != fade as i32 {
        g.set_sleep_fade(fade as i32);
    }
    let (m, left, items) = T.with(|t| {
        let t = t.borrow();
        (t.mode, remaining(&t, &p), t.items_left)
    });
    if m == Mode::Off {
        TICK.with(|t| t.stop());
        if !g.get_sleep_state().is_empty() {
            g.set_sleep_state("".into());
        }
        return;
    }
    let at_end = matches!(m, Mode::EndOfFile) || (matches!(m, Mode::Items(_)) && items <= 1);
    let eof = at_end && p.get("eof-reached").as_deref() == Some("yes");
    if eof || left.is_some_and(|l| l <= 0.25) {
        finish(&a, &p);
        return;
    }
    if let Some(l) = left {
        if l < fade && p.get("pause").as_deref() != Some("yes") {
            T.with(|t| {
                let mut t = t.borrow_mut();
                let base = *t
                    .base_volume
                    .get_or_insert_with(|| f(&p, "volume").unwrap_or(100.0));
                p.set_volume(fade_volume(base, l, fade));
            });
        }
    }
    let text = describe(m, left, items);
    let changed = T.with(|t| {
        let mut t = t.borrow_mut();
        let c = t.shown != text;
        t.shown = text.clone();
        c
    });
    if changed {
        g.set_sleep_state(text.into());
    }
    TICK.with(|t| {
        if !t.running() {
            t.start(slint::TimerMode::Repeated, Duration::from_millis(250), tick);
        }
    });
}

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>) {
    CTX.with(|c| *c.borrow_mut() = Some((app.as_weak(), player.clone(), cfg.clone())));
    let g = app.global::<crate::Status>();
    g.set_sleep_fade(fade_secs(&cfg) as i32);
    g.on_sleep_set(|s| {
        if let Err(e) = set_spec(&s) {
            eprintln!("sleep: {e}");
        }
    });
    g.on_sleep_fade_set(move |secs| {
        let Some((a, _, cfg)) = ctx() else { return };
        cfg.borrow_mut().playback.sleep_fade = secs.clamp(0, 120) as i64;
        if let Err(e) = cfg.borrow().save() {
            eprintln!("sleep: saving the fade failed: {e}");
        }
        a.global::<crate::Status>()
            .set_sleep_fade(secs.clamp(0, 120));
        if let Some((_, p, _)) = ctx() {
            publish(&p);
        }
    });
    publish(player);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_specs() {
        assert_eq!(parse("off"), Some(Mode::Off));
        assert_eq!(parse(""), Some(Mode::Off));
        assert_eq!(parse("0"), Some(Mode::Off));
        assert_eq!(parse("min:45"), Some(Mode::Minutes(45)));
        assert_eq!(parse(" 30 "), Some(Mode::Minutes(30)));
        assert_eq!(parse("EOF"), Some(Mode::EndOfFile));
        assert_eq!(parse("end-of-file"), Some(Mode::EndOfFile));
        assert_eq!(parse("chapter"), Some(Mode::EndOfChapter));
        assert_eq!(parse("items:3"), Some(Mode::Items(3)));
        assert_eq!(parse("min:99999"), Some(Mode::Minutes(1440)));
        assert_eq!(parse("min:0"), None);
        assert_eq!(parse("items:"), None);
        assert_eq!(parse("-5"), None);
        assert_eq!(parse("soon"), None);
    }

    #[test]
    fn spec_round_trips() {
        for m in [
            Mode::Off,
            Mode::Minutes(15),
            Mode::Minutes(42),
            Mode::EndOfFile,
            Mode::EndOfChapter,
            Mode::Items(4),
        ] {
            assert_eq!(parse(&spec(m)), Some(m));
        }
    }

    #[test]
    fn panel_choices() {
        assert_eq!(choice(Mode::Off), 0);
        assert_eq!(choice(Mode::Minutes(30)), 2);
        assert_eq!(choice(Mode::Minutes(45)), 4);
        assert_eq!(choice(Mode::EndOfFile), 5);
        assert_eq!(choice(Mode::EndOfChapter), 6);
        assert_eq!(choice(Mode::Items(2)), 7);
    }

    #[test]
    fn fade_ramps_down_linearly() {
        assert_eq!(fade_volume(80.0, 100.0, 30.0), 80.0);
        assert_eq!(fade_volume(80.0, 30.0, 30.0), 80.0);
        assert_eq!(fade_volume(80.0, 15.0, 30.0), 40.0);
        assert_eq!(fade_volume(80.0, 0.0, 30.0), 0.0);
        assert_eq!(fade_volume(80.0, -2.0, 30.0), 0.0);
        assert_eq!(fade_volume(80.0, 1.0, 0.0), 80.0, "no fade");
    }

    #[test]
    fn clock_and_text() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(59.2), "1:00");
        assert_eq!(clock(1421.0), "23:41");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(describe(Mode::Off, None, 0), "");
        assert_eq!(
            describe(Mode::Minutes(30), Some(90.0), 0),
            "Pausing in 1:30"
        );
        assert_eq!(
            describe(Mode::Items(3), None, 3),
            "Pausing after 3 files (this one included)"
        );
        assert_eq!(
            describe(Mode::Items(3), Some(5.0), 1),
            "Pausing at the end of this file"
        );
    }
}
