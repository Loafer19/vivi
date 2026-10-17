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
    let text = crate::plugins::tagger::read_tags(path)
        .map(|(t, _)| t.lyrics)
        .unwrap_or_default();
    if is_synced(&text) {
        return Some(text);
    }
    let is_mp3 = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"));
    if is_mp3 {
        let sylt = crate::plugins::tagger::read_sylt(path);
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
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        t.artist.to_lowercase(),
        t.title.to_lowercase(),
        t.album.to_lowercase(),
        t.duration.round()
    )
}

pub(crate) fn lyrics_dir() -> PathBuf {
    crate::store::cache_dir().join("lyrics")
}

/// Cache file stem for a track: `<md5>.lrc` (synced), `.txt` (plain), `.missing`.
pub(crate) fn cache_file(t: &Track) -> PathBuf {
    lyrics_dir().join(format!("{:x}.lrc", md5::compute(cache_key(t))))
}

/// Cached lyrics of a track (synced `.lrc` first, then plain `.txt`).
pub(crate) fn read_cache(stem: &Path) -> Option<String> {
    ["lrc", "txt"]
        .iter()
        .find_map(|e| std::fs::read_to_string(stem.with_extension(e)).ok())
        .filter(|s| !s.trim().is_empty())
}

pub(crate) fn write_cache(stem: &Path, text: Option<&str>) {
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

pub(crate) fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.trim().is_empty())
}

impl LrclibItem {
    pub(crate) fn has_lyrics(&self) -> bool {
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

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(12)))
        .user_agent(format!(
            "{} (https://github.com/Loafer19/vivi)",
            crate::plugins::USER_AGENT
        ))
        .build()
        .into()
}

/// Runs an LRCLIB request, retrying when the server is busy. LRCLIB answers uncached
/// lookups with `503 ServerOverloaded` (Retry-After: 1) under load and `429` when rate
/// limited; both are transient, so wait 1 s, then 2 s, before giving up.
pub(crate) fn call(
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
pub(crate) fn err(e: ureq::Error) -> String {
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
