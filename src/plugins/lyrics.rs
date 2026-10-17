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

use super::{Ctx, Manifest, Plugin, SettingSpec};
use slint::ComponentHandle;
use slint::{ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

pub const ID: &str = "lyrics";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: f64,
}

/// Timed words of a line: (start seconds, text).
pub type Words = Vec<(f64, String)>;

/// Parsed LRC: lines sorted by time (seconds) and, for enhanced LRC
/// (`[00:12.00]<00:12.00>Never <00:12.40>gonna`), the timed words of each line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lrc {
    pub lines: Vec<(f64, String)>,
    /// Same length as `lines` (or empty); a word's text keeps its trailing space. An
    /// entry with empty text marks the end of the last word.
    pub words: Vec<Vec<(f64, String)>>,
}

impl Lrc {
    /// Parse LRC text. Supports several time tags per line, enhanced `<mm:ss.xx>` word
    /// tags and the `[offset:±ms]` tag.
    pub fn parse(src: &str) -> Self {
        let mut offset = 0.0;
        let mut rows: Vec<(f64, String, Words)> = Vec::new();
        for raw in src.lines() {
            let mut rest = raw.trim();
            let mut times = Vec::new();
            while let Some(r) = rest.strip_prefix('[') {
                let Some(end) = r.find(']') else { break };
                let tag = &r[..end];
                rest = &r[end + 1..];
                if let Some(o) = tag.strip_prefix("offset:") {
                    offset = o.trim().parse::<f64>().unwrap_or(0.0) / 1000.0;
                } else if let Some(t) = tag_time(tag) {
                    times.push(t);
                }
            }
            let Some(&t0) = times.first() else { continue };
            let (text, words) = parse_words(rest, t0);
            for t in times {
                let w = words
                    .iter()
                    .map(|(wt, s)| (wt - t0 + t, s.clone()))
                    .collect();
                rows.push((t, text.clone(), w));
            }
        }
        // A positive offset shows lyrics earlier.
        let sh = |t: f64| (t - offset).max(0.0);
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));
        let enhanced = rows.iter().any(|r| !r.2.is_empty());
        let mut l = Lrc::default();
        for (t, s, w) in rows {
            l.lines.push((sh(t), s));
            if enhanced {
                l.words
                    .push(w.into_iter().map(|(wt, s)| (sh(wt), s)).collect());
            }
        }
        l
    }
    /// Index of the line being sung at `t`.
    pub fn index_at(&self, t: f64) -> Option<usize> {
        let n = self.lines.partition_point(|l| l.0 <= t);
        n.checked_sub(1)
    }
    pub fn has_words(&self) -> bool {
        self.words.iter().any(|w| !w.is_empty())
    }
    /// Word being sung in line `i` at `t` and how far it is (0..1). Words before it are
    /// done, words after it are still to come. None when the line has no word timing.
    pub fn word_at(&self, i: usize, t: f64) -> Option<(usize, f64)> {
        let w = self.words.get(i).filter(|w| !w.is_empty())?;
        let next_line = self.lines.get(i + 1).map(|l| l.0);
        let k = w.partition_point(|x| x.0 <= t).saturating_sub(1);
        let start = w[k].0;
        let end = w
            .get(k + 1)
            .map(|x| x.0)
            .or(next_line)
            .unwrap_or(start + 1.0)
            .max(start + 0.01);
        let p = ((t - start) / (end - start)).clamp(0.0, 1.0);
        Some((k, p))
    }
    /// The line texts (no timing).
    pub fn texts(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.1.clone()).collect()
    }
}

