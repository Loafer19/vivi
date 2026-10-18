//! Lyrics: synced (LRC, incl. enhanced word timing) lyrics shown as a karaoke overlay or
//! a full-screen karaoke view with word-level highlight.
//!
//! Sources, best first (a synced source always beats a plain one; a manual pick beats
//! everything): the manual pick from *Search lyrics…*, a `.lrc` file next to the media
//! file, lyrics embedded in the tags (ID3 SYLT / USLT, Vorbis/FLAC `LYRICS`, MP4 `©lyr`,
//! as written by the Auto-tagger), the on-disk cache (`$XDG_CACHE_HOME/vivi/lyrics`),
//! then LRCLIB (<https://lrclib.net>) using artist / title / album / duration from the
//! tags (lofty) or mpv metadata. When LRCLIB only has plain lyrics they are shown as a
//! scrollable block.
//!
//! A manual timing offset (`[` / `]` ±0.1 s, with Shift ±0.5 s) is saved per track.
//! Fetched lyrics can be saved as `song.lrc` or embedded in the tags (Auto-tagger code,
//! with a tag backup).
//!
//! Network streams (radio): whenever the stream title (`icy-title`, else `media-title`,
//! usually "Artist - Title") changes, it is parsed and looked up on LRCLIB; ads, jingles
//! and station IDs are skipped and results are cached (memory + disk). Plain lyrics are
//! shown as a scrollable block, since the song's start time is unknown. Optionally
//! ("Radio lyrics: Approximate sync") synced lyrics are timed from the moment the title
//! changed, shifted by a manual offset.

use super::{contrib, Ctx, Manifest, Plugin, SettingSpec};
use slint::ComponentHandle;
use slint::{ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod lrc;
mod radio;
mod sources;
pub use lrc::*;
pub use radio::*;
pub use sources::*;

pub const ID: &str = "lyrics";

/// Rows of the words of a line for the karaoke view, at most `cols` characters each.
pub fn wrap_rows(words: &[&str], cols: usize) -> Vec<i32> {
    let mut rows = Vec::with_capacity(words.len());
    let (mut row, mut used) = (0, 0usize);
    for w in words {
        let n = w.trim().chars().count();
        let need = if used == 0 { n } else { used + n + 1 };
        if used > 0 && need > cols.max(8) {
            row += 1;
            used = n;
        } else {
            used = need;
        }
        rows.push(row);
    }
    rows
}

/// Manual search dialog state.
#[derive(Default)]
struct Search {
    results: Vec<LrclibItem>,
    task: crate::util::Task<Result<Vec<LrclibItem>, String>>,
}

fn mmss(s: f64) -> String {
    let s = s.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Default)]
pub struct Lyrics {
    lrc: Option<Lrc>,
    index: Option<usize>,
    task: crate::util::Task<(String, Option<String>)>,
    path: Option<String>,
    /// Some while a network stream plays.
    radio: Option<Radio>,
    /// Synced lines are timed from the title change (radio, approximate sync).
    radio_synced: bool,
    /// Lyrics being shown and where they come from.
    current: Option<Found>,
    track: Option<Track>,
    /// Key for the per-track offset and manual pick (file path or radio song).
    key: String,
    offset: f64,
    words: Option<std::rc::Rc<VecModel<crate::KWord>>>,
    word_line: Option<usize>,
    word_cols: i32,
    search: Search,
}

impl Lyrics {
    fn show(&mut self, cx: &Ctx, lrc: Option<Lrc>) {
        self.index = None;
        self.radio_synced = false;
        self.word_line = None;
        self.words = None;
        cx.app.global::<crate::Lyrics>().set_plain(false);
        cx.app
            .global::<crate::Lyrics>()
            .set_words(ModelRc::new(VecModel::<crate::KWord>::default()));
        cx.app.global::<crate::Lyrics>().set_rows(0);
        let lines: Vec<SharedString> = lrc
            .as_ref()
            .map(|l| l.lines.iter().map(|(_, s)| s.as_str().into()).collect())
            .unwrap_or_default();
        cx.app
            .global::<crate::Lyrics>()
            .set_lines(ModelRc::new(VecModel::from(lines)));
        cx.app.global::<crate::Lyrics>().set_index(-1);
        self.lrc = lrc.filter(|l| !l.lines.is_empty());
        if self.lrc.is_none() {
            self.current = None;
        }
        self.update_info(cx);
    }

    /// Show `f` (synced → karaoke, plain → scrollable block).
    fn show_found(&mut self, cx: &Ctx, f: Option<Found>) {
        let Some(f) = f else {
            self.show(cx, None);
            return;
        };
        let lrc = Lrc::parse(&f.text);
        if lrc.lines.is_empty() {
            let lines = f
                .text
                .lines()
                .map(|l| (0.0, l.trim().to_string()))
                .collect();
            self.show(
                cx,
                Some(Lrc {
                    lines,
                    ..Default::default()
                }),
            );
            cx.app.global::<crate::Lyrics>().set_plain(true);
        } else {
            self.show(cx, Some(lrc));
        }
        self.current = self.lrc.is_some().then_some(f);
        self.update_info(cx);
    }

    fn plain(&self) -> bool {
        self.current.as_ref().is_some_and(|c| !c.synced())
            || self.radio.is_some() && !self.radio_synced
    }

