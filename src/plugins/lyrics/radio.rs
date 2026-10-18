//! Lyrics for network radio: stream-title parsing, filtering and LRCLIB lookups.

use super::*;

// ---------- Radio (network streams) ----------

/// Words that mark a stream title as an ad, jingle, station ID or talk segment.
pub(crate) const NOT_SONG: &[&str] = &[
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
pub(crate) fn clean_title(t: &str) -> String {
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

pub(crate) fn radio_cache_file(artist: &str, title: &str) -> PathBuf {
    let key = crate::plugins::protocol::join(&[
        "radio".to_string(),
        artist.to_lowercase(),
        title.to_lowercase(),
    ]);
    crate::store::cache_dir()
        .join("lyrics")
        .join(format!("{:x}.json", md5::compute(key)))
}

pub(crate) fn norm(s: &str) -> String {
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
pub(crate) fn plain_lines(it: &LrclibItem) -> Vec<String> {
    if let Some(p) = it.plain.as_ref().filter(|p| !p.trim().is_empty()) {
        return p.lines().map(|l| l.trim().to_string()).collect();
    }
    it.synced
        .as_deref()
        .map(|s| Lrc::parse(s).lines.into_iter().map(|(_, l)| l).collect())
        .unwrap_or_default()
}

/// Manual sync of synced lyrics on a live stream. The song's start is unknown (you can
/// tune in mid-song, stations change the title late), so the lyrics start as a list and
/// the user clicks the line being sung: that line's timestamp is taken as "now".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Anchor {
    /// Stream time (`time-pos`) at which the song's lyrics time is 0.
    base: Option<f64>,
}

impl Anchor {
    pub(crate) fn synced(&self) -> bool {
        self.base.is_some()
    }
    /// The line at `line_t` (lyrics time) is being sung at stream time `now`.
    /// Clicking another line later simply re-anchors.
    pub(crate) fn set(&mut self, now: f64, line_t: f64) {
        self.base = Some(now - line_t);
    }
    /// Back to the unsynced list (new song).
    pub(crate) fn clear(&mut self) {
        self.base = None;
    }
    /// Lyrics time at stream time `now`, minus the user's fine-tune `offset` (positive =
    /// lines later, like the per-track offset); None while unsynced.
    pub(crate) fn lyrics_time(&self, now: f64, offset: f64) -> Option<f64> {
        self.base.map(|b| now - b - offset)
    }
}

/// Whether the *Radio lyrics* setting lets synced lyrics be synced by clicking a line
/// (anything but "Plain lyrics" / "Off"; the old "Approximate sync" value maps here).
pub(crate) fn click_sync(mode: &str) -> bool {
    !matches!(mode, "Plain lyrics" | "Off")
}

#[derive(Default)]
pub(crate) struct Radio {
    /// Raw stream title last seen.
    pub(crate) title: String,
    /// Lookup key ("artist\u{1f}title") whose result is displayed / awaited.
    pub(crate) key: String,
    /// Parsed (artist, title) of the current song.
    pub(crate) song: Option<(String, String)>,
    pub(crate) mem: HashMap<String, Option<LrclibItem>>,
    pub(crate) task: crate::util::Task<(String, Option<LrclibItem>)>,
    pub(crate) last_poll: Option<std::time::Instant>,
}
