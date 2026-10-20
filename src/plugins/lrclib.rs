//! LRCLIB (<https://lrclib.net>) client shared by Lyrics and the Auto-tagger: requests
//! with one User-Agent and timeout, retries when LRCLIB is busy, result ranking
//! ([`pick`]) and the on-disk answer cache, including remembered misses that expire
//! ([`cached_miss`]). Callers choose the cache location (Lyrics keeps it in its cache
//! folder; the Auto-tagger review asks LRCLIB directly).

use std::path::Path;
use std::time::Duration;

const API: &str = "https://lrclib.net/api";

/// What LRCLIB is asked about: a song's artist and title, plus album and length
/// (seconds, 0 = unknown) when known, which pick the right version.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: f64,
}

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

/// What an LRCLIB lookup found for a track.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched {
    /// LRC text (synced) or plain text.
    Lyrics(String),
    /// LRCLIB lists the track as instrumental: there are no lyrics to look for.
    Instrumental,
    NotFound,
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
    let ua = format!(
        "{} (https://github.com/Loafer19/vivi)",
        crate::plugins::USER_AGENT
    );
    crate::plugins::web::agent_as(12, &ua, true)
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

/// LRCLIB `/api/search` by artist and title (title only when the artist is empty);
/// only entries with lyrics.
pub fn search(artist: &str, title: &str) -> Result<Vec<LrclibItem>, String> {
    Ok(search_all(artist, title)?
        .into_iter()
        .filter(|i| i.has_lyrics())
        .collect())
}

/// LRCLIB `/api/search`, every entry (instrumental ones too). An empty `artist` searches
/// `title` as free text (`q`).
pub fn search_all(artist: &str, title: &str) -> Result<Vec<LrclibItem>, String> {
    let a = agent();
    let mut r = call(|| {
        let req = a.get(format!("{API}/search"));
        if artist.trim().is_empty() {
            req.query("q", title)
        } else {
            req.query("artist_name", artist).query("track_name", title)
        }
        .call()
    })
    .map_err(err)?;
    r.body_mut().read_json().map_err(|e| e.to_string())
}

/// Lyrics from LRCLIB: `/api/get` (exact match), then `/api/search` (synced first,
/// closest duration). Plain lyrics are returned when no synced ones exist;
/// [`Fetched::Instrumental`] when LRCLIB marks the track as instrumental.
pub fn fetch(t: &Track) -> Result<Fetched, String> {
    let exact = get(t)?;
    if let Some(s) = exact.as_ref().and_then(|it| non_empty(&it.synced)) {
        return Ok(Fetched::Lyrics(s.to_string()));
    }
    let items = search_all(&t.artist, &t.title)?;
    Ok(pick(exact, items, t.duration))
}

/// LRCLIB `/api/get`: the entry for exactly this artist and title (album and duration
/// when known), or None (404).
pub fn get(t: &Track) -> Result<Option<LrclibItem>, String> {
    let a = agent();
    let send = || {
        let mut req = a
            .get(format!("{API}/get"))
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
    match call(send) {
        Ok(mut r) => Ok(Some(r.body_mut().read_json().map_err(|e| e.to_string())?)),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(err(e)),
    }
}

/// The answer for a track of `duration` s from the `/api/get` match (`exact`, if any) and
/// the search results: synced lyrics of the best-ranked result, else plain lyrics (the
/// exact match's first), else instrumental when the exact match, or a search result
/// within 3 s of the track's length, is marked instrumental.
pub fn pick(exact: Option<LrclibItem>, items: Vec<LrclibItem>, duration: f64) -> Fetched {
    let instrumental = |i: &LrclibItem| i.instrumental == Some(true) && !i.has_lyrics();
    let close = |i: &LrclibItem| match (i.duration, duration > 0.0) {
        (Some(d), true) => (d - duration).abs() <= 3.0,
        _ => false,
    };
    let (mut lyr, other): (Vec<LrclibItem>, Vec<LrclibItem>) =
        items.into_iter().partition(|i| i.has_lyrics());
    rank(&mut lyr, duration);
    let plain = exact
        .as_ref()
        .and_then(|e| non_empty(&e.plain))
        .map(String::from);
    if let Some(s) = lyr
        .first()
        .and_then(|i| non_empty(&i.synced).map(String::from))
        .or(plain)
        .or_else(|| lyr.first().and_then(|i| i.best_text()))
    {
        return Fetched::Lyrics(s);
    }
    if exact.as_ref().is_some_and(instrumental) || other.iter().any(|i| instrumental(i) && close(i))
    {
        return Fetched::Instrumental;
    }
    Fetched::NotFound
}

// ---------- answer cache ----------

/// Cached lyrics of a track (synced `.lrc` first, then plain `.txt`).
pub(crate) fn read_cache(stem: &Path) -> Option<String> {
    ["lrc", "txt"]
        .iter()
        .find_map(|e| std::fs::read_to_string(stem.with_extension(e)).ok())
        .filter(|s| !s.trim().is_empty())
}

pub(crate) fn write_cache(stem: &Path, found: &Fetched) {
    if let Some(d) = stem.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let (ext, text) = match found {
        Fetched::Lyrics(s) if crate::plugins::lyrics::is_synced(s) => ("lrc", s.as_str()),
        Fetched::Lyrics(s) => ("txt", s.as_str()),
        Fetched::Instrumental => ("instrumental", ""),
        Fetched::NotFound => ("missing", ""),
    };
    let _ = std::fs::write(stem.with_extension(ext), text);
}

/// A "not found" answer is trusted for a week, then LRCLIB is asked again (it keeps
/// growing); "instrumental" for a month.
pub const MISS_DAYS: u64 = 7;
pub const INSTRUMENTAL_DAYS: u64 = 30;

/// Whether `p` exists and was written less than `days` ago.
pub(crate) fn fresh(p: &Path, days: u64) -> bool {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(days * 86_400))
}