    /// Source, offset and track labels for the karaoke view and the side panel.
    fn update_info(&self, cx: &Ctx) {
        let state = match &self.current {
            Some(_) => crate::LyricsState::Found,
            None if self.task.is_running()
                || self.radio.as_ref().is_some_and(|r| r.task.is_running()) =>
            {
                crate::LyricsState::Searching
            }
            None => crate::LyricsState::Missing,
        };
        cx.app.global::<crate::Lyrics>().set_state(state);
        let src = match &self.current {
            Some(c) => format!(
                "{} · {}",
                c.source.label(),
                if !c.synced() {
                    "plain"
                } else if self.lrc.as_ref().is_some_and(|l| l.has_words()) {
                    "word-synced"
                } else {
                    "synced"
                }
            ),
            None if self.task.is_running()
                || self.radio.as_ref().is_some_and(|r| r.task.is_running()) =>
            {
                "Searching LRCLIB…".into()
            }
            None => "No lyrics found".into(),
        };
        cx.app.global::<crate::Lyrics>().set_source(src.into());
        cx.app.global::<crate::Lyrics>().set_manual(
            self.current
                .as_ref()
                .is_some_and(|c| c.source == Source::Manual),
        );
        cx.app
            .global::<crate::Lyrics>()
            .set_offset(self.offset as f32);
        cx.app
            .global::<crate::Lyrics>()
            .set_offset_label(offset_label(self.offset).into());
        let (artist, title) = match (
            &self.track,
            self.radio.as_ref().and_then(|r| r.song.clone()),
        ) {
            (_, Some((a, t))) => (a, t),
            (Some(t), _) => (t.artist.clone(), t.title.clone()),
            _ => (
                String::new(),
                cx.player.get("media-title").unwrap_or_default(),
            ),
        };
        cx.app.global::<crate::Lyrics>().set_artist(artist.into());
        cx.app.global::<crate::Lyrics>().set_title(title.into());
        cx.app.global::<crate::Lyrics>().set_can_save(
            self.current.is_some() && self.radio.is_none() && cx.local_path().is_some(),
        );
    }

    fn apply_size(&self, cx: &Ctx) {
        cx.app
            .global::<crate::Lyrics>()
            .set_size(match cx.get("size").as_str() {
                "Small" => 16.0,
                "Large" => 30.0,
                _ => 22.0,
            });
    }

    fn set_key(&mut self, cx: &Ctx, key: String) {
        self.offset = load_offsets(&offsets_file())
            .get(&key)
            .copied()
            .unwrap_or(0.0);
        self.key = key;
        self.update_info(cx);
    }

    fn load(&mut self, cx: &Ctx) {
        self.task.cancel();
        self.track = None;
        self.show(cx, None);
        self.path = cx.player.get("path");
        self.radio = None;
        self.set_key(cx, self.path.clone().unwrap_or_default());
        let stream = self
            .path
            .as_deref()
            .is_some_and(|p| p.contains("://") && !p.starts_with("file://"));
        if stream {
            // Stream titles are polled in `tick`.
            self.radio = Some(Radio::default());
            return;
        }
        let mut found = Vec::new();
        if let Ok(s) = std::fs::read_to_string(manual_file(&self.key)) {
            found.extend(Found::new(Source::Manual, s));
        }
        let local = cx.local_path();
        if let Some(p) = &local {
            if let Some(l) = local_lrc(p).and_then(|f| std::fs::read_to_string(f).ok()) {
                found.extend(Found::new(Source::LocalFile, l));
            }
        }
        // Tags and online lookup only for music (no real video track).
        let music = crate::viz::audio_only(cx.player);
        if let (Some(p), true) = (&local, music) {
            found.extend(embedded(p).and_then(|s| Found::new(Source::Embedded, s)));
        }
        self.track = if music { track_info(cx) } else { None };
        let cache = self.track.as_ref().map(cache_file);
        if let Some(c) = &cache {
            found.extend(read_cache(c).and_then(|s| Found::new(Source::Cache, s)));
        }
        let best = choose(&found).cloned();
        let done = best
            .as_ref()
            .is_some_and(|b| b.source == Source::Manual || b.synced());
        self.show_found(cx, best);
        let (Some(t), Some(cache)) = (self.track.clone(), cache) else {
            return;
        };
        if done || !cx.flag("online") || cache.with_extension("missing").exists() {
            return;
        }
        // Cached plain lyrics: no need to ask again.
        if found.iter().any(|f| f.source == Source::Cache) {
            return;
        }
        let key = self.path.clone().unwrap_or_default();
        self.task = crate::util::Task::spawn(move || {
            let res = fetch_lrclib(&t);
            match &res {
                Ok(s) => write_cache(&cache, s.as_deref()),
                Err(e) => eprintln!("vivi: lyrics: {e}"),
            }
            (key, res.ok().flatten())
        });
        self.update_info(cx);
    }

    /// Online result: shown unless a better local source is already displayed.
    fn online_result(&mut self, cx: &Ctx, text: Option<String>) {
        let Some(online) = text.and_then(|t| Found::new(Source::Online, t)) else {
            self.update_info(cx);
            return;
        };
        let mut all: Vec<Found> = self.current.iter().cloned().collect();
        all.push(online.clone());
        if choose(&all) == Some(&online) {
            self.show_found(cx, Some(online));
            match cx.get("save-mode").as_str() {
                "Save .lrc next to the song" => self.save_lrc(cx, true),
                "Embed in the file's tags" => self.embed(cx, true),
                _ => {}
            }
        } else {
            self.update_info(cx);
        }
    }

