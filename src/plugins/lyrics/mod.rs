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
//! Clicking a line of synced lyrics (karaoke view, overlay, side drawer) seeks there.
//! Fetched lyrics can be saved as `song.lrc` or embedded in the tags (Auto-tagger code,
//! with a tag backup). Tracks LRCLIB marks as instrumental say so instead of "no lyrics";
//! "not found" is remembered for a week, then asked again.
//!
//! Network streams (radio): whenever the stream title (`icy-title`, else `media-title`,
//! usually "Artist - Title") changes, it is parsed and looked up on LRCLIB; ads, jingles
//! and station IDs are skipped and results are cached (memory + disk). The song's start
//! time is unknown, so lyrics are shown as a scrollable list. Synced lyrics can be synced
//! by hand: clicking (or Enter on) the line being sung anchors that line's timestamp to
//! the current stream time ([`Anchor`]); from then on they highlight and scroll like any
//! karaoke, fine-tuned with the same offset control (not saved). A new stream title
//! drops the anchor.

use super::lrclib::{self, cached_miss, non_empty, rank, read_cache, write_cache};
pub use super::lrclib::{Fetched, LrclibItem, Track};
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
mod plugin;
mod radio_view;

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

/// Player time at which lyrics line time `line_t` is shown, with the track's `offset`
/// (positive = lyrics later).
pub fn line_seek(line_t: f64, offset: f64) -> f64 {
    (line_t + offset).max(0.0)
}

/// The song to look up from the shared now-playing track `np` of `path`: only metadata a
/// plugin sent (music services, the Auto-tagger's identification) for a song with a
/// known length; live radio and plain tags / file names are handled elsewhere.
pub fn np_track(np: &crate::nowplaying::Track, path: Option<&str>) -> Option<Track> {
    let ok = path.is_some_and(|p| p == np.path)
        && np.source.starts_with("plugin:")
        && np.kind == "music"
        && np.duration > 0.0
        && !np.artist.trim().is_empty()
        && !np.title.trim().is_empty();
    ok.then(|| Track {
        artist: np.artist.trim().to_string(),
        title: np.title.trim().to_string(),
        album: np.album.trim().to_string(),
        duration: np.duration,
    })
}

fn mmss(s: f64) -> String {
    let s = s.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Default)]
