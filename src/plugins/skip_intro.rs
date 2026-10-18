//! Skip Intro/Credits: a "Skip intro" button while playback is inside an intro or
//! credits range. Ranges come from chapter names (Intro, Opening, OP, Credits, ED, …)
//! and from user-defined per-show ranges (`show = 0:30-2:00, 21:10-end; …`), which
//! can also be marked from Settings while watching. Optionally skips automatically.

use super::{contrib, Ctx, Manifest, Plugin, SettingSpec};
use crate::config::show_key;
use slint::ComponentHandle;

pub const ID: &str = "skip-intro";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Intro,
    Credits,
}

impl Part {
    fn label(self) -> &'static str {
        match self {
            Part::Intro => "Skip intro",
            Part::Credits => "Skip credits",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub start: f64,
    /// `f64::INFINITY` = until the end of the file.
    pub end: f64,
    pub part: Part,
}

#[derive(Default)]
pub struct SkipIntro {
    ranges: Vec<Range>,
    /// Index of the range already auto-skipped (or dismissed) in this file.
    done: Vec<usize>,
    duration: f64,
    mark: Option<f64>,
}

/// Classify a chapter title.
pub fn classify(title: &str) -> Option<Part> {
    let t = title.to_lowercase();
    let words: Vec<&str> = t
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let any = |set: &[&str]| words.iter().any(|w| set.contains(w));
    let numbered = |p: &str| {
        words.iter().any(|w| {
            w.strip_prefix(p)
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
    };
    if any(&["intro", "opening", "op", "introduction"]) || numbered("op") {
        Some(Part::Intro)
    } else if any(&["credits", "ending", "ed", "outro", "endcard"]) || numbered("ed") {
        Some(Part::Credits)
    } else {
        None
    }
}

/// Ranges from chapters `(title, start)`; a chapter ends where the next one starts.
pub fn chapter_ranges(ch: &[(String, f64)], duration: f64) -> Vec<Range> {
    ch.iter()
        .enumerate()
        .filter_map(|(i, (t, s))| {
            let part = classify(t)?;
            let end = ch.get(i + 1).map(|c| c.1).unwrap_or(if duration > 0.0 {
                duration
            } else {
                f64::INFINITY
            });
            (end > *s).then_some(Range {
                start: *s,
                end,
                part,
            })
        })
        .collect()
}

/// "1:02:03" / "1:30" / "90" / "end" → seconds.
pub fn parse_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("end") {
        return Some(f64::INFINITY);
    }
    let mut t = 0.0;
    for p in s.split(':') {
        t = t * 60.0 + p.trim().parse::<f64>().ok()?;
    }
    Some(t)
}

pub fn fmt_time(t: f64) -> String {
    if t.is_infinite() {
        return "end".into();
    }
    crate::util::clock(t.round() as i64)
}

/// Parse `show = a-b, c-d; other = …` into (show, ranges). The first range of a show
/// is its intro; ranges reaching the end (or after the half of the file) are credits.
pub fn parse_user(spec: &str) -> Vec<(String, Vec<(f64, f64)>)> {
    spec.split([';', '\n'])
        .filter_map(|entry| {
            let (show, r) = entry.split_once('=')?;
            let ranges = r
                .split(',')
                .filter_map(|x| {
                    let (a, b) = x.split_once('-')?;
                    let (a, b) = (parse_time(a)?, parse_time(b)?);
                    (b > a).then_some((a, b))
                })
                .collect::<Vec<_>>();
            Some((show.trim().to_lowercase(), ranges))
        })
        .filter(|(s, r)| !s.is_empty() && !r.is_empty())
        .collect()
}

pub fn format_user(v: &[(String, Vec<(f64, f64)>)]) -> String {
    v.iter()
        .map(|(s, r)| {
            let r: Vec<String> = r
                .iter()
                .map(|(a, b)| format!("{}-{}", fmt_time(*a), fmt_time(*b)))
                .collect();
            format!("{s} = {}", r.join(", "))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn user_ranges(spec: &str, show: &str, duration: f64) -> Vec<Range> {
    parse_user(spec)
        .into_iter()
        .filter(|(s, _)| s == show)
        .flat_map(|(_, r)| r)
        .map(|(start, end)| {
            let credits = end.is_infinite() || (duration > 0.0 && start > duration / 2.0);
            Range {
                start,
                end: if end.is_infinite() && duration > 0.0 {
                    duration
                } else {
                    end
                },
                part: if credits { Part::Credits } else { Part::Intro },
            }
        })
        .collect()
}

impl SkipIntro {
    fn reload(&mut self, cx: &Ctx) {
        self.done.clear();
        self.duration = cx.player.mpv.get_property("duration").unwrap_or(0.0);
        let mut r = Vec::new();
        if cx.flag("chapters") {
            r = chapter_ranges(&cx.player.chapters(), self.duration);
        }
        if let Some(p) = cx.player.get("path") {
            r.extend(user_ranges(&cx.get("ranges"), &show_key(&p), self.duration));
        }
        self.ranges = r;
    }
}

impl Plugin for SkipIntro {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Skip Intro & Credits",
            "Shows a Skip button during intros and credits found in chapter names or set per show.",
            "skip",
        )
        .contribute(contrib::PALETTE_COMMAND, "Mark intro / credits here", "mark")
        .contribute(contrib::CONTROLBAR_BUTTON, "Mark intro / credits here", "mark")
        .setting(SettingSpec::toggle(
            "auto",
            "Skip automatically",
            "Jump over intros and credits without asking.",
            false,
        ))
        .setting(SettingSpec::toggle(
            "chapters",
            "Detect from chapter names",
            "Chapters named Intro, Opening, OP, Credits, Ending or ED.",
            true,
        ))
        .setting(SettingSpec::text(
            "ranges",
            "Per-show ranges",
            "Format: show name = 0:30-2:00, 21:10-end; other show = …  The show name is the file name without the episode number.",
            "the wand = 0:30-2:00",
            "",
        ))
        .setting(SettingSpec::action(
            "mark",
            "Mark range for this show",
            "Press once at the start of the intro (or credits) and again at its end.",
            "Mark",
        ))
        .setting(SettingSpec::action(
            "clear",
            "Clear ranges for this show",
            "",
            "Clear",
        ))
    }

    fn enable(&mut self, cx: &Ctx) {
        self.reload(cx);
    }
    fn disable(&mut self, cx: &Ctx) {
        self.ranges.clear();
        cx.app.global::<crate::SkipIntro>().set_label("".into());
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.mark = None;
        self.reload(cx);
    }
    fn setting_changed(&mut self, cx: &Ctx, _key: &str) {
        self.reload(cx);
    }

    fn command(&mut self, cx: &Ctx, name: &str, _args: &[String]) {
        let Some(path) = cx.player.get("path") else {
            cx.toast("Open a file first");
            return;
        };
        let show = show_key(&path);
        let t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        let mut all = parse_user(&cx.get("ranges"));
        match name {
            "mark" => match self.mark.take() {
                None => {
                    self.mark = Some(t);
                    cx.toast(&format!(
                        "Range starts at {}; mark again at its end",
                        fmt_time(t)
                    ));
                    return;
                }
                Some(a) if t > a => {
                    let end = if self.duration > 0.0 && self.duration - t < 2.0 {
                        f64::INFINITY
                    } else {
                        t
                    };
                    match all.iter_mut().find(|(s, _)| *s == show) {
                        Some((_, r)) => r.push((a, end)),
                        None => all.push((show.clone(), vec![(a, end)])),
                    }
                    cx.toast(&format!(
                        "Saved {}–{} for “{show}”",
                        fmt_time(a),
                        fmt_time(end)
                    ));
                }
                Some(_) => {
                    cx.toast("The end must be after the start");
                    return;
                }
            },
            "clear" => {
                all.retain(|(s, _)| *s != show);
                cx.toast(&format!("Cleared ranges for “{show}”"));
            }
            "skip" => {
                if let Some(i) = self.current(t) {
                    self.skip(cx, i);
                }
                return;
            }
            "dismiss" => {
                if let Some(i) = self.current(t) {
                    self.done.push(i);
                }
                cx.app.global::<crate::SkipIntro>().set_label("".into());
                return;
            }
            _ => return,
        }
        cx.set("ranges", &format_user(&all));
        self.reload(cx);
    }

    fn tick(&mut self, cx: &Ctx) {
        if self.ranges.is_empty() {
            if !cx.app.global::<crate::SkipIntro>().get_label().is_empty() {
                cx.app.global::<crate::SkipIntro>().set_label("".into());
            }
            return;
        }
        let t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(-1.0);
        let cur = self.current(t).filter(|i| !self.done.contains(i));
        let label = match cur {
            Some(i) => {
                let r = self.ranges[i].clone();
                if cx.flag("auto") {
                    self.skip(cx, i);
                    ""
                } else {
                    cx.app
                        .global::<crate::SkipIntro>()
                        .set_target(if r.end.is_infinite() {
                            self.duration as f32
                        } else {
                            r.end as f32
                        });
                    r.part.label()
                }
            }
            None => "",
        };
        if cx.app.global::<crate::SkipIntro>().get_label().as_str() != label {
            cx.app.global::<crate::SkipIntro>().set_label(label.into());
        }
    }
}

impl SkipIntro {
    fn skip(&mut self, cx: &Ctx, i: usize) {
        let r = self.ranges[i].clone();
        self.done.push(i);
        if r.end.is_infinite() || (self.duration > 0.0 && r.end >= self.duration - 0.5) {
            cx.player.playlist_step(true);
        } else {
            cx.player.seek_abs(r.end);
        }
        cx.app.global::<crate::SkipIntro>().set_label("".into());
        if cx.flag("auto") {
            cx.toast(match r.part {
                Part::Intro => "Skipped intro",
                Part::Credits => "Skipped credits",
            });
        }
    }
    fn current(&self, t: f64) -> Option<usize> {
        self.ranges
            .iter()
            .position(|r| t >= r.start && t < r.end - 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_names() {
        for t in [
            "Intro",
            "Opening",
            "OP",
            "OP1",
            "Opening Credits",
            "Prologue / Intro",
        ] {
            assert_eq!(classify(t), Some(Part::Intro), "{t}");
        }
        for t in ["Credits", "Ending", "ED", "ED2", "End Credits", "Outro"] {
            assert_eq!(classify(t), Some(Part::Credits), "{t}");
        }
        for t in ["Chapter 1", "Opportunity", "Episode", "Edward", "Part B"] {
            assert_eq!(classify(t), None, "{t}");
        }
    }

    #[test]
    fn ranges_from_chapters() {
        let ch = vec![
            ("Prologue".to_string(), 0.0),
            ("Opening".to_string(), 60.0),
            ("Part A".to_string(), 150.0),
            ("Ending".to_string(), 1300.0),
        ];
        let r = chapter_ranges(&ch, 1420.0);
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].start, r[0].end, r[0].part),
            (60.0, 150.0, Part::Intro)
        );
        assert_eq!(
            (r[1].start, r[1].end, r[1].part),
            (1300.0, 1420.0, Part::Credits)
        );
    }

    #[test]
    fn user_spec_roundtrip() {
        assert_eq!(parse_time("1:02:03"), Some(3723.0));
        assert_eq!(parse_time("90"), Some(90.0));
        assert_eq!(parse_time("x"), None);
        let v = parse_user("The Wand = 0:30-2:00, 21:10-end; bad; other=1-0");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, "the wand");
        assert_eq!(v[0].1, vec![(30.0, 120.0), (1270.0, f64::INFINITY)]);
        assert_eq!(format_user(&v), "the wand = 0:30-2:00, 21:10-end");
        let r = user_ranges(&format_user(&v), "the wand", 1400.0);
        assert_eq!(r[0].part, Part::Intro);
        assert_eq!(r[1].part, Part::Credits);
        assert_eq!(r[1].end, 1400.0);
    }
}