    fn set_offset(&mut self, cx: &Ctx, o: f64) {
        self.offset = clamp_offset(o);
        if !self.key.is_empty() {
            save_offset(&offsets_file(), &self.key, self.offset);
        }
        self.update_info(cx);
        self.index = None; // re-sync on the next tick
        self.word_line = None;
        cx.toast(&format!("Lyrics offset {}", offset_label(self.offset)));
    }

    /// Text to save: the shown lyrics with the user offset baked in.
    fn save_text(&self) -> Option<String> {
        let c = self.current.as_ref()?;
        Some(with_offset(&c.text, self.offset))
    }

    fn save_lrc(&mut self, cx: &Ctx, auto: bool) {
        let (Some(p), Some(text)) = (cx.local_path(), self.save_text()) else {
            cx.toast("No lyrics to save");
            return;
        };
        let out = p.with_extension("lrc");
        if auto && out.exists() {
            return;
        }
        if out.exists() {
            let _ = std::fs::copy(&out, out.with_extension("lrc.bak"));
        }
        match std::fs::write(&out, &text) {
            Ok(()) => {
                let name = out.file_name().unwrap_or_default().to_string_lossy();
                self.baked(cx);
                cx.toast(&format!("Saved {name}"));
            }
            Err(e) => cx.toast(&format!("Could not save lyrics: {e}")),
        }
    }

    fn embed(&mut self, cx: &Ctx, _auto: bool) {
        let (Some(p), Some(text)) = (cx.local_path(), self.save_text()) else {
            cx.toast("No lyrics to embed");
            return;
        };
        let _ = super::auto_tagger::backup(&super::auto_tagger::backup_dir(), &p);
        let tags = super::auto_tagger::Tags {
            lyrics: text,
            ..Default::default()
        };
        match super::auto_tagger::write_tags(&p, &tags, None) {
            Ok(()) => {
                self.baked(cx);
                cx.toast("Lyrics embedded in the file's tags");
            }
            Err(e) => cx.toast(&format!("Could not embed lyrics: {e}")),
        }
    }

    /// After saving with the offset baked in, the saved copy needs no extra offset.
    fn baked(&mut self, cx: &Ctx) {
        if let Some(c) = self.current.as_mut() {
            c.text = with_offset(&c.text, self.offset);
        }
        if self.offset != 0.0 {
            self.offset = 0.0;
            save_offset(&offsets_file(), &self.key, 0.0);
            let cur = self.current.clone();
            self.show_found(cx, cur);
        }
        self.update_info(cx);
    }

    // ---------- manual search ----------

    fn search_open(&mut self, cx: &Ctx) {
        let (a, t) = match (
            self.radio.as_ref().and_then(|r| r.song.clone()),
            &self.track,
        ) {
            (Some(s), _) => s,
            (None, Some(t)) => (t.artist.clone(), t.title.clone()),
            _ => (
                String::new(),
                cx.player
                    .get("media-title")
                    .or_else(|| self.path.as_deref().map(crate::store::display_name))
                    .unwrap_or_default(),
            ),
        };
        cx.app.global::<crate::Lyrics>().set_search_artist(a.into());
        cx.app.global::<crate::Lyrics>().set_search_title(t.into());
        cx.app
            .global::<crate::Lyrics>()
            .set_search_preview("".into());
        cx.app.global::<crate::Lyrics>().set_search_selected(-1);
        self.search.results.clear();
        self.search_results(cx);
        cx.app.global::<crate::Overlays>().set_ls_open(true);
        if !cx
            .app
            .global::<crate::Lyrics>()
            .get_search_title()
            .is_empty()
        {
            self.search_run(cx);
        } else {
            cx.app
                .global::<crate::Lyrics>()
                .set_search_status("Enter an artist and a title.".into());
        }
    }

    fn search_run(&mut self, cx: &Ctx) {
        let artist = cx
            .app
            .global::<crate::Lyrics>()
            .get_search_artist()
            .trim()
            .to_string();
        let title = cx
            .app
            .global::<crate::Lyrics>()
            .get_search_title()
            .trim()
            .to_string();
        if title.is_empty() {
            cx.app
                .global::<crate::Lyrics>()
                .set_search_status("Enter a title.".into());
            return;
        }
        cx.app.global::<crate::Lyrics>().set_search_busy(true);
        cx.app.global::<crate::Lyrics>().set_search_selected(-1);
        cx.app
            .global::<crate::Lyrics>()
            .set_search_preview("".into());
        cx.app
            .global::<crate::Lyrics>()
            .set_search_status("Searching LRCLIB…".into());
        self.search.task = crate::util::Task::spawn(move || search_lrclib(&artist, &title));
    }

    fn duration(&self, cx: &Ctx) -> f64 {
        self.track
            .as_ref()
            .map(|t| t.duration)
            .filter(|d| *d > 0.0)
            .unwrap_or_else(|| cx.player.mpv.get_property("duration").unwrap_or(0.0))
    }

