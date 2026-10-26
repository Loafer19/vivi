//! A-B loop and frame stepping: explicit set A / set B / clear, nudging the last set
//! point by one frame, `,` / `.` frame steps (keymap `AbSetA`, `AbSetB`, `AbClear`,
//! `AbNudge`, `FrameStep`). `Shift+L` keeps mpv's own `ab-loop` cycle. The loop is
//! shown by the seek-bar A/B overlay (`Playback.ab-a` / `ab-b`), which follows mpv's
//! `ab-loop-a` / `ab-loop-b` properties whoever sets them ([`sync`]); feedback is a
//! toast. This replaces the A-B Loop & Frame Step package (`ab-loop`). The control
//! socket's `ab_loop` command (vivi-mcp `get_ab_loop` / `set_ab_loop`) reads and sets
//! the points; the top-bar chip follows every change at once.

use crate::player::Player;
use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::Cell;

/// Which loop point a key changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    A,
    B,
}

thread_local! {
    /// The point set last (what Alt+← / Alt+→ move).
    static LAST: Cell<Point> = const { Cell::new(Point::A) };
}

/// mpv's `ab-loop-a` / `ab-loop-b` value ("no" = unset).
fn point(p: &Player, name: &str) -> Option<f64> {
    p.get(name)
        .filter(|v| v != "no")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
}

