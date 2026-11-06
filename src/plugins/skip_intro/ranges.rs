//! Skip ranges: chapter names, submitted and user ranges, modes, merging (pure functions).

use super::*;

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
    pub(crate) fn parse(s: &str) -> Option<Mode> {
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
pub(crate) type Key = (i64, i64, bool);

impl Range {
    pub(crate) fn key(&self) -> Key {
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
    pub(crate) fn priority(&self) -> u8 {
        match self.source.as_str() {
            SHOW => 0,
            CHAPTERS => 2,
            _ => 1,
        }
    }
    pub(crate) fn part(start: f64, end: f64, source: &str, part: Part) -> Range {
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
pub(crate) struct Submitted {
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
pub(crate) fn same_segment(a: &Range, b: &Range) -> bool {
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

pub(crate) fn user_ranges(spec: &str, show: &str, duration: f64) -> Vec<Range> {
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