    fn search_results(&mut self, cx: &Ctx) {
        let dur = if self.radio.is_some() {
            0.0
        } else {
            self.duration(cx)
        };
        let hits: Vec<crate::LyricsHit> = self
            .search
            .results
            .iter()
            .map(|i| {
                let d = i.duration.unwrap_or(0.0);
                crate::LyricsHit {
                    title: i.title.clone().unwrap_or_default().into(),
                    artist: i.artist.clone().unwrap_or_default().into(),
                    album: i.album.clone().unwrap_or_default().into(),
                    duration: if d > 0.0 { mmss(d).into() } else { "".into() },
                    synced: non_empty(&i.synced).is_some(),
                    close: dur > 0.0 && d > 0.0 && (d - dur).abs() <= 3.0,
                }
            })
            .collect();
        cx.app
            .global::<crate::Lyrics>()
            .set_search_results(ModelRc::new(VecModel::from(hits)));
    }

    fn search_tick(&mut self, cx: &Ctx) {
        let Some(res) = self.search.task.poll() else {
            return;
        };
        cx.app.global::<crate::Lyrics>().set_search_busy(false);
        match res {
            Ok(mut items) => {
                let dur = if self.radio.is_some() {
                    0.0
                } else {
                    self.duration(cx)
                };
                rank(&mut items, dur);
                let n = items.len();
                self.search.results = items;
                cx.app.global::<crate::Lyrics>().set_search_status(
                    match n {
                        0 => "No lyrics found. Try another spelling, or only the title.".into(),
                        1 => "1 result".into(),
                        n => format!("{n} results · synced first, then closest duration"),
                    }
                    .into(),
                );
            }
            Err(e) => {
                self.search.results.clear();
                cx.app
                    .global::<crate::Lyrics>()
                    .set_search_status(format!("Search failed: {e}").into());
            }
        }
        self.search_results(cx);
    }

    fn search_select(&mut self, cx: &Ctx, i: usize) {
        let Some(it) = self.search.results.get(i) else {
            return;
        };
        let text = match non_empty(&it.synced) {
            Some(s) => Lrc::parse(s).texts().join("\n"),
            None => it.plain.clone().unwrap_or_default(),
        };
        cx.app
            .global::<crate::Lyrics>()
            .set_search_preview(text.trim().into());
    }

    /// Use result `i` (`plain`: its plain lyrics even when synced ones exist).
    fn search_use(&mut self, cx: &Ctx, i: usize, plain: bool) {
        let Some(it) = self.search.results.get(i).cloned() else {
            return;
        };
        let text = if plain {
            non_empty(&it.plain)
                .map(String::from)
                .or_else(|| non_empty(&it.synced).map(|s| Lrc::parse(s).texts().join("\n")))
        } else {
            it.best_text()
        };
        let Some(f) = text.and_then(|t| Found::new(Source::Manual, t)) else {
            return;
        };
        let file = manual_file(&self.key);
        if let Some(d) = file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&file, &f.text);
        cx.app.global::<crate::Overlays>().set_ls_open(false);
        if let Some(r) = self.radio.as_mut() {
            r.mem.remove(&r.key);
        }
        let synced = f.synced();
        self.show_found(cx, Some(f));
        if synced && self.radio.is_some() {
            self.radio_synced = cx.get("radio-mode") == "Approximate sync";
            if !self.radio_synced {
                // Radio without approximate sync: plain block.
                let cur = self.current.clone();
                let lines = self.lrc.as_ref().map(|l| l.texts()).unwrap_or_default();
                self.show(
                    cx,
                    Some(Lrc {
                        lines: lines.into_iter().map(|l| (0.0, l)).collect(),
                        ..Default::default()
                    }),
                );
                cx.app.global::<crate::Lyrics>().set_plain(true);
                self.current = cur;
                self.update_info(cx);
            }
        }
        cx.toast("Lyrics saved for this track");
    }

    // ---------- karaoke words ----------

    /// Current-line words for the karaoke view: rebuilt when the line changes, progress
    /// updated in place so the wipe animates smoothly.
    fn update_words(&mut self, cx: &Ctx, t: f64) {
        let Some(l) = &self.lrc else { return };
        let cols = cx.app.get_karaoke_cols();
        let i = self
            .index
            .filter(|_| cx.app.global::<crate::Overlays>().get_karaoke_open() && l.has_words());
        let Some(i) = i else {
            if self.word_line.take().is_some() {
                self.words = None;
                cx.app
                    .global::<crate::Lyrics>()
                    .set_words(ModelRc::new(VecModel::<crate::KWord>::default()));
                cx.app.global::<crate::Lyrics>().set_rows(0);
            }
            return;
        };
        let ws: Vec<&(f64, String)> = l.words[i].iter().filter(|w| !w.1.is_empty()).collect();
        if self.word_line != Some(i) || self.words.is_none() || self.word_cols != cols {
            let texts: Vec<&str> = ws.iter().map(|w| w.1.as_str()).collect();
            let rows = wrap_rows(&texts, cols.max(1) as usize);
            let m: Vec<crate::KWord> = ws
                .iter()
                .zip(&rows)
                .map(|(w, r)| crate::KWord {
                    text: w.1.trim().into(),
                    gap: w.1.ends_with(char::is_whitespace),
                    row: *r,
                    progress: 0.0,
                })
                .collect();
            let model = std::rc::Rc::new(VecModel::from(m));
            cx.app
                .global::<crate::Lyrics>()
                .set_words(ModelRc::from(model.clone()));
            cx.app
                .global::<crate::Lyrics>()
                .set_rows(rows.last().map(|r| r + 1).unwrap_or(0));
            self.words = Some(model);
            self.word_line = Some(i);
            self.word_cols = cols;
        }
        let Some(model) = &self.words else { return };
        let (k, p) = l.word_at(i, t).unwrap_or((0, 0.0));
        // Index among the non-empty words.
        let all = &l.words[i];
        let mut n = 0;
        use slint::Model;
        for (j, w) in all.iter().enumerate() {
            if w.1.is_empty() {
                continue;
            }
            let prog = if j < k {
                1.0
            } else if j == k {
                p as f32
            } else {
                0.0
            };
            if let Some(mut row) = model.row_data(n) {
                if (row.progress - prog).abs() > 0.001 {
                    row.progress = prog;
                    model.set_row_data(n, row);
                }
            }
            n += 1;
        }
    }
}

