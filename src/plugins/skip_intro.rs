//! Skip Intro & Credits: the player's one Skip button. It shows while playback is
//! inside a range to skip (or mute) and comes from three sources, merged per file:
//!
//! * chapter names (Intro, Opening, OP, Credits, ED, …; end markers such as
//!   "End of intro" are boundaries, not ranges),
//! * user-defined per-show ranges (`show = 0:30-2:00, 21:10-end; …`), which can also be
//!   marked from Settings while watching,
//! * ranges that plugin packages submit for the current file with
//!   `script-message vivi-skip-ranges <source> <json>` (SponsorBlock 1.3+, see
//!   docs/plugins-architecture.md). Vivi sets `user-data/vivi/skip-api` to the integer 1
//!   while this plugin is on, so packages can tell that the core shows the button.
//!
//! Overlapping ranges that describe the same segment are shown once (your per-show
//! ranges win over plugins, plugins over chapter names). Each range is skipped, offered
//! (*ask*) or ignored (*off*) per category or source; automatic skips leave an Undo
//! button for a few seconds, and after a manual skip the button can offer to skip that
//! part of the show automatically from then on.

use super::{contrib, Ctx, Manifest, Plugin, SettingSpec};
use crate::config::show_key;
use slint::ComponentHandle;
use std::time::{Duration, Instant};

pub const ID: &str = "skip-intro";
/// mpv property packages read to know that the core takes skip ranges (1 = yes).
pub const CAPABILITY: &str = "user-data/vivi/skip-api";
/// Source names of the built-in ranges (also usable in the per-category setting).
pub const CHAPTERS: &str = "chapters";
pub const SHOW: &str = "show";
/// How long the Undo / "always skip" button stays after a skip.
const NOTICE: Duration = Duration::from_secs(6);
/// Most ranges one source may submit for a file.
const MAX_SUBMITTED: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Intro,
    Credits,
}

impl Part {
    pub fn kind(self) -> &'static str {
        match self {
            Part::Intro => "intro",
            Part::Credits => "credits",
        }
    }
}

/// What a range does when it is acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Seek to its end (or the next file for credits that run to the end).
    Skip,
    /// Mute while inside it, sound back after it.
    Mute,
}

/// Skip (or mute) automatically, offer the button, or ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Skip,
    Ask,
    Off,
}

impl Mode {
    fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "skip" | "auto" => Some(Mode::Skip),
            "ask" => Some(Mode::Ask),
            "off" | "no" | "never" => Some(Mode::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub start: f64,
    /// `f64::INFINITY` = until the end of the file.
    pub end: f64,
    /// `chapters`, `show` or the submitting plugin (`sponsorblock`).
    pub source: String,
    /// Category: `intro`, `credits` or a plugin's own (`sponsor`, `selfpromo`, …).
    pub kind: String,
    /// What the button names: "Skip <label>" (`intro`, `sponsor`, …).
    pub label: String,
    pub act: Act,
    /// The mode the source asks for (plugins); `None` = this plugin's settings decide.
    pub wish: Option<Mode>,
}

/// Identity of a range within a file (survives re-merging when chapters change).
type Key = (i64, i64, bool);

impl Range {
    fn key(&self) -> Key {
        let ms = |t: f64| {
            if t.is_finite() {
                (t * 1000.0).round() as i64
            } else {
                i64::MAX
            }
        };
        (ms(self.start), ms(self.end), self.act == Act::Mute)
    }
    /// Lower wins when two ranges describe the same segment.
    fn priority(&self) -> u8 {
        match self.source.as_str() {
            SHOW => 0,
            CHAPTERS => 2,
            _ => 1,
        }
    }
    fn part(start: f64, end: f64, source: &str, part: Part) -> Range {
        Range {
            start,
            end,
            source: source.into(),
            kind: part.kind().into(),
            label: part.kind().into(),
            act: Act::Skip,
            wish: None,
        }
    }
}

/// Whether a chapter title marks where something ends ("End of intro", "Intro end",
/// "End of muted sponsor") rather than starting a range.
pub fn is_end_marker(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    let words: Vec<&str> = t
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    t.starts_with("end of")
        || t.starts_with("end:")
        || (words.len() >= 2 && matches!(words.last(), Some(&"end") | Some(&"ends")))
}

/// Classify a chapter title.
pub fn classify(title: &str) -> Option<Part> {
    if is_end_marker(title) {
        return None;
    }
    let t = title.to_lowercase();
    let words: Vec<&str> = t
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    // Story content around the credits: "Post-credits scene", "Mid-credits", "Opening scene".
    if matches!(
        words.first(),
        Some(&"post") | Some(&"mid") | Some(&"pre") | Some(&"after") | Some(&"before")
    ) || words.contains(&"scene")
    {
        return None;
    }
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
            (end > *s).then(|| Range::part(*s, end, CHAPTERS, part))
        })
        .collect()
}

