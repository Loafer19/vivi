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

mod plugin;
mod ranges;
pub use ranges::*;

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

#[cfg(test)]
mod tests;