impl Lyrics {
    /// Poll the stream title (twice a second) and look lyrics up when it changes.
    fn radio_tick(&mut self, cx: &Ctx) {
        let Some(r) = self.radio.as_mut() else { return };
        if r.last_poll
            .is_some_and(|t| t.elapsed() < Duration::from_millis(500))
        {
            return;
        }
        r.last_poll = Some(std::time::Instant::now());
        let raw = cx
            .player
            .get("metadata/by-key/icy-title")
            .filter(|s| !s.trim().is_empty())
            .or_else(|| cx.player.get("media-title"))
            .unwrap_or_default();
        if raw == r.title {
            return;
        }
        r.title = raw.clone();
        r.since = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        r.task.cancel();
        let station = cx
            .player
            .get("metadata/by-key/icy-name")
            .unwrap_or_default();
        let parsed = parse_stream_title(&raw, &station);
        let key = parsed
            .as_ref()
            .map(|(a, t)| crate::plugins::protocol::join(&[a, t]))
            .unwrap_or_default();
        r.key = key.clone();
        r.song = parsed.clone();
        let Some((artist, title)) = parsed else {
            self.key.clear();
            self.show(cx, None); // ad / station ID / talk: hide
            return;
        };
        self.set_key(cx, crate::plugins::protocol::join(&["radio", key.as_str()]));
        if let Ok(s) = std::fs::read_to_string(manual_file(&self.key)) {
            let it = if is_synced(&s) {
                LrclibItem {
                    synced: Some(s),
                    ..Default::default()
                }
            } else {
                LrclibItem {
                    plain: Some(s),
                    ..Default::default()
                }
            };
            self.show_radio(cx, Some(&it), Source::Manual);
            return;
        }
        let r = self.radio.as_mut().expect("radio");
        if let Some(hit) = r.mem.get(&key).cloned() {
            self.show_radio(cx, hit.as_ref(), Source::Cache);
            return;
        }
        let cache = radio_cache_file(&artist, &title);
        if let Ok(s) = std::fs::read_to_string(&cache) {
            if let Ok(it) = serde_json::from_str::<LrclibItem>(&s) {
                r.mem.insert(key, Some(it.clone()));
                self.show_radio(cx, Some(&it), Source::Cache);
                return;
            }
        }
        let offline = !cx.flag("online") || cache.with_extension("none").exists();
        if offline {
            r.mem.insert(key.clone(), None);
        }
        if !offline {
            r.task = crate::util::Task::spawn(move || {
                let res = fetch_radio(&artist, &title);
                if let Some(d) = cache.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match &res {
                    Ok(Some(it)) => {
                        if let Ok(j) = serde_json::to_string(it) {
                            let _ = std::fs::write(&cache, j);
                        }
                    }
                    Ok(None) => {
                        let _ = std::fs::write(cache.with_extension("none"), b"");
                    }
                    Err(e) => eprintln!("vivi: radio lyrics: {e}"),
                }
                (key, res.ok().flatten())
            });
        }
        self.show(cx, None);
    }

    fn radio_result(&mut self, cx: &Ctx) {
        let Some(r) = self.radio.as_mut() else { return };
        let Some((key, it)) = r.task.poll() else {
            return;
        };
        r.mem.insert(key.clone(), it.clone());
        if key == r.key {
            self.show_radio(cx, it.as_ref(), Source::Online);
        }
    }

    fn show_radio(&mut self, cx: &Ctx, it: Option<&LrclibItem>, source: Source) {
        let Some(it) = it.filter(|i| i.has_lyrics()) else {
            self.show(cx, None);
            return;
        };
        let synced = non_empty(&it.synced)
            .map(Lrc::parse)
            .filter(|l| !l.lines.is_empty());
        let text = it.best_text().unwrap_or_default();
        match synced {
            Some(l) if cx.get("radio-mode") == "Approximate sync" => {
                self.show(cx, Some(l));
                self.radio_synced = true;
            }
            _ => {
                let lines: Vec<(f64, String)> =
                    plain_lines(it).into_iter().map(|l| (0.0, l)).collect();
                self.show(
                    cx,
                    Some(Lrc {
                        lines,
                        ..Default::default()
                    }),
                );
                cx.app.global::<crate::Lyrics>().set_plain(true);
            }
        }
        self.current = Found::new(source, text);
        self.update_info(cx);
    }
}