/// Text of a line body and its timed words ("<00:01.00>Hel<00:01.20>lo <00:01.50>").
fn parse_words(rest: &str, line_t: f64) -> (String, Vec<(f64, String)>) {
    let mut words: Vec<(f64, String)> = Vec::new();
    let mut text = String::new();
    let mut cur: Option<f64> = None;
    let mut buf = String::new();
    let mut s = rest;
    let mut tagged = false;
    let flush = |cur: Option<f64>, buf: &mut String, words: &mut Vec<(f64, String)>| {
        if !buf.trim().is_empty() {
            words.push((cur.unwrap_or(line_t), std::mem::take(buf)));
        } else {
            buf.clear();
        }
    };
    loop {
        let tag = s.find('<').and_then(|i| {
            let j = s[i..].find('>')? + i;
            tag_time(&s[i + 1..j]).map(|t| (i, j, t))
        });
        match tag {
            Some((i, j, t)) => {
                buf.push_str(&s[..i]);
                text.push_str(&s[..i]);
                flush(cur, &mut buf, &mut words);
                cur = Some(t);
                tagged = true;
                s = &s[j + 1..];
            }
            None => {
                buf.push_str(s);
                text.push_str(s);
                flush(cur, &mut buf, &mut words);
                break;
            }
        }
    }
    if !tagged {
        return (rest.trim().to_string(), Vec::new());
    }
    // A trailing time tag without text marks where the last word ends.
    if let Some(t) = cur.filter(|t| words.last().is_some_and(|w| w.0 < *t)) {
        words.push((t, String::new()));
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (text, words)
}

/// "mm:ss.xx" → seconds.
fn tag_time(tag: &str) -> Option<f64> {
    let (m, s) = tag.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let s: f64 = s.trim().replace(':', ".").parse().ok()?;
    (m >= 0.0 && s >= 0.0).then_some(m * 60.0 + s)
}

/// Whether text has LRC line time tags.
pub fn is_synced(text: &str) -> bool {
    !Lrc::parse(text).lines.is_empty()
}

/// `song.lrc` (or `song.LRC`) next to `song.flac`.
pub fn local_lrc(media: &Path) -> Option<PathBuf> {
    ["lrc", "LRC"]
        .iter()
        .map(|e| media.with_extension(e))
        .find(|p| p.is_file())
}

// ---------- sources ----------

/// Where lyrics come from, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Manual,
    LocalFile,
    Embedded,
    Cache,
    Online,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Manual => "Your pick",
            Source::LocalFile => ".lrc file",
            Source::Embedded => "Embedded tags",
            Source::Cache => "LRCLIB (cached)",
            Source::Online => "LRCLIB",
        }
    }
}

/// Lyrics text from one source (LRC when synced, plain text otherwise).
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub source: Source,
    pub text: String,
}

impl Found {
    pub fn new(source: Source, text: impl Into<String>) -> Option<Self> {
        let text = text.into();
        (!text.trim().is_empty()).then_some(Found { source, text })
    }
    pub fn synced(&self) -> bool {
        is_synced(&self.text)
    }
}

/// The lyrics to show: a manual pick wins; otherwise synced beats plain, then the
/// source order (.lrc, embedded, cache, online).
pub fn choose(found: &[Found]) -> Option<&Found> {
    if let Some(m) = found.iter().find(|f| f.source == Source::Manual) {
        return Some(m);
    }
    found.iter().min_by_key(|f| (!f.synced(), f.source))
}

/// Lyrics embedded in the file's tags: LRC (or plain) text from USLT / LYRICS / ©lyr,
/// else an MP3's SYLT frame converted to LRC.
pub fn embedded(path: &Path) -> Option<String> {
    let text = super::tagger::read_tags(path)
        .map(|(t, _)| t.lyrics)
        .unwrap_or_default();
    if is_synced(&text) {
        return Some(text);
    }
    let is_mp3 = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"));
    if is_mp3 {
        let sylt = super::tagger::read_sylt(path);
        if !sylt.is_empty() {
            return Some(
                sylt.iter()
                    .map(|(ms, l)| format!("[{}]{l}", lrc_time(*ms as f64 / 1000.0)))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    (!text.trim().is_empty()).then_some(text)
}

/// Seconds → "mm:ss.xx".
pub fn lrc_time(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{:02}:{:02}.{:02}", cs / 6000, cs / 100 % 60, cs % 100)
}

/// LRC text with the user offset baked in as an `[offset:]` tag (positive user offset =
/// lyrics later = negative LRC offset). Replaces an existing offset tag.
pub fn with_offset(text: &str, offset: f64) -> String {
    let body: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("[offset:"))
        .collect();
    let old = Lrc::parse(text);
    let base = Lrc::parse(&body.join("\n"));
    // Offset already applied by an existing tag (seconds, positive = earlier).
    let prev = match (base.lines.first(), old.lines.first()) {
        (Some(a), Some(b)) => a.0 - b.0,
        _ => 0.0,
    };
    let total_ms = ((prev - offset) * 1000.0).round() as i64;
    let mut out = String::new();
    if total_ms != 0 && is_synced(text) {
        out.push_str(&format!("[offset:{total_ms:+}]\n"));
    }
    out.push_str(&body.join("\n"));
    out
}

fn cache_key(t: &Track) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        t.artist.to_lowercase(),
        t.title.to_lowercase(),
        t.album.to_lowercase(),
        t.duration.round()
    )
}