pub struct Lyrics {
    lrc: Option<Lrc>,
    index: Option<usize>,
    task: crate::util::Task<(String, Option<Fetched>)>,
    /// LRCLIB lists the track as instrumental (nothing shown, said so in the view).
    instrumental: bool,
    /// Change counter of the shared now-playing track last looked at.
    np_id: i64,
    path: Option<String>,
    /// Some while a network stream plays.
    radio: Option<Radio>,
    /// Radio with synced lyrics: lines can be clicked to sync them (see [`Anchor`]).
    live: bool,
    anchor: Anchor,
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
        self.instrumental = false;
        self.live = false;
        self.anchor.clear();
        self.word_line = None;
        self.words = None;
        cx.app.global::<crate::Lyrics>().set_plain(false);
        cx.app.global::<crate::Lyrics>().set_live(false);
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
            || self.radio.is_some() && !self.anchor.synced()
    }

    /// Radio, synced lyrics just shown: start as the unsynced, clickable list.
    fn radio_list(&mut self, cx: &Ctx) {
        self.live = true;
        self.anchor.clear();
        self.offset = 0.0;
        cx.app.global::<crate::Lyrics>().set_plain(true);
        cx.app.global::<crate::Lyrics>().set_live(true);
    }

    /// The user marked line `i` as being sung now (radio): anchor and start highlighting.
    fn sync_line(&mut self, cx: &Ctx, i: usize) {
        let Some(line_t) = self
            .lrc
            .as_ref()
            .filter(|_| self.live)
            .and_then(|l| l.lines.get(i))
            .map(|l| l.0)
        else {
            return;
        };
        let now: f64 = cx.player.mpv.get_property("time-pos").unwrap_or(0.0);
        self.anchor.set(now, line_t);
        self.offset = 0.0;
        self.index = None;
        self.word_line = None;
        cx.app.global::<crate::Lyrics>().set_plain(false);
        self.update_info(cx);
    }

    /// Source, offset and track labels for the karaoke view and the side panel.
    fn update_info(&self, cx: &Ctx) {
        let state = match &self.current {
            Some(_) => crate::LyricsState::Found,
            None if self.instrumental => crate::LyricsState::Instrumental,
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
            None if self.instrumental => "Instrumental · LRCLIB".into(),
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
        // Save / embed: a local file's lyrics, unless that is where they came from (and
        // no offset is waiting to be baked in).
        let save = self.radio.is_none() && cx.local_path().is_some();
        let from = |s: Source| {
            self.current
                .as_ref()
                .is_some_and(|c| c.source == s && self.offset == 0.0)
        };
        let l = cx.app.global::<crate::Lyrics>();
        l.set_can_save_lrc(save && self.current.is_some() && !from(Source::LocalFile));
        l.set_can_embed(save && self.current.is_some() && !from(Source::Embedded));
        l.set_seekable(self.seekable());
    }

    /// Synced lyrics of a file: a click on a line seeks there.
    fn seekable(&self) -> bool {
        self.radio.is_none() && self.lrc.is_some() && !self.plain()
    }

    /// Seek to the start of line `i` (with the track's offset).
    fn seek_line(&mut self, cx: &Ctx, i: usize) {
        if !self.seekable() {
            return;
        }
        let Some(t) = self.lrc.as_ref().and_then(|l| l.lines.get(i)).map(|l| l.0) else {
            return;
        };
        cx.player.seek_abs(line_seek(t, self.offset));
        self.index = None;
        self.word_line = None;
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
            // Stream titles are polled in `tick`; a music service's song info (shared
            // now-playing track) turns it into a normal song.
            self.radio = Some(Radio::default());
            self.np_id = crate::nowplaying::current_id();
            self.now_playing(cx);
            return;
        }
        let mut found = Vec::new();
        if let Some(s) = read_lyrics_file(&manual_file(&self.key)) {
            found.extend(Found::new(Source::Manual, s));
        }
        let local = cx.local_path();
        if let Some(p) = &local {
            if let Some(l) = local_lrc(p).and_then(|f| read_lyrics_file(&f)) {
                found.extend(Found::new(Source::LocalFile, l));
            }
        }
        // Tags and online lookup only for music (no real video track).
        let music = crate::viz::audio_only(cx.player);
        if let (Some(p), true) = (&local, music) {
            found.extend(embedded(p).and_then(|s| Found::new(Source::Embedded, s)));
        }
        let track = if music { track_info(cx) } else { None };
        self.lookup(cx, track, found);
        // A music service may already have described the stream (see `now_playing`).
        self.np_id = crate::nowplaying::current_id();
        self.now_playing(cx);
    }

    /// Show the best of `found` and the cache of `track`, then ask LRCLIB when nothing
    /// synced is known yet.
    fn lookup(&mut self, cx: &Ctx, track: Option<Track>, mut found: Vec<Found>) {
        self.track = track;
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
        if done || !cx.flag("online") {
            return;
        }
        // Cached plain lyrics: no need to ask again.
        if found.iter().any(|f| f.source == Source::Cache) {
            return;
        }
        // Remembered as instrumental / not found (until it is stale).
        if let Some(miss) = cached_miss(&cache) {
            if miss == Fetched::Instrumental && self.current.is_none() {
                self.instrumental = true;
                self.update_info(cx);
            }
            return;
        }
        let key = self.path.clone().unwrap_or_default();
        self.task = crate::util::Task::spawn(move || {
            let res = lrclib::fetch(&t);
            match &res {
                Ok(f) => write_cache(&cache, f),
                Err(e) => eprintln!("vivi: lyrics: {e}"),
            }
            (key, res.ok())
        });
        self.update_info(cx);
    }

    /// The shared now-playing track changed. A song a music service (`vivi-media-meta`) or
    /// the Auto-tagger described, with a known length, is looked up like a tagged file:
    /// a stream then plays as a song (synced from its start, not the radio list) and an
    /// untagged file gets lyrics once it is identified. Lyrics from your pick, a .lrc file
    /// or the tags are kept.
    fn now_playing(&mut self, cx: &Ctx) {
        let np = crate::nowplaying::current();
        let Some(track) = np_track(&np, self.path.as_deref()) else {
            return;
        };
        if self.track.as_ref() == Some(&track) {
            return;
        }
        let local_lyrics = self
            .current
            .as_ref()
            .is_some_and(|c| c.source <= Source::Embedded);
        if self.radio.is_none() && local_lyrics {
            return;
        }
        self.task.cancel();
        if let Some(r) = self.radio.as_mut() {
            r.task.cancel();
        }
        self.radio = None;
        self.show(cx, None);
        // Streams: offsets and picks follow the song (the URL may change).
        if cx.local_path().is_none() {
            self.set_key(
                cx,
                crate::plugins::protocol::join(&["song", track.artist.as_str(), &track.title]),
            );
        }
        let mut found = Vec::new();
        if let Some(s) = read_lyrics_file(&manual_file(&self.key)) {
            found.extend(Found::new(Source::Manual, s));
        }
        self.lookup(cx, Some(track), found);
    }

    /// Online result: shown unless a better local source is already displayed.
    fn online_result(&mut self, cx: &Ctx, res: Option<Fetched>) {
        let text = match res {
            Some(Fetched::Lyrics(t)) => Some(t),
            Some(Fetched::Instrumental) => {
                self.instrumental = self.current.is_none();
                None
            }
            _ => None,
        };
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
        if self.lrc.is_none() {
            cx.toast("No lyrics to shift for this track");
            return;
        }
        if self.radio.is_none() && self.plain() {
            cx.toast("Plain lyrics have no timing to shift");
            return;
        }
        if self.radio.is_some() && !self.anchor.synced() {
            if self.live {
                cx.toast("Click the line being sung to sync the lyrics first");
            }
            return;
        }
        self.offset = clamp_offset(o);
        // Radio: the offset fine-tunes this listen's sync only.
        if !self.key.is_empty() && self.radio.is_none() {
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
        self.search.task = crate::util::Task::spawn(move || lrclib::search(&artist, &title));
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
        // Remembered for the track (nothing to remember it by without one).
        if !self.key.is_empty() {
            let file = manual_file(&self.key);
            if let Some(d) = file.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&file, &f.text);
        }
        cx.app.global::<crate::Overlays>().set_ls_open(false);
        if let Some(r) = self.radio.as_mut() {
            r.mem.remove(&r.key);
        }
        let synced = f.synced();
        self.show_found(cx, Some(f));
        if synced && self.radio.is_some() {
            if click_sync(&cx.get("radio-mode")) {
                self.radio_list(cx);
                self.update_info(cx);
            } else {
                // Radio, "Plain lyrics": plain block.
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

#[cfg(test)]
mod tests;