/// Chapters without the ones a plugin wrote for the ranges it also submitted
/// (SponsorBlock's "Intro" / "End of intro" pairs), so they are not detected twice.
pub fn without_markers(ch: &[(String, f64)], submitted: &[Range]) -> Vec<(String, f64)> {
    let near = |t: f64| {
        submitted
            .iter()
            .any(|r| (r.start - t).abs() < 0.25 || (r.end - t).abs() < 0.25)
    };
    ch.iter().filter(|(_, t)| !near(*t)).cloned().collect()
}

/// One entry of `vivi-skip-ranges`.
#[derive(serde::Deserialize)]
struct Submitted {
    start: f64,
    end: f64,
    #[serde(default, alias = "category")]
    kind: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    action: String,
}

/// Ranges in a `vivi-skip-ranges <source> <json>` message: a JSON array of
/// `{start, end, kind | category, label, action: skip | ask | mute | off}`.
pub fn parse_submitted(source: &str, json: &str) -> Result<Vec<Range>, String> {
    let v: Vec<Submitted> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(v.into_iter()
        .take(MAX_SUBMITTED)
        .filter(|s| s.start.is_finite() && s.end.is_finite() && s.start >= 0.0 && s.end > s.start)
        .filter_map(|s| {
            let (act, wish) = match s.action.trim().to_ascii_lowercase().as_str() {
                "" => (Act::Skip, None),
                "skip" => (Act::Skip, Some(Mode::Skip)),
                "ask" => (Act::Skip, Some(Mode::Ask)),
                "mute" => (Act::Mute, Some(Mode::Skip)),
                "off" => return None,
                _ => (Act::Skip, None),
            };
            let kind = match s.kind.trim().to_ascii_lowercase() {
                k if k.is_empty() => "segment".to_string(),
                k => k,
            };
            let label = s.label.trim();
            let label = label
                .get(..5)
                .filter(|p| p.eq_ignore_ascii_case("skip "))
                .map_or(label, |_| label[5..].trim());
            let label = if label.is_empty() {
                kind.replace('_', " ")
            } else {
                label.chars().take(40).collect()
            };
            Some(Range {
                start: s.start,
                end: s.end,
                source: source.into(),
                kind,
                label,
                act,
                wish,
            })
        })
        .collect())
}

/// Whether two ranges describe the same segment: same action and they overlap by at
/// least half of the shorter one.
fn same_segment(a: &Range, b: &Range) -> bool {
    if a.act != b.act {
        return false;
    }
    let (ae, be) = (a.end.min(1e12), b.end.min(1e12));
    let overlap = ae.min(be) - a.start.max(b.start);
    let shorter = (ae - a.start).min(be - b.start);
    overlap > 0.0 && overlap >= 0.5 * shorter
}