fn lyrics_dir() -> PathBuf {
    crate::store::cache_dir().join("lyrics")
}

/// Cache file stem for a track: `<md5>.lrc` (synced), `.txt` (plain), `.missing`.
fn cache_file(t: &Track) -> PathBuf {
    lyrics_dir().join(format!("{:x}.lrc", md5::compute(cache_key(t))))
}

/// Cached lyrics of a track (synced `.lrc` first, then plain `.txt`).
fn read_cache(stem: &Path) -> Option<String> {
    ["lrc", "txt"]
        .iter()
        .find_map(|e| std::fs::read_to_string(stem.with_extension(e)).ok())
        .filter(|s| !s.trim().is_empty())
}

fn write_cache(stem: &Path, text: Option<&str>) {
    if let Some(d) = stem.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    match text {
        Some(s) => {
            let ext = if is_synced(s) { "lrc" } else { "txt" };
            let _ = std::fs::write(stem.with_extension(ext), s);
        }
        None => {
            let _ = std::fs::write(stem.with_extension("missing"), b"");
        }
    }
}

/// Manual pick for a track key (file path or radio "artist\u{1f}title").
fn manual_file(key: &str) -> PathBuf {
    lyrics_dir()
        .join("manual")
        .join(format!("{:x}.lrc", md5::compute(key)))
}

// ---------- per-track offsets ----------

fn offsets_file() -> PathBuf {
    crate::store::data_dir().join("lyrics-offsets.json")
}

/// Saved timing offsets (seconds, positive = lyrics later) by track key.
pub fn load_offsets(file: &Path) -> HashMap<String, f64> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Round to 0.1 s and clamp to ±30 s.
pub fn clamp_offset(o: f64) -> f64 {
    ((o * 10.0).round() / 10.0).clamp(-30.0, 30.0)
}

/// Store (or, at 0, forget) the offset of `key`.
pub fn save_offset(file: &Path, key: &str, offset: f64) {
    let mut m = load_offsets(file);
    if offset == 0.0 {
        m.remove(key);
    } else {
        m.insert(key.to_string(), offset);
    }
    if let Ok(s) = serde_json::to_string_pretty(&m) {
        let _ = crate::store::write_atomic(file, &s);
    }
}

/// "+0.3 s", "−1.5 s", "0.0 s".
pub fn offset_label(o: f64) -> String {
    if o.abs() < 0.05 {
        "0.0 s".into()
    } else if o > 0.0 {
        format!("+{o:.1} s")
    } else {
        format!("−{:.1} s", -o)
    }
}

// ---------- LRCLIB ----------

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default, PartialEq)]
pub struct LrclibItem {
    #[serde(rename = "artistName", default)]
    pub artist: Option<String>,
    #[serde(rename = "trackName", default)]
    pub title: Option<String>,
    #[serde(rename = "albumName", default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(rename = "syncedLyrics", default)]
    pub synced: Option<String>,
    #[serde(rename = "plainLyrics", default)]
    pub plain: Option<String>,
    #[serde(default)]
    pub instrumental: Option<bool>,
}

fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.trim().is_empty())
}

impl LrclibItem {
    fn has_lyrics(&self) -> bool {
        non_empty(&self.synced).is_some() || non_empty(&self.plain).is_some()
    }
    /// Synced LRC if present, else plain text.
    pub fn best_text(&self) -> Option<String> {
        non_empty(&self.synced)
            .or(non_empty(&self.plain))
            .map(String::from)
    }
}

/// Order search results: synced first, then by distance to the track's duration.
pub fn rank(items: &mut [LrclibItem], duration: f64) {
    let diff = |i: &LrclibItem| match (i.duration, duration > 0.0) {
        (Some(d), true) => ((d - duration).abs() * 10.0) as i64,
        _ => i64::MAX / 2,
    };
    items.sort_by_key(|i| (non_empty(&i.synced).is_none(), diff(i)));
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .user_agent(format!(
            "{} (https://github.com/Loafer19/vivi)",
            super::USER_AGENT
        ))
        .build()
        .into()
}

