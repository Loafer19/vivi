//! Lyrics sources: embedded tags, cache, per-track offsets, LRCLIB and the current track.

use super::*;

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
    let text = crate::plugins::auto_tagger::read_tags(path)
        .map(|(t, _)| t.lyrics)
        .unwrap_or_default();
    if is_synced(&text) {
        return Some(text);
    }
    let is_mp3 = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"));
    if is_mp3 {
        let sylt = crate::plugins::auto_tagger::read_sylt(path);
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

pub(crate) fn cache_key(t: &Track) -> String {
    crate::plugins::protocol::join(&[
        t.artist.to_lowercase(),
        t.title.to_lowercase(),
        t.album.to_lowercase(),
        t.duration.round().to_string(),
    ])
}

pub(crate) fn lyrics_dir() -> PathBuf {
    crate::store::cache_dir().join("lyrics")
}

/// Cache file stem for a track: `<md5>.lrc` (synced), `.txt` (plain), `.missing` (not
/// found, retried after [`lrclib::MISS_DAYS`]), `.instrumental` (LRCLIB lists it as instrumental).
pub(crate) fn cache_file(t: &Track) -> PathBuf {
    lyrics_dir().join(format!("{:x}.lrc", md5::compute(cache_key(t))))
}

/// Local lyrics files larger than this are ignored (a real LRC is a few KB).
pub const MAX_LYRICS_FILE: u64 = 1 << 20;

/// Text of a lyrics file, unless it is missing, not UTF-8 or larger than
/// [`MAX_LYRICS_FILE`].
pub(crate) fn read_lyrics_file(p: &Path) -> Option<String> {
    let len = std::fs::metadata(p).ok()?.len();
    (len <= MAX_LYRICS_FILE)
        .then(|| std::fs::read_to_string(p).ok())
        .flatten()
}

/// Manual pick for a track key (file path or radio "artist\u{1f}title").
pub(crate) fn manual_file(key: &str) -> PathBuf {
    lyrics_dir()
        .join("manual")
        .join(format!("{:x}.lrc", md5::compute(key)))
}

// ---------- per-track offsets ----------

pub(crate) fn offsets_file() -> PathBuf {
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

/// Artist/title/album/duration from tags (lofty), falling back to mpv metadata.
pub(crate) fn track_info(cx: &Ctx) -> Option<Track> {
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