/// A remembered negative answer for a track: instrumental or not found (while fresh).
pub(crate) fn cached_miss(stem: &Path) -> Option<Fetched> {
    if fresh(&stem.with_extension("instrumental"), INSTRUMENTAL_DAYS) {
        Some(Fetched::Instrumental)
    } else if fresh(&stem.with_extension("missing"), MISS_DAYS) {
        Some(Fetched::NotFound)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn pick_lyrics_or_instrumental() {
        let it = |d: f64, synced: Option<&str>, plain: Option<&str>, inst: bool| LrclibItem {
            duration: Some(d),
            synced: synced.map(String::from),
            plain: plain.map(String::from),
            instrumental: Some(inst),
            ..Default::default()
        };
        // Synced lyrics of the closest result win over the exact match's plain ones.
        let exact = it(180.0, None, Some("plain"), false);
        let v = vec![
            it(300.0, Some("[00:01.00]far"), None, false),
            it(181.0, Some("[00:01.00]near"), None, false),
        ];
        assert_eq!(
            pick(Some(exact.clone()), v, 180.0),
            Fetched::Lyrics("[00:01.00]near".into())
        );
        // Only plain lyrics: the exact match's first.
        let v = vec![it(181.0, None, Some("other"), false)];
        assert_eq!(
            pick(Some(exact), v.clone(), 180.0),
            Fetched::Lyrics("plain".into())
        );
        assert_eq!(pick(None, v, 180.0), Fetched::Lyrics("other".into()));
        // Instrumental: the exact match says so, or a result of about the same length.
        assert_eq!(
            pick(Some(it(67.0, None, None, true)), vec![], 67.0),
            Fetched::Instrumental
        );
        assert_eq!(
            pick(None, vec![it(68.5, None, None, true)], 67.0),
            Fetched::Instrumental
        );
        assert_eq!(
            pick(None, vec![it(120.0, None, None, true)], 67.0),
            Fetched::NotFound,
            "other length"
        );
        // Any lyrics beat an instrumental flag.
        let v = vec![
            it(67.0, None, None, true),
            it(90.0, None, Some("words"), false),
        ];
        assert_eq!(pick(None, v, 67.0), Fetched::Lyrics("words".into()));
        assert_eq!(pick(None, vec![], 67.0), Fetched::NotFound);
    }

    #[test]
    fn cache_misses_expire() {
        let d = std::env::temp_dir().join(format!("vivi-lrclib-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let stem = d.join("abc.lrc");
        assert_eq!(cached_miss(&stem), None);
        write_cache(&stem, &Fetched::NotFound);
        assert_eq!(cached_miss(&stem), Some(Fetched::NotFound));
        // A week-old "not found" is asked again.
        let old = std::time::SystemTime::now() - Duration::from_secs(MISS_DAYS * 86_400 + 60);
        std::fs::File::options()
            .write(true)
            .open(stem.with_extension("missing"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(cached_miss(&stem), None);
        write_cache(&stem, &Fetched::Instrumental);
        assert_eq!(cached_miss(&stem), Some(Fetched::Instrumental));
        write_cache(&stem, &Fetched::Lyrics("[00:01.00]x".into()));
        assert_eq!(read_cache(&stem).as_deref(), Some("[00:01.00]x"));
        let _ = std::fs::remove_dir_all(&d);
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
        assert!(matches!(fetch(&t).unwrap(), Fetched::Lyrics(_)));
    }

    #[test]
    fn retries_when_busy() {
        // 503 / 429 are retried twice (1 s, then 2 s); anything else is returned at once.
        let mut n = 0;
        let r = call(|| {
            n += 1;
            Err(ureq::Error::StatusCode(if n == 1 { 503 } else { 404 }))
        });
        assert!(matches!(r, Err(ureq::Error::StatusCode(404))));
        assert_eq!(n, 2);
        let mut n = 0;
        let r = call(|| {
            n += 1;
            Err(ureq::Error::StatusCode(429))
        });
        assert!(matches!(r, Err(ureq::Error::StatusCode(429))));
        assert_eq!(n, 3, "gives up after two retries");
        let mut n = 0;
        let _ = call(|| {
            n += 1;
            Err(ureq::Error::StatusCode(500))
        });
        assert_eq!(n, 1, "server errors are not retried");
    }

    #[test]
    fn readable_errors() {
        assert!(err(ureq::Error::StatusCode(503)).contains("busy"));
        assert!(err(ureq::Error::StatusCode(429)).contains("rate limit"));
        assert!(err(ureq::Error::StatusCode(500)).contains("500"));
    }

    #[test]
    fn items_parse() {
        let j = r#"[{"id":1,"trackName":"T","artistName":"A","albumName":"B","duration":67.0,
            "instrumental":true,"plainLyrics":null,"syncedLyrics":null},
            {"trackName":"U","artistName":"A","plainLyrics":"  ","syncedLyrics":"[00:01.00]x"}]"#;
        let v: Vec<LrclibItem> = serde_json::from_str(j).unwrap();
        assert_eq!(v[0].instrumental, Some(true));
        assert!(!v[0].has_lyrics());
        assert_eq!(v[1].best_text().as_deref(), Some("[00:01.00]x"));
        assert_eq!(pick(None, v, 67.0), Fetched::Lyrics("[00:01.00]x".into()));
    }
}