/// Runs an LRCLIB request, retrying when the server is busy. LRCLIB answers uncached
/// lookups with `503 ServerOverloaded` (Retry-After: 1) under load and `429` when rate
/// limited; both are transient, so wait 1 s, then 2 s, before giving up.
fn call(
    mut send: impl FnMut() -> Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let mut wait = 1;
    loop {
        match send() {
            Err(ureq::Error::StatusCode(503 | 429)) if wait <= 2 => {
                std::thread::sleep(Duration::from_secs(wait));
                wait *= 2;
            }
            r => return r,
        }
    }
}

/// Human-readable error, so a busy LRCLIB is not reported as a bare "http status: 503".
fn err(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(503) => {
            "LRCLIB is busy right now (HTTP 503), try again in a moment".into()
        }
        ureq::Error::StatusCode(429) => {
            "LRCLIB rate limit reached (HTTP 429), try again later".into()
        }
        e => e.to_string(),
    }
}

/// LRCLIB `/api/search` by artist and title (title only when the artist is empty).
pub fn search_lrclib(artist: &str, title: &str) -> Result<Vec<LrclibItem>, String> {
    let a = agent();
    let mut r = call(|| {
        let req = a.get("https://lrclib.net/api/search");
        if artist.trim().is_empty() {
            req.query("q", title)
        } else {
            req.query("artist_name", artist).query("track_name", title)
        }
        .call()
    })
    .map_err(err)?;
    let items: Vec<LrclibItem> = r.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(items.into_iter().filter(|i| i.has_lyrics()).collect())
}

/// Lyrics from LRCLIB: `/api/get` (exact match), then `/api/search` (synced first,
/// closest duration). Plain lyrics are returned when no synced ones exist.
pub fn fetch_lrclib(t: &Track) -> Result<Option<String>, String> {
    let a = agent();
    let get = || {
        let mut req = a
            .get("https://lrclib.net/api/get")
            .query("artist_name", &t.artist)
            .query("track_name", &t.title);
        if !t.album.is_empty() {
            req = req.query("album_name", &t.album);
        }
        if t.duration > 0.0 {
            req = req.query("duration", format!("{}", t.duration.round()));
        }
        req.call()
    };
    let mut plain = None;
    match call(get) {
        Ok(mut r) => {
            let it: LrclibItem = r.body_mut().read_json().map_err(|e| e.to_string())?;
            if let Some(s) = non_empty(&it.synced) {
                return Ok(Some(s.to_string()));
            }
            plain = non_empty(&it.plain).map(String::from);
        }
        Err(ureq::Error::StatusCode(404)) => {}
        Err(e) => return Err(err(e)),
    }
    let mut items = search_lrclib(&t.artist, &t.title)?;
    rank(&mut items, t.duration);
    Ok(items
        .first()
        .and_then(|i| non_empty(&i.synced).map(String::from))
        .or(plain)
        .or_else(|| items.first().and_then(|i| i.best_text())))
}

/// Artist/title/album/duration from tags (lofty), falling back to mpv metadata.
fn track_info(cx: &Ctx) -> Option<Track> {
    let mut t = Track::default();
    if let Some(p) = cx.local_path() {
        use lofty::prelude::*;
        if let Ok(tf) = lofty::read_from_path(&p) {
            t.duration = tf.properties().duration().as_secs_f64();
            if let Some(tag) = tf.primary_tag().or_else(|| tf.first_tag()) {
                let s = |v: Option<std::borrow::Cow<str>>| {
                    v.map(|x| x.trim().to_string()).unwrap_or_default()
                };
                t.artist = s(tag.artist());
                t.title = s(tag.title());
                t.album = s(tag.album());
            }
        }
    }
    let meta = |k: &str| {
        cx.player
            .get(&format!("metadata/by-key/{k}"))
            .or_else(|| {
                cx.player
                    .get(&format!("metadata/by-key/{}", k.to_uppercase()))
            })
            .unwrap_or_default()
    };
    if t.artist.is_empty() {
        t.artist = meta("artist");
    }
    if t.title.is_empty() {
        t.title = meta("title");
    }
    if t.album.is_empty() {
        t.album = meta("album");
    }
    if t.duration <= 0.0 {
        t.duration = cx.player.mpv.get_property("duration").unwrap_or(0.0);
    }
    (!t.artist.is_empty() && !t.title.is_empty()).then_some(t)
}