fn prop(p: &Player, name: &str) -> Option<f64> {
    p.get(name)
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// "1:05.25": minutes and seconds with hundredths (frame-level points).
pub fn fmt(t: f64) -> String {
    let t = t.max(0.0);
    let cs = (t * 100.0).round() as u64;
    let (m, s, c) = (cs / 6000, cs / 100 % 60, cs % 100);
    if m >= 60 {
        format!("{}:{:02}:{:02}.{c:02}", m / 60, m % 60, s)
    } else {
        format!("{m}:{s:02}.{c:02}")
    }
}

/// Side panel's *A-B Loop* line.
pub fn state_text(a: Option<f64>, b: Option<f64>) -> String {
    match (a, b) {
        (Some(x), Some(y)) => format!("Looping {} → {}", fmt(x), fmt(y)),
        (Some(x), None) => format!("A = {}, set B", fmt(x)),
        (None, Some(y)) => format!("B = {}, set A", fmt(y)),
        _ => "Off".into(),
    }
}

/// Loop points after setting `which` to `t`. A point set on the wrong side of the other
/// one swaps them, so A stays before B and the loop never runs backwards. Returns the
/// new (A, B) and which point now holds `t`.
pub fn set_point(
    a: Option<f64>,
    b: Option<f64>,
    which: Point,
    t: f64,
) -> (Option<f64>, Option<f64>, Point) {
    match which {
        Point::A => match b {
            Some(b) if t > b => (Some(b), Some(t), Point::B),
            _ => (Some(t), b, Point::A),
        },
        Point::B => match a {
            Some(a) if t < a => (Some(t), Some(a), Point::A),
            _ => (a, Some(t), Point::B),
        },
    }
}

/// One frame in seconds from the container / estimated frame rate (25 fps when unknown,
/// e.g. audio files).
pub fn frame(container_fps: Option<f64>, estimated_fps: Option<f64>) -> f64 {
    let fps = [container_fps, estimated_fps]
        .into_iter()
        .flatten()
        .find(|f| *f > 0.0 && *f < 1000.0)
        .unwrap_or(25.0);
    1.0 / fps
}

/// `which` moved by `dir` frames, kept within 0..duration and on its side of the other
/// point (at least one frame apart). None when that point is not set.
pub fn nudge(
    a: Option<f64>,
    b: Option<f64>,
    which: Point,
    dir: f64,
    frame: f64,
    duration: Option<f64>,
) -> Option<f64> {
    let (cur, other) = match which {
        Point::A => (a?, b),
        Point::B => (b?, a),
    };
    let mut v = (cur + dir * frame).max(0.0);
    if let Some(d) = duration.filter(|d| *d > 0.0) {
        v = v.min(d);
    }
    match (which, other) {
        (Point::A, Some(o)) => v = v.min((o - frame).max(0.0)),
        (Point::B, Some(o)) => v = v.max(o + frame),
        _ => {}
    }
    Some(v)
}

fn write(p: &Player, a: Option<f64>, b: Option<f64>) {
    let v = |t: Option<f64>| t.map_or("no".to_string(), |t| format!("{t:.6}"));
    p.set("ab-loop-a", &v(a));
    p.set("ab-loop-b", &v(b));
}

/// Mirror mpv's loop points into the seek-bar overlay and the side panel. Called when
/// `ab-loop-a` / `ab-loop-b` change (keys, Shift+L, the panel, scripts, vivi-mcp).
pub fn sync(app: &AppWindow, p: &Player) {
    let (a, b) = (point(p, "ab-loop-a"), point(p, "ab-loop-b"));
    let pb = app.global::<crate::Playback>();
    pb.set_ab_a(a.map_or(-1.0, |v| v as f32));
    pb.set_ab_b(b.map_or(-1.0, |v| v as f32));
    pb.set_ab_state(state_text(a, b).into());
    // The top-bar A-B chip follows at once (not on the next status tick).
    crate::status::refresh();
}

/// Alt+A / Alt+B: set a loop point at the playhead.
pub fn set(app: &AppWindow, p: &Player, which: Point) {
    let Some(t) = prop(p, "time-pos") else {
        return;
    };
    let (a, b, now) = set_point(point(p, "ab-loop-a"), point(p, "ab-loop-b"), which, t);
    write(p, a, b);
    LAST.with(|l| l.set(now));
    sync(app, p);
    let name = |w| if w == Point::A { "A" } else { "B" };
    let msg = match (a, b) {
        (Some(x), Some(y)) => format!("Loop {} → {} ({} set)", fmt(x), fmt(y), name(now)),
        _ => format!(
            "Loop point {}: {} (set {} next)",
            name(now),
            fmt(t),
            name(match now {
                Point::A => Point::B,
                Point::B => Point::A,
            })
        ),
    };
    crate::plugins::toast(app, &msg);
}

/// Alt+C: clear both points.
pub fn clear(app: &AppWindow, p: &Player) {
    write(p, None, None);
    LAST.with(|l| l.set(Point::A));
    sync(app, p);
    crate::plugins::toast(app, "A-B loop cleared");
}

/// Alt+← / Alt+→: move the point set last by one frame.
pub fn nudge_last(app: &AppWindow, p: &Player, dir: f64) {
    let (a, b) = (point(p, "ab-loop-a"), point(p, "ab-loop-b"));
    // After Shift+L or a drag the point we remember may be unset; use the one that is.
    let which = match (LAST.with(Cell::get), a, b) {
        (Point::B, _, Some(_)) | (Point::A, None, Some(_)) => Point::B,
        _ => Point::A,
    };
    let f = frame(prop(p, "container-fps"), prop(p, "estimated-vf-fps"));
    let Some(v) = nudge(a, b, which, dir, f, prop(p, "duration")) else {
        crate::plugins::toast(app, "No A-B loop point to move (Alt+A sets A)");
        return;
    };
    match which {
        Point::A => write(p, Some(v), b),
        Point::B => write(p, a, Some(v)),
    }
    LAST.with(|l| l.set(which));
    sync(app, p);
    let name = if which == Point::A { "A" } else { "B" };
    crate::plugins::toast(app, &format!("Loop point {name}: {}", fmt(v)));
}

/// Loop points from the control socket: each point set, kept (`None` in `keep`) or
/// cleared; A after B swaps them. Err for a negative or non-finite time.
pub fn set_both(
    app: &AppWindow,
    p: &Player,
    a: Option<Option<f64>>,
    b: Option<Option<f64>>,
) -> Result<(), String> {
    let ok = |t: Option<f64>| match t {
        Some(v) if !v.is_finite() || v < 0.0 => Err(format!("invalid loop point {v}")),
        _ => Ok(t),
    };
    let x = ok(a.unwrap_or_else(|| point(p, "ab-loop-a")))?;
    let y = ok(b.unwrap_or_else(|| point(p, "ab-loop-b")))?;
    let (x, y) = match (x, y) {
        (Some(x), Some(y)) if x > y => (Some(y), Some(x)),
        v => v,
    };
    write(p, x, y);
    LAST.with(|l| l.set(if b.is_some() { Point::B } else { Point::A }));
    sync(app, p);
    Ok(())
}

/// Control socket `ab_loop` state: the points (null = unset) and the panel's line.
pub fn state_json(p: &Player) -> serde_json::Value {
    let (a, b) = (point(p, "ab-loop-a"), point(p, "ab-loop-b"));
    serde_json::json!({ "a": a, "b": b, "active": a.is_some() && b.is_some(), "state": state_text(a, b) })
}

/// `,` / `.`: one frame back / forward while paused, otherwise a 1 s seek.
pub fn step(p: &Player, forward: bool) {
    if p.get("pause").as_deref() == Some("yes") {
        p.run(if forward {
            "frame-step"
        } else {
            "frame-back-step"
        });
    } else {
        p.cmd(
            "seek",
            &[if forward { "1" } else { "-1" }, "relative+exact"],
        );
    }
}

/// Points don't carry over to the next file (mpv keeps them as options).
pub fn reset(app: &AppWindow, p: &Player) {
    if point(p, "ab-loop-a").is_some() || point(p, "ab-loop-b").is_some() {
        write(p, None, None);
    }
    LAST.with(|l| l.set(Point::A));
    sync(app, p);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_points() {
        assert_eq!(fmt(0.0), "0:00.00");
        assert_eq!(fmt(14.016667), "0:14.02");
        assert_eq!(fmt(65.25), "1:05.25");
        assert_eq!(fmt(3725.5), "1:02:05.50");
        assert_eq!(fmt(59.999), "1:00.00");
        assert_eq!(state_text(None, None), "Off");
        assert_eq!(state_text(Some(10.0), None), "A = 0:10.00, set B");
        assert_eq!(
            state_text(Some(10.0), Some(14.0)),
            "Looping 0:10.00 → 0:14.00"
        );
    }

    #[test]
    fn set_keeps_a_before_b() {
        assert_eq!(
            set_point(None, None, Point::A, 10.0),
            (Some(10.0), None, Point::A)
        );
        assert_eq!(
            set_point(Some(10.0), None, Point::B, 14.0),
            (Some(10.0), Some(14.0), Point::B)
        );
        // B before A swaps: the loop is 5 → 10, the new point is A.
        assert_eq!(
            set_point(Some(10.0), None, Point::B, 5.0),
            (Some(5.0), Some(10.0), Point::A)
        );
        // A after B swaps too.
        assert_eq!(
            set_point(Some(1.0), Some(4.0), Point::A, 6.0),
            (Some(4.0), Some(6.0), Point::B)
        );
        // B alone, then A before it.
        assert_eq!(
            set_point(None, Some(8.0), Point::A, 2.0),
            (Some(2.0), Some(8.0), Point::A)
        );
    }

    #[test]
    fn frame_length() {
        assert!((frame(Some(60.0), None) - 1.0 / 60.0).abs() < 1e-9);
        assert!((frame(None, Some(30.0)) - 1.0 / 30.0).abs() < 1e-9);
        assert!((frame(Some(0.0), Some(24.0)) - 1.0 / 24.0).abs() < 1e-9);
        assert!((frame(None, None) - 0.04).abs() < 1e-9);
    }

    #[test]
    fn nudges_within_bounds() {
        let f = 0.5;
        assert_eq!(
            nudge(Some(10.0), Some(14.0), Point::B, 1.0, f, Some(60.0)),
            Some(14.5)
        );
        assert_eq!(
            nudge(Some(10.0), Some(14.0), Point::A, -1.0, f, None),
            Some(9.5)
        );
        assert_eq!(nudge(None, None, Point::A, 1.0, f, None), None);
        assert_eq!(nudge(Some(0.2), None, Point::A, -1.0, f, None), Some(0.0));
        assert_eq!(
            nudge(None, Some(59.8), Point::B, 1.0, f, Some(60.0)),
            Some(60.0)
        );
        // A never passes B (one frame gap), B never passes A.
        assert_eq!(
            nudge(Some(13.5), Some(14.0), Point::A, 1.0, f, None),
            Some(13.5)
        );
        assert_eq!(
            nudge(Some(10.0), Some(10.5), Point::B, -1.0, f, None),
            Some(10.5)
        );
    }
}