impl Plugin for Lyrics {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Lyrics",
            "Synced lyrics as a karaoke overlay or a full-screen karaoke view with word highlight, from a .lrc file, the file's tags or LRCLIB; manual search, per-track timing offset, save as .lrc or embed.",
            "lyrics",
        )
        .contribute(contrib::PALETTE_COMMAND, "Find lyrics", "search-open")
        .contribute(contrib::PALETTE_COMMAND, "Karaoke lyrics view", "karaoke")
        .contribute(contrib::CONTEXT_PLAYER, "Find lyrics…", "search-open")
        .setting(SettingSpec::toggle(
            "online",
            "Search lyrics online (LRCLIB)",
            "Sends artist, title, album and duration to lrclib.net. Results are cached.",
            true,
        ))
        .setting(SettingSpec::choice(
            "save-mode",
            "Keep downloaded lyrics",
            "Also store lyrics found on LRCLIB with the song: a .lrc file next to it (never replaces one) or embedded in its tags (a tag backup is kept, Auto-tagger → Restore).",
            &["Don't save", "Save .lrc next to the song", "Embed in the file's tags"],
        ))
        .setting(SettingSpec::choice(
            "radio-mode",
            "Radio lyrics",
            "Network streams: lyrics are looked up when the stream title changes. The song's start is unknown, so synced lyrics can only be timed approximately from the title change.",
            &["Plain lyrics", "Approximate sync", "Off"],
        ))
        .setting(SettingSpec::text(
            "radio-offset",
            "Radio sync offset (seconds)",
            "Approximate sync only: positive shows lines later, negative earlier.",
            "0",
            "0",
        ))
        .setting(SettingSpec::choice(
            "size",
            "Text size",
            "",
            &["Medium", "Small", "Large"],
        ))
        .setting(SettingSpec::action(
            "clear-cache",
            "Downloaded lyrics",
            "Delete cached lyrics, including songs remembered as not found and your manual picks.",
            "Clear cache",
        ))
    }
    fn enable(&mut self, cx: &Ctx) {
        cx.app.global::<crate::Lyrics>().set_enabled(true);
        self.apply_size(cx);
        if cx.app.global::<crate::Playback>().get_has_media() {
            self.load(cx);
        }
    }
    fn disable(&mut self, cx: &Ctx) {
        cx.app.global::<crate::Lyrics>().set_enabled(false);
        cx.app.global::<crate::Overlays>().set_karaoke_open(false);
        cx.app.global::<crate::Overlays>().set_ls_open(false);
        self.task.cancel();
        self.show(cx, None);
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.load(cx);
    }
    fn setting_changed(&mut self, cx: &Ctx, key: &str) {
        match key {
            "size" => self.apply_size(cx),
            "online" | "radio-mode" => self.load(cx),
            _ => {}
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let arg = args.first().map(String::as_str).unwrap_or("");
        let idx = arg.parse::<usize>().ok();
        match name {
            "clear-cache" => {
                let _ = std::fs::remove_dir_all(lyrics_dir());
                cx.toast("Lyrics cache cleared");
                self.load(cx);
            }
            "karaoke" => {
                let on = match arg {
                    "on" => true,
                    "off" => false,
                    _ => !cx.app.global::<crate::Overlays>().get_karaoke_open(),
                };
                cx.app
                    .global::<crate::Overlays>()
                    .set_karaoke_open(on && cx.app.global::<crate::Playback>().get_has_media());
                self.word_line = None;
            }
            "offset" => match arg {
                "reset" => self.set_offset(cx, 0.0),
                d => {
                    if let Ok(d) = d.parse::<f64>() {
                        self.set_offset(cx, self.offset + d);
                    }
                }
            },
            "search-open" => self.search_open(cx),
            "search" => self.search_run(cx),
            "search-select" => {
                if let Some(i) = idx {
                    self.search_select(cx, i);
                }
            }
            "search-use" | "search-plain" => {
                if let Some(i) = idx {
                    self.search_use(cx, i, name == "search-plain");
                }
            }
            "search-close" => {
                self.search.task.cancel();
                cx.app.global::<crate::Lyrics>().set_search_busy(false);
                cx.app.global::<crate::Overlays>().set_ls_open(false);
            }
            "forget" => {
                let _ = std::fs::remove_file(manual_file(&self.key));
                if let Some(r) = self.radio.as_mut() {
                    r.title.clear();
                    r.mem.remove(&r.key);
                } else {
                    self.load(cx);
                }
                cx.toast("Manual lyrics removed");
            }
            "save-lrc" => self.save_lrc(cx, false),
            "embed" => self.embed(cx, false),
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        self.search_tick(cx);
        if self.radio.is_some() && cx.get("radio-mode") != "Off" {
            self.radio_result(cx);
            self.radio_tick(cx);
        }
        if let Some((key, res)) = self.task.poll() {
            if Some(&key) == self.path.as_ref() {
                self.online_result(cx, res);
            }
        }
        if self.lrc.is_none() || self.plain() {
            return; // plain lyrics: nothing to highlight
        }
        let mut t: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        if let (Some(r), true) = (&self.radio, self.radio_synced) {
            t -= r.since + parse_offset(&cx.get("radio-offset"));
        }
        t -= self.offset;
        let Some(l) = &self.lrc else { return };
        let i = l.index_at(t + 0.15);
        if i != self.index {
            self.index = i;
            cx.app
                .global::<crate::Lyrics>()
                .set_index(i.map(|i| i as i32).unwrap_or(-1));
        }
        self.update_words(cx, t + 0.05);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enhanced_lrc_words() {
        let l = Lrc::parse(
            "[00:10.00]<00:10.00>Never <00:10.50>gonna <00:11.00>give<00:11.40>\n[00:12.00]Plain line\n[00:20.00][00:30.00]<00:20.00>Hel<00:20.30>lo",
        );
        assert_eq!(l.lines.len(), 4);
        assert_eq!(l.lines[0].1, "Never gonna give");
        assert_eq!(l.lines[2].1, "Hello");
        assert!(l.has_words());
        assert_eq!(l.words.len(), 4);
        let w: Vec<&str> = l.words[0].iter().map(|w| w.1.as_str()).collect();
        assert_eq!(w, ["Never ", "gonna ", "give", ""], "end marker kept");
        assert!(l.words[1].is_empty(), "untagged line has no words");
        // A repeated line shifts its word times.
        assert_eq!(l.words[3][1].0, 30.3);
        // Word progress.
        assert_eq!(l.word_at(0, 10.25), Some((0, 0.5)));
        assert_eq!(l.word_at(0, 11.2).map(|x| x.0), Some(2));
        assert_eq!(l.word_at(0, 11.2).map(|x| (x.1 * 10.0).round()), Some(5.0));
        assert_eq!(l.word_at(1, 12.5), None);
        // Before the first word: first word at 0 %.
        assert_eq!(l.word_at(2, 19.0), Some((0, 0.0)));
    }

    #[test]
    fn enhanced_offset_and_plain() {
        let l = Lrc::parse("[offset:+1000]\n[00:05.00]<00:05.00>a <00:06.00>b");
        assert_eq!(l.lines[0].0, 4.0);
        assert_eq!(l.words[0][1].0, 5.0);
        let p = Lrc::parse("[00:01.00]x\n[00:02.00]y");
        assert!(!p.has_words() && p.words.is_empty());
        // "<" that is not a time tag stays text.
        assert_eq!(Lrc::parse("[00:01.00]a <3 b").lines[0].1, "a <3 b");
        assert!(!is_synced("just text\nmore"));
        assert!(is_synced("[01:02.03]x"));
    }

    #[test]
    fn offsets_per_track() {
        assert_eq!(clamp_offset(0.1 + 0.2), 0.3);
        assert_eq!(clamp_offset(99.0), 30.0);
        assert_eq!(clamp_offset(-0.04), 0.0);
        assert_eq!(offset_label(0.3), "+0.3 s");
        assert_eq!(offset_label(-1.5), "−1.5 s");
        assert_eq!(offset_label(0.0), "0.0 s");
        let f = std::env::temp_dir().join(format!("vivi-lyr-off-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&f);
        assert!(load_offsets(&f).is_empty());
        save_offset(&f, "/m/a.flac", 0.5);
        save_offset(&f, "/m/b.flac", -1.0);
        let m = load_offsets(&f);
        assert_eq!(m.get("/m/a.flac"), Some(&0.5));
        assert_eq!(m.get("/m/b.flac"), Some(&-1.0));
        save_offset(&f, "/m/a.flac", 0.0);
        assert_eq!(load_offsets(&f).get("/m/a.flac"), None, "0 is forgotten");
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn bake_offset() {
        let src = "[ti:x]\n[00:10.00]A\n[00:20.00]B";
        // +0.5 s user offset (later) = LRC offset −500 ms.
        let out = with_offset(src, 0.5);
        assert!(out.starts_with("[offset:-500]\n"));
        assert_eq!(Lrc::parse(&out).lines[0].0, 10.5);
        // Existing tag is replaced and combined.
        let again = with_offset(&out, -0.2);
        assert_eq!(Lrc::parse(&again).lines[0].0, 10.3);
        assert_eq!(again.matches("[offset:").count(), 1);
        assert_eq!(with_offset(src, 0.0), src);
        assert_eq!(with_offset("plain", 1.0), "plain");
        assert_eq!(lrc_time(83.456), "01:23.46");
    }

    #[test]
    fn source_priority() {
        let f = |s, t: &str| Found::new(s, t).unwrap();
        let synced = "[00:01.00]x";
        let plain = "x";
        // Synced beats plain regardless of source.
        let v = [f(Source::LocalFile, plain), f(Source::Online, synced)];
        assert_eq!(choose(&v).unwrap().source, Source::Online);
        // Same kind: source order.
        let v = [
            f(Source::Online, synced),
            f(Source::Cache, synced),
            f(Source::Embedded, synced),
            f(Source::LocalFile, synced),
        ];
        assert_eq!(choose(&v).unwrap().source, Source::LocalFile);
        let v = [f(Source::Cache, synced), f(Source::Embedded, synced)];
        assert_eq!(choose(&v).unwrap().source, Source::Embedded);
        let v = [f(Source::Online, plain), f(Source::Embedded, plain)];
        assert_eq!(choose(&v).unwrap().source, Source::Embedded);
        // A manual pick wins even when plain.
        let v = [f(Source::LocalFile, synced), f(Source::Manual, plain)];
        assert_eq!(choose(&v).unwrap().source, Source::Manual);
        assert!(choose(&[]).is_none());
        assert!(Found::new(Source::Cache, "  \n").is_none());
    }

    #[test]
    fn rank_results() {
        let it = |d: f64, s: bool| LrclibItem {
            duration: Some(d),
            synced: s.then(|| "[00:01.00]x".to_string()),
            plain: Some("x".into()),
            ..Default::default()
        };
        let mut v = vec![it(200.0, false), it(300.0, true), it(181.0, true)];
        rank(&mut v, 180.0);
        let d: Vec<f64> = v.iter().map(|i| i.duration.unwrap()).collect();
        assert_eq!(d, [181.0, 300.0, 200.0]);
        assert_eq!(v[2].best_text().unwrap(), "x");
    }

    #[test]
    fn karaoke_rows() {
        assert_eq!(wrap_rows(&["aaaa ", "bbbb ", "cc"], 10), [0, 0, 1]);
        assert_eq!(wrap_rows(&["a"], 10), [0]);
        assert!(wrap_rows(&[], 10).is_empty());
        // A word longer than a row gets its own row.
        assert_eq!(wrap_rows(&["x", "verylongwordhere", "y"], 8), [0, 1, 2]);
    }

    #[test]
    fn embedded_lyrics_roundtrip() {
        // Plain and synced lyrics written by the tagger are read back as a source.
        let mp3 = std::path::Path::new("/workspace/vivi-media/forecast.mp3");
        if !mp3.exists() {
            return;
        }
        let d = std::env::temp_dir().join(format!("vivi-lyr-emb-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("song.mp3");
        std::fs::copy(mp3, &p).unwrap();
        let lrc = "[00:01.00]First\n[00:02.50]Second";
        let tags = crate::plugins::auto_tagger::Tags {
            lyrics: lrc.into(),
            ..Default::default()
        };
        crate::plugins::auto_tagger::write_tags(&p, &tags, None).unwrap();
        let e = embedded(&p).unwrap();
        assert_eq!(Lrc::parse(&e).texts(), ["First", "Second"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parse_lrc() {
        let l = Lrc::parse(
            "[ar:Someone]\n[ti:Song]\n[00:12.00]First\n[00:05.50][01:00.00]Chorus\nno tag\n[00:20.5]  Third  ",
        );
        let t: Vec<f64> = l.lines.iter().map(|x| x.0).collect();
        assert_eq!(t, [5.5, 12.0, 20.5, 60.0]);
        assert_eq!(l.lines[2].1, "Third");
        assert_eq!(l.index_at(0.0), None);
        assert_eq!(l.index_at(12.0), Some(1));
        assert_eq!(l.index_at(30.0), Some(2));
        assert_eq!(l.index_at(1e9), Some(3));
    }

    #[test]
    fn offset_tag() {
        let l = Lrc::parse("[offset:+500]\n[00:10.00]A");
        assert_eq!(l.lines[0].0, 9.5);
    }

    #[test]
    fn local_file() {
        let d = std::env::temp_dir().join(format!("vivi-lrc-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let song = d.join("song.flac");
        assert_eq!(local_lrc(&song), None);
        std::fs::write(d.join("song.lrc"), "[00:01.00]x").unwrap();
        assert_eq!(local_lrc(&song), Some(d.join("song.lrc")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn stream_titles() {
        let p = |s: &str| parse_stream_title(s, "Groove Salad");
        assert_eq!(
            p("Daft Punk - Get Lucky (Radio Edit) [feat. Pharrell]"),
            Some(("Daft Punk".into(), "Get Lucky".into()))
        );
        assert_eq!(
            p("Massive Attack – Teardrop ft. Liz Fraser"),
            Some(("Massive Attack".into(), "Teardrop".into()))
        );
        assert_eq!(p("Groove Salad"), None);
        assert_eq!(p("Groove Salad - Commercial Free"), None);
        assert_eq!(p("Advertisement - Buy now"), None);
        assert_eq!(p("You're listening to Radio X"), None);
        assert_eq!(p("www.radio.fm - best hits"), None);
        assert_eq!(p("Just a title"), None);
        assert_eq!(p(" - "), None);
        assert_eq!(parse_offset("-2.5"), -2.5);
        assert_eq!(parse_offset("3s"), 3.0);
        assert_eq!(parse_offset("x"), 0.0);
    }

    #[test]
    fn plain_from_synced() {
        let it = LrclibItem {
            synced: Some("[00:01.00]A\n[00:02.00]B".into()),
            ..Default::default()
        };
        assert_eq!(plain_lines(&it), ["A", "B"]);
        assert!(it.has_lyrics());
        assert!(!LrclibItem::default().has_lyrics());
    }

    /// Network test; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn lrclib_radio_live() {
        let it = fetch_radio("Daft Punk", "Get Lucky").unwrap().unwrap();
        assert!(!plain_lines(&it).is_empty());
        // Swapped "Title - Artist" still found through search.
        assert!(fetch_radio("Get Lucky", "Daft Punk").unwrap().is_some());
    }

    /// Network test; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn lrclib_live() {
        let t = Track {
            artist: "Borislav Slavov".into(),
            title: "I Want to Live".into(),
            album: "Baldur's Gate 3 (Original Game Soundtrack)".into(),
            duration: 233.0,
        };
        assert!(fetch_lrclib(&t).unwrap().is_some());
    }
}