// ---------- Radio (network streams) ----------

/// Words that mark a stream title as an ad, jingle, station ID or talk segment.
const NOT_SONG: &[&str] = &[
    "advert",
    "commercial",
    "sponsor",
    "jingle",
    "station id",
    "promo",
    "spot block",
    "you're listening",
    "you are listening",
    "listening to",
    "now playing",
    "news",
    "weather",
    "traffic",
    "live stream",
    "on air",
    "unknown",
    "www.",
    "http",
    ".com",
    ".fm",
    "call us",
    "text us",
    "podcast",
];

/// "Artist - Title" from a stream title, or None for ads, station IDs and the like.
/// `station` (icy-name) is used to reject titles that are just the station's name.
pub fn parse_stream_title(raw: &str, station: &str) -> Option<(String, String)> {
    let s = raw.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    let low = s.to_lowercase();
    if s.len() < 3 || NOT_SONG.iter().any(|w| low.contains(w)) {
        return None;
    }
    let st = station.trim().to_lowercase();
    if !st.is_empty() && (low == st || low.starts_with(&format!("{st} -"))) {
        return None;
    }
    let (a, t) = [" - ", " – ", " — ", " ~ ", " | "]
        .iter()
        .find_map(|sep| s.split_once(sep))?;
    let artist = a.trim().to_string();
    let title = clean_title(t);
    let bad = |x: &str| x.is_empty() || x.chars().all(|c| !c.is_alphanumeric());
    if bad(&artist) || bad(&title) {
        return None;
    }
    Some((artist, title))
}

/// Drop decorations that LRCLIB does not know: "(Radio Edit)", "[Live]", "feat. X".
fn clean_title(t: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in t.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let low = out.to_lowercase();
    let cut = [" feat. ", " feat ", " ft. ", " ft "]
        .iter()
        .filter_map(|w| low.find(w))
        .min()
        .unwrap_or(out.len());
    out[..cut].split_whitespace().collect::<Vec<_>>().join(" ")
}

fn radio_cache_file(artist: &str, title: &str) -> PathBuf {
    let key = format!(
        "radio\u{1f}{}\u{1f}{}",
        artist.to_lowercase(),
        title.to_lowercase()
    );
    crate::store::cache_dir()
        .join("lyrics")
        .join(format!("{:x}.json", md5::compute(key)))
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Best LRCLIB entry for a radio title (no album or duration known): `/api/get`, then
/// `/api/search` keeping only results whose artist and title match (either way round,
/// some stations send "Title - Artist").
pub fn fetch_radio(artist: &str, title: &str) -> Result<Option<LrclibItem>, String> {
    let a = agent();
    match call(|| {
        a.get("https://lrclib.net/api/get")
            .query("artist_name", artist)
            .query("track_name", title)
            .call()
    }) {
        Ok(mut r) => {
            let it: LrclibItem = r.body_mut().read_json().map_err(|e| e.to_string())?;
            if it.has_lyrics() {
                return Ok(Some(it));
            }
        }
        Err(ureq::Error::StatusCode(404)) => {}
        Err(e) => return Err(err(e)),
    }
    let mut r = call(|| {
        a.get("https://lrclib.net/api/search")
            .query("q", format!("{artist} {title}"))
            .call()
    })
    .map_err(err)?;
    let items: Vec<LrclibItem> = r.body_mut().read_json().map_err(|e| e.to_string())?;
    let (na, nt) = (norm(artist), norm(title));
    let close =
        |x: &str, y: &str| !x.is_empty() && !y.is_empty() && (x.contains(y) || y.contains(x));
    let matches = |i: &LrclibItem| {
        let (ia, it) = (
            norm(i.artist.as_deref().unwrap_or("")),
            norm(i.title.as_deref().unwrap_or("")),
        );
        (close(&ia, &na) && close(&it, &nt)) || (close(&ia, &nt) && close(&it, &na))
    };
    let mut good: Vec<LrclibItem> = items
        .into_iter()
        .filter(|i| matches(i) && i.has_lyrics())
        .collect();
    // Prefer entries with synced lyrics.
    good.sort_by_key(|i| i.synced.as_ref().is_none_or(|s| s.trim().is_empty()));
    Ok(good.into_iter().next())
}

/// Plain text lines of an entry (from `plainLyrics`, else the synced text without tags).
fn plain_lines(it: &LrclibItem) -> Vec<String> {
    if let Some(p) = it.plain.as_ref().filter(|p| !p.trim().is_empty()) {
        return p.lines().map(|l| l.trim().to_string()).collect();
    }
    it.synced
        .as_deref()
        .map(|s| Lrc::parse(s).lines.into_iter().map(|(_, l)| l).collect())
        .unwrap_or_default()
}

/// Seconds from the "Radio sync offset" setting (positive = lyrics later).
fn parse_offset(s: &str) -> f64 {
    s.trim()
        .trim_end_matches('s')
        .trim()
        .replace(',', ".")
        .parse()
        .unwrap_or(0.0)
}

#[derive(Default)]
struct Radio {
    /// Raw stream title last seen.
    title: String,
    /// `time-pos` when it changed (approximate song start).
    since: f64,
    /// Lookup key ("artist\u{1f}title") whose result is displayed / awaited.
    key: String,
    /// Parsed (artist, title) of the current song.
    song: Option<(String, String)>,
    mem: HashMap<String, Option<LrclibItem>>,
    rx: Option<Receiver<(String, Option<LrclibItem>)>>,
    last_poll: Option<std::time::Instant>,
}

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
    rx: Option<Receiver<Result<Vec<LrclibItem>, String>>>,
}