/// All ranges of a file, each segment once (by [`Range::priority`]), sorted by start.
pub fn merge(mut all: Vec<Range>) -> Vec<Range> {
    all.sort_by(|a, b| {
        a.priority()
            .cmp(&b.priority())
            .then(a.start.total_cmp(&b.start))
    });
    let mut out: Vec<Range> = Vec::new();
    for r in all {
        if !out.iter().any(|o| same_segment(o, &r)) {
            out.push(r);
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// `intro=skip sponsorblock:filler=off, credits=ask` → `[(key, mode)]` (keys lowercase).
pub fn parse_modes(spec: &str) -> Vec<(String, Mode)> {
    spec.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter_map(|e| {
            let (k, m) = e.split_once('=')?;
            let k = k.trim().to_ascii_lowercase();
            (!k.is_empty()).then_some((k, Mode::parse(m)?))
        })
        .collect()
}

/// The mode for `r`: `source:kind`, then a remembered "always skip" for this show, then
/// `kind`, `source`, what the source asked for, and finally *Skip automatically*.
pub fn mode_for(r: &Range, modes: &[(String, Mode)], remembered: bool, auto: bool) -> Mode {
    let find = |k: &str| modes.iter().rev().find(|(m, _)| m == k).map(|x| x.1);
    find(&format!("{}:{}", r.source, r.kind))
        .or(remembered.then_some(Mode::Skip))
        .or_else(|| find(&r.kind))
        .or_else(|| find(&r.source))
        .or(r.wish)
        .unwrap_or(if auto { Mode::Skip } else { Mode::Ask })
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

/// `show = intro, credits; other = intro` (shows whose parts are skipped automatically).
pub fn parse_auto_shows(spec: &str) -> Vec<(String, Vec<String>)> {
    spec.split([';', '\n'])
        .filter_map(|entry| {
            let (show, k) = entry.split_once('=')?;
            let kinds: Vec<String> = k
                .split(',')
                .map(|x| x.trim().to_lowercase())
                .filter(|x| !x.is_empty())
                .collect();
            let show = show.trim().to_lowercase();
            (!show.is_empty() && !kinds.is_empty()).then_some((show, kinds))
        })
        .collect()
}

pub fn format_auto_shows(v: &[(String, Vec<String>)]) -> String {
    v.iter()
        .map(|(s, k)| format!("{s} = {}", k.join(", ")))
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
            let end = if end.is_infinite() && duration > 0.0 {
                duration
            } else {
                end
            };
            let part = if credits { Part::Credits } else { Part::Intro };
            Range::part(start, end, SHOW, part)
        })
        .collect()
}

/// What the button does when shown.
#[derive(Debug, Clone, PartialEq)]
enum Button {
    /// Offer to skip (or mute) range `i`.
    Act(usize),
    /// We muted range `i`: give the sound back.
    Unmute(usize),
    /// After a skip: go back.
    Undo,
    /// After a manual skip: skip `kind` of `show` automatically from now on.
    Always { show: String, kind: String },
}

#[derive(Debug, Clone)]
enum After {
    Undo,
    Always { show: String, kind: String },
}

/// The last skip, for Undo (no time limit within the file).
#[derive(Debug, Clone)]
struct Last {
    key: Key,
    start: f64,
    from: f64,
    label: String,
}

#[derive(Default)]
pub struct SkipIntro {
    /// Merged ranges of the current file.
    ranges: Vec<Range>,
    /// Submitted ranges: (source, file path, ranges).
    submitted: Vec<(String, String, Vec<Range>)>,
    /// Ranges skipped, muted or dismissed in this file (no button, no auto action).
    done: Vec<Key>,
    /// Ranges whose skip / mute was undone: played through.
    allowed: Vec<Key>,
    /// Button hidden with ✕ while muted (the mute stays).
    hidden: Vec<Key>,
    /// The range we muted (sound comes back after it).
    muted: Option<Key>,
    last: Option<Last>,
    /// Undo / "always" button until the instant.
    after: Option<(After, String, Instant)>,
    /// "show|kind" already offered for automatic skipping in this session.
    offered: Vec<String>,
    duration: f64,
    chapter_count: i64,
    mark: Option<f64>,
    path: String,
    // Settings, read on reload.
    modes: Vec<(String, Mode)>,
    auto: bool,
    show: String,
    auto_kinds: Vec<String>,
}

impl SkipIntro {
    fn reload(&mut self, cx: &Ctx) {
        let path = cx.player.get("path").unwrap_or_default();
        if path != self.path {
            self.restore_sound(cx);
            self.done.clear();
            self.allowed.clear();
            self.hidden.clear();
            self.last = None;
            self.after = None;
            self.submitted.retain(|(_, p, _)| *p == path);
            self.path = path.clone();
        }
        self.duration = cx.player.mpv.get_property("duration").unwrap_or(0.0);
        self.modes = parse_modes(&cx.get("modes"));
        self.auto = cx.flag("auto");
        let local = !path.is_empty() && !path.contains("://");
        self.show = if local {
            show_key(&path)
        } else {
            String::new()
        };
        self.auto_kinds = parse_auto_shows(&cx.get("auto-shows"))
            .into_iter()
            .find(|(s, _)| !self.show.is_empty() && *s == self.show)
            .map(|(_, k)| k)
            .unwrap_or_default();
        let ext: Vec<Range> = self
            .submitted
            .iter()
            .flat_map(|(_, _, r)| r.iter().cloned())
            .collect();
        let mut r = Vec::new();
        let ch = cx.player.chapters();
        self.chapter_count = ch.len() as i64;
        if cx.flag("chapters") {
            r = chapter_ranges(&without_markers(&ch, &ext), self.duration);
        }
        if !path.is_empty() {
            r.extend(user_ranges(
                &cx.get("ranges"),
                &show_key(&path),
                self.duration,
            ));
        }
        r.extend(ext);
        self.ranges = merge(r);
    }

    fn mode(&self, r: &Range) -> Mode {
        let remembered = self.auto_kinds.contains(&r.kind);
        mode_for(r, &self.modes, remembered, self.auto)
    }

    /// The range of `act` playback is in that is still to be acted on.
    fn current(&self, t: f64, act: Act) -> Option<usize> {
        let tail = if act == Act::Mute { 0.25 } else { 1.0 };
        self.ranges.iter().position(|r| {
            r.act == act
                && t >= r.start
                && t < r.end - tail
                && !self.done.contains(&r.key())
                && !self.allowed.contains(&r.key())
                && self.mode(r) != Mode::Off
        })
    }

    fn button(&self, t: f64) -> Option<Button> {
        if let Some(k) = self.muted.filter(|k| !self.hidden.contains(k)) {
            if let Some(i) = self.ranges.iter().position(|r| r.key() == k) {
                return Some(Button::Unmute(i));
            }
        }
        for act in [Act::Skip, Act::Mute] {
            if let Some(i) = self.current(t, act) {
                if self.mode(&self.ranges[i]) == Mode::Ask {
                    return Some(Button::Act(i));
                }
            }
        }
        match &self.after {
            Some((a, _, until)) if Instant::now() < *until => Some(match a {
                After::Undo => Button::Undo,
                After::Always { show, kind } => Button::Always {
                    show: show.clone(),
                    kind: kind.clone(),
                },
            }),
            _ => None,
        }
    }

    fn render(&self, cx: &Ctx, b: Option<&Button>) {
        let g = cx.app.global::<crate::SkipIntro>();
        let (label, icon) = match b {
            None => (String::new(), "skip"),
            Some(Button::Act(i)) => {
                let r = &self.ranges[*i];
                let end = if r.end.is_infinite() {
                    self.duration
                } else {
                    r.end
                };
                g.set_target(end as f32);
                match r.act {
                    Act::Skip => (format!("Skip {}", r.label), "skip"),
                    Act::Mute => (format!("Mute {}", r.label), "unmute"),
                }
            }
            Some(Button::Unmute(i)) => (format!("Unmute {}", self.ranges[*i].label), "unmute"),
            Some(Button::Undo) => {
                let l = self.after.as_ref().map(|a| a.1.as_str()).unwrap_or("");
                (format!("Skipped {l} · Undo"), "undo")
            }
            Some(Button::Always { kind, .. }) => {
                (format!("Always skip the {kind} of this show"), "always")
            }
        };
        if g.get_label().as_str() != label {
            g.set_label(label.into());
        }
        if g.get_icon().as_str() != icon {
            g.set_icon(icon.into());
        }
    }

    fn has_next(cx: &Ctx) -> bool {
        let n: i64 = cx.player.mpv.get_property("playlist-count").unwrap_or(0);
        let i: i64 = cx.player.mpv.get_property("playlist-pos").unwrap_or(-1);
        (i >= 0 && i + 1 < n) || cx.player.get("loop-playlist").is_some_and(|v| v != "no")
    }

    fn skip(&mut self, cx: &Ctx, i: usize, auto: bool) {
        let r = self.ranges[i].clone();
        let t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(r.start);
        self.done.push(r.key());
        let to_end = r.end.is_infinite() || (self.duration > 0.0 && r.end >= self.duration - 0.5);
        if to_end && Self::has_next(cx) {
            // Credits running to the end: the next file (nothing to undo there).
            self.last = None;
            self.after = None;
            cx.player.playlist_step(true);
            if auto {
                cx.toast(&format!("Skipped {}", r.label));
            }
            return;
        }
        // Last file: to its end (keep-open pauses on the last frame).
        let to = if to_end && self.duration > 0.0 {
            self.duration
        } else {
            r.end
        };
        cx.player.seek_abs(to);
        self.last = Some(Last {
            key: r.key(),
            start: r.start,
            from: t,
            label: r.label.clone(),
        });
        let offer = format!("{}|{}", self.show, r.kind);
        self.after = if auto {
            Some((After::Undo, r.label.clone(), Instant::now() + NOTICE))
        } else if cx.flag("offer")
            && !self.show.is_empty()
            && matches!(r.kind.as_str(), "intro" | "credits")
            && !self.auto_kinds.contains(&r.kind)
            && !self.offered.contains(&offer)
        {
            self.offered.push(offer);
            Some((
                After::Always {
                    show: self.show.clone(),
                    kind: r.kind.clone(),
                },
                r.label.clone(),
                Instant::now() + NOTICE,
            ))
        } else {
            None
        };
    }

    /// Mute inside range `i` (again on every entry, unless undone).
    fn mute(&mut self, cx: &Ctx, i: usize) {
        let k = self.ranges[i].key();
        if cx.player.get("mute").as_deref() == Some("yes") {
            self.done.push(k);
            return; // muted by the user already: leave the sound alone
        }
        cx.player.set("mute", "yes");
        self.muted = Some(k);
    }

    /// Unmute if we muted (file change, leaving the range, Unmute, plugin off).
    fn restore_sound(&mut self, cx: &Ctx) {
        if self.muted.take().is_some() {
            cx.player.set("mute", "no");
        }
    }

    fn undo(&mut self, cx: &Ctx) {
        if let Some(k) = self.muted {
            self.allowed.push(k);
            self.restore_sound(cx);
            cx.toast("Unmuted: the segment plays with sound");
            return;
        }
        let Some(l) = self.last.take() else {
            cx.toast("Nothing to undo");
            return;
        };
        self.allowed.push(l.key);
        self.done.retain(|k| *k != l.key);
        self.after = None;
        cx.player.seek_abs(l.from.min(l.start).max(0.0));
        cx.toast(&format!("Skip undone: playing the {}", l.label));
    }

    /// The Skip button (or Alt+K): whatever it currently shows.
    fn press(&mut self, cx: &Ctx, from_key: bool) {
        let t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(-1.0);
        match self.button(t) {
            Some(Button::Act(i)) => match self.ranges[i].act {
                Act::Skip => self.skip(cx, i, false),
                Act::Mute => self.mute(cx, i),
            },
            Some(Button::Unmute(_)) | Some(Button::Undo) => self.undo(cx),
            Some(Button::Always { show, kind }) => self.always(cx, &show, &kind),
            None => {
                // Alt+K inside a range set to skip automatically but already handled,
                // or one the user dismissed: skip it anyway.
                let i = self.ranges.iter().position(|r| {
                    r.act == Act::Skip
                        && t >= r.start
                        && t < r.end - 1.0
                        && !self.allowed.contains(&r.key())
                });
                match i {
                    Some(i) => self.skip(cx, i, false),
                    None if from_key => cx.toast("Nothing to skip here"),
                    None => {}
                }
            }
        }
        self.render(cx, self.button(t).as_ref());
    }

    fn always(&mut self, cx: &Ctx, show: &str, kind: &str) {
        let mut all = parse_auto_shows(&cx.get("auto-shows"));
        match all.iter_mut().find(|(s, _)| s == show) {
            Some((_, k)) if !k.iter().any(|x| x == kind) => k.push(kind.into()),
            Some(_) => {}
            None => all.push((show.into(), vec![kind.into()])),
        }
        cx.set("auto-shows", &format_auto_shows(&all));
        self.after = None;
        self.reload(cx);
        cx.toast(&format!(
            "The {kind} of “{show}” will be skipped automatically (Settings → Skip Intro & Credits)"
        ));
    }

    fn dismiss(&mut self, cx: &Ctx) {
        let t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(-1.0);
        match self.button(t) {
            Some(Button::Act(i)) => self.done.push(self.ranges[i].key()),
            Some(Button::Unmute(i)) => self.hidden.push(self.ranges[i].key()),
            Some(_) => self.after = None,
            None => {}
        }
        self.render(cx, None);
    }

    fn set_capability(cx: &Ctx, on: bool) {
        if let Err(e) = cx.player.mpv.set_property(CAPABILITY, i64::from(on)) {
            eprintln!("vivi: set {CAPABILITY}: {e:?}");
        }
    }
}

impl Plugin for SkipIntro {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Skip Intro & Credits",
            "One Skip button for intros, credits and the segments extensions such as SponsorBlock find: from chapter names, your per-show ranges or the extension. Skip, ask or off per category, Undo after automatic skips, Alt+K / Alt+Shift+K.",
            "skip",
        )
        .contribute(contrib::PALETTE_COMMAND, "Skip segment", "key")
        .contribute(contrib::PALETTE_COMMAND, "Undo last skip", "undo")
        .contribute(contrib::PALETTE_COMMAND, "Mark intro / credits here", "mark")
        .contribute(contrib::CONTROLBAR_BUTTON, "Mark intro / credits here", "mark")
        .setting(SettingSpec::toggle(
            "auto",
            "Skip automatically",
            "Jump over intros and credits without asking; an Undo button shows for a few seconds (Alt+Shift+K undoes later too). Segments from extensions follow the extension's own setting.",
            false,
        ))
        .setting(SettingSpec::toggle(
            "chapters",
            "Detect from chapter names",
            "Chapters named Intro, Opening, OP, Credits, Ending or ED. Markers like “End of intro” and post-credits scenes are not skipped.",
            true,
        ))
        .setting(SettingSpec::text(
            "modes",
            "Per category or source",
            "skip, ask or off; overrides the rest. Categories: intro, credits and the ones extensions send (sponsor, selfpromo, outro, filler, …). Sources: chapters, show, or an extension id (sponsorblock). Both: sponsorblock:intro=off.",
            "intro=skip credits=ask sponsorblock:filler=off",
            "",
        ))
        .setting(SettingSpec::toggle(
            "offer",
            "Offer to always skip for a show",
            "After you skip an intro or credits by hand, the button offers to skip them automatically for that show from now on.",
            true,
        ))
        .setting(SettingSpec::text(
            "auto-shows",
            "Shows skipped automatically",
            "Filled in by that offer. Format: show name = intro, credits; other show = intro",
            "the wand = intro, credits",
            "",
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
            "Forget this show",
            "Clears its marked ranges and automatic skipping.",
            "Clear",
        ))
    }

    fn enable(&mut self, cx: &Ctx) {
        Self::set_capability(cx, true);
        self.reload(cx);
    }
    fn disable(&mut self, cx: &Ctx) {
        // Packages fall back to their own skipping while the button is off.
        Self::set_capability(cx, false);
        self.restore_sound(cx);
        self.ranges.clear();
        self.submitted.clear();
        cx.app.global::<crate::SkipIntro>().set_label("".into());
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.mark = None;
        // A new start even when the same file plays again (ranges it submitted already
        // for this path stay).
        self.path.clear();
        self.reload(cx);
    }
    fn setting_changed(&mut self, cx: &Ctx, _key: &str) {
        self.reload(cx);
    }

    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        match name {
            // `vivi-skip-ranges <source> <json>` / `vivi-skip-ranges-clear <source>`.
            "ranges" | "ranges-clear" => {
                let Some(source) = args.first().map(|s| s.trim().to_ascii_lowercase()) else {
                    return;
                };
                if source.is_empty() || source == CHAPTERS || source == SHOW {
                    eprintln!("vivi: vivi-skip-ranges: invalid source {source:?}");
                    return;
                }
                let list = match (name, args.get(1)) {
                    ("ranges", Some(json)) => match parse_submitted(&source, json) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("vivi: vivi-skip-ranges {source}: bad json: {e}");
                            return;
                        }
                    },
                    _ => Vec::new(),
                };
                let path = cx.player.get("path").unwrap_or_default();
                if path != self.path {
                    self.reload(cx);
                }
                eprintln!("vivi: {} skip range(s) from {source}", list.len());
                self.submitted.retain(|(s, _, _)| *s != source);
                if !list.is_empty() {
                    self.submitted.push((source, path, list));
                }
                self.reload(cx);
                return;
            }
            "skip" => return self.press(cx, false),
            "key" => return self.press(cx, true),
            "undo" => {
                self.undo(cx);
                return;
            }
            "dismiss" => return self.dismiss(cx),
            _ => {}
        }
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
                let mut auto = parse_auto_shows(&cx.get("auto-shows"));
                auto.retain(|(s, _)| *s != show);
                cx.set("auto-shows", &format_auto_shows(&auto));
                self.offered.retain(|o| !o.starts_with(&format!("{show}|")));
                cx.toast(&format!(
                    "Forgot ranges and automatic skipping for “{show}”"
                ));
            }
            _ => return,
        }
        cx.set("ranges", &format_user(&all));
        self.reload(cx);
    }

    fn tick(&mut self, cx: &Ctx) {
        // Chapters added later (SponsorBlock, scene detection), another file, or the
        // duration of a stream known only now.
        let n: i64 = cx
            .player
            .mpv
            .get_property("chapter-list/count")
            .unwrap_or(0);
        let stale = n != self.chapter_count
            || cx.player.get("path").unwrap_or_default() != self.path
            || (self.duration <= 0.0
                && cx.player.mpv.get_property::<f64>("duration").unwrap_or(0.0) > 0.0);
        if stale {
            self.reload(cx);
        }
        let Ok(t) = cx.player.mpv.get_property::<f64>("time-pos") else {
            self.render(cx, None);
            return;
        };
        // Sound back after the range we muted.
        if let Some(k) = self.muted {
            let inside = self
                .ranges
                .iter()
                .any(|r| r.key() == k && t >= r.start && t < r.end);
            if !inside {
                self.restore_sound(cx);
            }
        }
        if let Some(i) = self.current(t, Act::Skip) {
            if self.mode(&self.ranges[i]) == Mode::Skip {
                self.skip(cx, i, true);
            }
        }
        if self.muted.is_none() {
            if let Some(i) = self.current(t, Act::Mute) {
                if self.mode(&self.ranges[i]) == Mode::Skip {
                    self.mute(cx, i);
                }
            }
        }
        let b = self.button(t);
        if b.is_none() && self.after.as_ref().is_some_and(|a| Instant::now() >= a.2) {
            self.after = None;
        }
        self.render(cx, b.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(v: &[(&str, f64)]) -> Vec<(String, f64)> {
        v.iter().map(|(t, s)| (t.to_string(), *s)).collect()
    }

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
    fn end_markers_are_not_ranges() {
        for t in [
            "End of intro",
            "End of outro",
            "End of Opening",
            "End of muted sponsor",
            "Intro End",
            "Credits end",
            "end: intro",
            "Post-credits scene",
            "Mid-credits",
            "After the credits",
            "Opening scene",
        ] {
            assert_eq!(classify(t), None, "{t}");
        }
        assert!(is_end_marker("End of intro"));
        assert!(!is_end_marker("End Credits"));
        assert!(!is_end_marker("Ending"));
        // SponsorBlock 1.2 chapters on a file: the intro runs to "End of intro" only.
        let r = chapter_ranges(
            &ch(&[
                ("Chapter 1", 0.0),
                ("Intro", 5.0),
                ("End of intro", 35.0),
                ("Outro", 500.0),
                ("End of outro", 560.0),
            ]),
            600.0,
        );
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].start, r[0].end, r[0].kind.as_str()),
            (5.0, 35.0, "intro")
        );
        assert_eq!(
            (r[1].start, r[1].end, r[1].kind.as_str()),
            (500.0, 560.0, "credits")
        );
    }

    #[test]
    fn ranges_from_chapters() {
        let r = chapter_ranges(
            &ch(&[
                ("Prologue", 0.0),
                ("Opening", 60.0),
                ("Part A", 150.0),
                ("Ending", 1300.0),
            ]),
            1420.0,
        );
        assert_eq!(r.len(), 2);
        assert_eq!(
            (r[0].start, r[0].end, r[0].kind.as_str()),
            (60.0, 150.0, "intro")
        );
        assert_eq!(
            (r[1].start, r[1].end, r[1].kind.as_str()),
            (1300.0, 1420.0, "credits")
        );
        assert_eq!(r[0].label, "intro");
        assert_eq!(r[0].source, CHAPTERS);
    }

    #[test]
    fn submitted_ranges() {
        let r = parse_submitted(
            "sponsorblock",
            r#"[{"start":10,"end":40,"category":"sponsor","label":"sponsor","action":"skip"},
                {"start":50,"end":60,"kind":"selfpromo","label":"Skip self-promotion","action":"mute"},
                {"start":70,"end":80,"category":"music_offtopic","action":"ask"},
                {"start":90,"end":95,"action":"off"},
                {"start":9,"end":3},
                {"start":100,"end":110}]"#,
        )
        .unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(
            (r[0].act, r[0].wish, r[0].label.as_str()),
            (Act::Skip, Some(Mode::Skip), "sponsor")
        );
        assert_eq!(
            (r[1].act, r[1].label.as_str()),
            (Act::Mute, "self-promotion")
        );
        assert_eq!(
            (r[2].wish, r[2].label.as_str()),
            (Some(Mode::Ask), "music offtopic")
        );
        assert_eq!((r[3].kind.as_str(), r[3].wish), ("segment", None));
        assert_eq!(r[0].source, "sponsorblock");
        assert!(parse_submitted("x", "{}").is_err());
        assert!(parse_submitted("x", "[]").unwrap().is_empty());
    }

    #[test]
    fn merging_shows_one_button_per_segment() {
        let sb = parse_submitted(
            "sponsorblock",
            r#"[{"start":61,"end":149,"category":"intro","label":"intro","action":"skip"},
                {"start":300,"end":330,"category":"sponsor","label":"sponsor"},
                {"start":300,"end":330,"category":"sponsor","label":"sponsor","action":"mute"}]"#,
        )
        .unwrap();
        let mut all = chapter_ranges(&ch(&[("Opening", 60.0), ("Part A", 150.0)]), 1400.0);
        all.extend(user_ranges("show = 20:00-end", "show", 1400.0));
        all.extend(user_ranges("show = 1:01-2:30", "show", 1400.0));
        all.extend(sb);
        let m = merge(all);
        // Opening (chapters), the SponsorBlock intro and the per-show 1:01-2:30 are one.
        let intros: Vec<&Range> = m.iter().filter(|r| r.start < 200.0).collect();
        assert_eq!(intros.len(), 1);
        assert_eq!(intros[0].source, SHOW);
        // Skip and mute of the same time are different things.
        assert_eq!(m.iter().filter(|r| r.start == 300.0).count(), 2);
        assert_eq!(m.last().unwrap().kind, "credits");
        // A short overlap is two segments.
        let a = Range::part(0.0, 100.0, CHAPTERS, Part::Intro);
        let b = Range::part(90.0, 200.0, SHOW, Part::Intro);
        assert_eq!(merge(vec![a, b]).len(), 2);
    }

    #[test]
    fn sponsorblock_chapter_markers_ignored_when_submitted() {
        let sb = parse_submitted(
            "sponsorblock",
            r#"[{"start":5,"end":35,"category":"intro","action":"ask"}]"#,
        )
        .unwrap();
        let chapters = ch(&[
            ("Opening", 0.0),
            ("Intro", 5.0),
            ("End of intro", 35.0),
            ("Part A", 90.0),
        ]);
        let left = without_markers(&chapters, &sb);
        assert_eq!(left, ch(&[("Opening", 0.0), ("Part A", 90.0)]));
        let r = chapter_ranges(&left, 1000.0);
        assert_eq!((r[0].start, r[0].end), (0.0, 90.0));
    }

    #[test]
    fn modes() {
        let m = parse_modes(
            "intro=skip, sponsorblock:filler=off; credits=ASK sponsorblock=ask bad=maybe =x",
        );
        assert_eq!(m.len(), 4);
        let intro = Range::part(0.0, 10.0, CHAPTERS, Part::Intro);
        let credits = Range::part(0.0, 10.0, SHOW, Part::Credits);
        assert_eq!(mode_for(&intro, &m, false, false), Mode::Skip);
        assert_eq!(mode_for(&credits, &m, false, true), Mode::Ask);
        assert_eq!(
            mode_for(&credits, &m, true, false),
            Mode::Skip,
            "remembered for the show"
        );
        let mut seg = parse_submitted(
            "sponsorblock",
            r#"[{"start":1,"end":5,"category":"filler","action":"skip"},
                {"start":6,"end":9,"category":"sponsor","action":"skip"},
                {"start":10,"end":19,"category":"intro","action":"ask"}]"#,
        )
        .unwrap();
        assert_eq!(
            mode_for(&seg[0], &m, false, false),
            Mode::Off,
            "source:kind"
        );
        assert_eq!(mode_for(&seg[1], &m, false, false), Mode::Ask, "source");
        assert_eq!(
            mode_for(&seg[2], &m, false, false),
            Mode::Skip,
            "kind before source"
        );
        assert_eq!(
            mode_for(&seg[1], &[], false, false),
            Mode::Skip,
            "the plugin's wish"
        );
        seg[1].wish = None;
        assert_eq!(mode_for(&seg[1], &[], false, true), Mode::Skip, "auto");
        assert_eq!(mode_for(&seg[1], &[], false, false), Mode::Ask);
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
        assert_eq!(r[0].kind, "intro");
        assert_eq!(r[1].kind, "credits");
        assert_eq!(r[1].end, 1400.0);
        let a = parse_auto_shows("The Wand = intro, Credits; x = ; other=intro");
        assert_eq!(
            a,
            vec![
                (
                    "the wand".to_string(),
                    vec!["intro".to_string(), "credits".to_string()]
                ),
                ("other".to_string(), vec!["intro".to_string()]),
            ]
        );
        assert_eq!(
            format_auto_shows(&a),
            "the wand = intro, credits; other = intro"
        );
    }
}