fn mmss(s: f64) -> String {
    let s = s.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Default)]
pub struct Lyrics {
    lrc: Option<Lrc>,
    index: Option<usize>,
    rx: Option<Receiver<(String, Option<String>)>>,
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
        cx.app.set_lyrics_plain(false);
        cx.app
            .set_lyrics_words(ModelRc::new(VecModel::<crate::KWord>::default()));
        cx.app.set_lyrics_rows(0);
        let lines: Vec<SharedString> = lrc
            .as_ref()
            .map(|l| l.lines.iter().map(|(_, s)| s.as_str().into()).collect())
            .unwrap_or_default();
        cx.app.set_lyrics_lines(ModelRc::new(VecModel::from(lines)));
        cx.app.set_lyrics_index(-1);
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
            cx.app.set_lyrics_plain(true);
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
            None if self.rx.is_some() || self.radio.as_ref().is_some_and(|r| r.rx.is_some()) => {
                crate::LyricsState::Searching
            }
            None => crate::LyricsState::Missing,
        };
        cx.app.set_lyrics_state(state);
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
            None if self.rx.is_some() || self.radio.as_ref().is_some_and(|r| r.rx.is_some()) => {
                "Searching LRCLIB…".into()
            }
            None => "No lyrics found".into(),
        };
        cx.app.set_lyrics_source(src.into());
        cx.app.set_lyrics_manual(
            self.current
                .as_ref()
                .is_some_and(|c| c.source == Source::Manual),
        );
        cx.app.set_lyrics_offset(self.offset as f32);
        cx.app
            .set_lyrics_offset_label(offset_label(self.offset).into());
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
        cx.app.set_lyrics_artist(artist.into());
        cx.app.set_lyrics_title(title.into());
        cx.app.set_lyrics_can_save(
            self.current.is_some() && self.radio.is_none() && cx.local_path().is_some(),
        );
    }

    fn apply_size(&self, cx: &Ctx) {
        cx.app.set_lyrics_size(match cx.get("size").as_str() {
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
        self.rx = None;
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
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.update_info(cx);
        let key = self.path.clone().unwrap_or_default();
        std::thread::spawn(move || {
            let res = fetch_lrclib(&t);
            match &res {
                Ok(s) => write_cache(&cache, s.as_deref()),
                Err(e) => eprintln!("vivi: lyrics: {e}"),
            }
            let _ = tx.send((key, res.ok().flatten()));
        });
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
        let _ = super::tagger::backup(&super::tagger::backup_dir(), &p);
        let tags = super::tagger::Tags {
            lyrics: text,
            ..Default::default()
        };
        match super::tagger::write_tags(&p, &tags, None) {
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
        cx.app.set_ls_artist(a.into());
        cx.app.set_ls_title(t.into());
        cx.app.set_ls_preview("".into());
        cx.app.set_ls_selected(-1);
        self.search.results.clear();
        self.search_results(cx);
        cx.app.global::<crate::Overlays>().set_ls_open(true);
        if !cx.app.get_ls_title().is_empty() {
            self.search_run(cx);
        } else {
            cx.app.set_ls_status("Enter an artist and a title.".into());
        }
    }

    fn search_run(&mut self, cx: &Ctx) {
        let artist = cx.app.get_ls_artist().trim().to_string();
        let title = cx.app.get_ls_title().trim().to_string();
        if title.is_empty() {
            cx.app.set_ls_status("Enter a title.".into());
            return;
        }
        let (tx, rx) = channel();
        self.search.rx = Some(rx);
        cx.app.set_ls_busy(true);
        cx.app.set_ls_selected(-1);
        cx.app.set_ls_preview("".into());
        cx.app.set_ls_status("Searching LRCLIB…".into());
        std::thread::spawn(move || {
            let _ = tx.send(search_lrclib(&artist, &title));
        });
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
        cx.app.set_ls_results(ModelRc::new(VecModel::from(hits)));
    }

    fn search_tick(&mut self, cx: &Ctx) {
        let Some(rx) = &self.search.rx else { return };
        let Ok(res) = rx.try_recv() else { return };
        self.search.rx = None;
        cx.app.set_ls_busy(false);
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
                cx.app.set_ls_status(
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
                cx.app.set_ls_status(format!("Search failed: {e}").into());
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
        cx.app.set_ls_preview(text.trim().into());
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
                cx.app.set_lyrics_plain(true);
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
                    .set_lyrics_words(ModelRc::new(VecModel::<crate::KWord>::default()));
                cx.app.set_lyrics_rows(0);
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
            cx.app.set_lyrics_words(ModelRc::from(model.clone()));
            cx.app
                .set_lyrics_rows(rows.last().map(|r| r + 1).unwrap_or(0));
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
        r.rx = None;
        let station = cx
            .player
            .get("metadata/by-key/icy-name")
            .unwrap_or_default();
        let parsed = parse_stream_title(&raw, &station);
        let key = parsed
            .as_ref()
            .map(|(a, t)| format!("{a}\u{1f}{t}"))
            .unwrap_or_default();
        r.key = key.clone();
        r.song = parsed.clone();
        let Some((artist, title)) = parsed else {
            self.key.clear();
            self.show(cx, None); // ad / station ID / talk: hide
            return;
        };
        self.set_key(cx, format!("radio\u{1f}{key}"));
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
        let (tx, rx) = channel();
        if !offline {
            r.rx = Some(rx);
        }
        self.show(cx, None);
        if offline {
            return;
        }
        std::thread::spawn(move || {
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
            let _ = tx.send((key, res.ok().flatten()));
        });
    }

    fn radio_result(&mut self, cx: &Ctx) {
        let Some(r) = self.radio.as_mut() else { return };
        let Some(rx) = &r.rx else { return };
        let Ok((key, it)) = rx.try_recv() else { return };
        r.rx = None;
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
                cx.app.set_lyrics_plain(true);
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
        cx.app.set_lyrics_enabled(true);
        self.apply_size(cx);
        if cx.app.global::<crate::Playback>().get_has_media() {
            self.load(cx);
        }
    }
    fn disable(&mut self, cx: &Ctx) {
        cx.app.set_lyrics_enabled(false);
        cx.app.global::<crate::Overlays>().set_karaoke_open(false);
        cx.app.global::<crate::Overlays>().set_ls_open(false);
        self.rx = None;
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
                self.search.rx = None;
                cx.app.set_ls_busy(false);
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
        if let Some(rx) = &self.rx {
            if let Ok((key, res)) = rx.try_recv() {
                self.rx = None;
                if Some(&key) == self.path.as_ref() {
                    self.online_result(cx, res);
                }
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
            cx.app.set_lyrics_index(i.map(|i| i as i32).unwrap_or(-1));
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
        let tags = crate::plugins::tagger::Tags {
            lyrics: lrc.into(),
            ..Default::default()
        };
        crate::plugins::tagger::write_tags(&p, &tags, None).unwrap();
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
