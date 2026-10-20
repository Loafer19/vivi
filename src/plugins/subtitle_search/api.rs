//! OpenSubtitles API: movie hash, query parsing, search, download, sidecars and previews.

use super::*;

/// OpenSubtitles hash: file size + the sums of the first and last 64 KiB as
/// little-endian u64 words (wrapping), as 16 hex digits.
pub fn moviehash(path: &Path) -> std::io::Result<String> {
    const CHUNK: u64 = 65536;
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    if size < CHUNK * 2 {
        return Err(std::io::Error::other("file too small for a hash"));
    }
    let mut h = size;
    let mut buf = vec![0u8; CHUNK as usize];
    for off in [0, size - CHUNK] {
        f.seek(SeekFrom::Start(off))?;
        f.read_exact(&mut buf)?;
        for w in buf.as_chunks::<8>().0 {
            h = h.wrapping_add(u64::from_le_bytes(*w));
        }
    }
    Ok(format!("{h:016x}"))
}

/// Search query from a file name: "The.Wand.S01E05.1080p.WEB.mkv" → "The Wand S01E05".
pub fn query_from_name(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let junk = [
        "1080p", "720p", "2160p", "480p", "4k", "web", "webrip", "web-dl", "webdl", "bluray",
        "bdrip", "brrip", "hdtv", "x264", "x265", "h264", "h265", "hevc", "aac", "dts", "hdr",
        "remux", "proper", "repack",
    ];
    let words: Vec<&str> = stem
        .split(['.', '_', ' ', '[', ']'])
        .filter(|w| !w.is_empty())
        .collect();
    let end = words
        .iter()
        .position(|w| junk.contains(&w.to_lowercase().as_str()))
        .unwrap_or(words.len());
    words[..end]
        .join(" ")
        .trim_end_matches(['-', ' '])
        .to_string()
}

/// IMDb id from a query: "tt0133093", "TT133093" or an imdb.com/title/tt… URL → 133093.
/// Plain numbers stay a title ("1917").
pub fn imdb_id(q: &str) -> Option<u64> {
    let q = q.trim().to_lowercase();
    let q = match q.find("imdb.com/title/") {
        Some(i) => &q[i + "imdb.com/title/".len()..],
        None => q.as_str(),
    };
    let digits = q.strip_prefix("tt")?;
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    let rest = &q[2 + digits.len()..];
    if digits.is_empty() || !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('?')) {
        return None;
    }
    digits.parse().ok()
}

/// Comma-separated language codes, lowercase, sorted, without duplicates.
pub fn norm_langs(s: &str) -> String {
    let mut l: Vec<String> = s
        .split([',', ' ', ';'])
        .map(|x| x.trim().to_lowercase())
        .filter(|x| !x.is_empty())
        .collect();
    l.sort();
    l.dedup();
    l.join(",")
}

/// Add `code` to the language list, or remove it when present.
pub fn toggle_lang(langs: &str, code: &str) -> String {
    let code = code.trim().to_lowercase();
    let mut l: Vec<String> = norm_langs(langs).split(',').map(String::from).collect();
    l.retain(|x| !x.is_empty());
    if let Some(i) = l.iter().position(|x| *x == code) {
        l.remove(i);
    } else if !code.is_empty() {
        l.push(code);
    }
    norm_langs(&l.join(","))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hit {
    pub file_id: i64,
    pub file_name: String,
    pub release: String,
    pub language: String,
    pub downloads: i64,
    pub rating: f64,
    pub votes: i64,
    pub hearing_impaired: bool,
    pub fps: f64,
    pub hash_match: bool,
    pub trusted: bool,
    pub machine: bool,
    /// "The Matrix (1999)", "Show S01E05 · Episode title".
    pub title: String,
    pub uploader: String,
    pub uploaded: String,
}

impl Hit {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "file_id": self.file_id, "file_name": self.file_name, "release": self.release,
            "language": self.language, "downloads": self.downloads, "rating": self.rating,
            "hearing_impaired": self.hearing_impaired, "fps": self.fps,
            "hash_match": self.hash_match, "trusted": self.trusted,
            "machine_translated": self.machine, "title": self.title,
        })
    }
}

pub(crate) fn feature_title(f: &serde_json::Value) -> String {
    let s = |k: &str| f[k].as_str().unwrap_or("").trim().to_string();
    let year = f["year"].as_i64().filter(|y| *y > 0);
    match (f["season_number"].as_i64(), f["episode_number"].as_i64()) {
        (Some(se), Some(ep)) if se > 0 || ep > 0 => {
            let show = Some(s("parent_title"))
                .filter(|x| !x.is_empty())
                .unwrap_or_else(|| s("movie_name"));
            let t = s("title");
            let mut out = format!("{show} S{se:02}E{ep:02}");
            if !t.is_empty() && t != show {
                out.push_str(&format!(" · {t}"));
            }
            out
        }
        _ => {
            let t = Some(s("title"))
                .filter(|x| !x.is_empty())
                .unwrap_or_else(|| s("movie_name"));
            match year {
                Some(y) if !t.is_empty() && !t.contains(&y.to_string()) => format!("{t} ({y})"),
                _ => t,
            }
        }
    }
}

/// Parse `/subtitles` JSON into hits (one per file; hash matches first, then by downloads).
pub fn parse_search(v: &serde_json::Value) -> Vec<Hit> {
    let mut out = Vec::new();
    for d in v["data"].as_array().into_iter().flatten() {
        let a = &d["attributes"];
        for f in a["files"].as_array().into_iter().flatten() {
            let Some(id) = f["file_id"].as_i64() else {
                continue;
            };
            let release = a["release"].as_str().unwrap_or("").trim().to_string();
            out.push(Hit {
                file_id: id,
                file_name: f["file_name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| {
                        if release.is_empty() {
                            "subtitle".into()
                        } else {
                            release.clone()
                        }
                    }),
                release,
                language: a["language"].as_str().unwrap_or("").to_string(),
                downloads: a["download_count"].as_i64().unwrap_or(0),
                rating: a["ratings"].as_f64().unwrap_or(0.0),
                votes: a["votes"].as_i64().unwrap_or(0),
                hearing_impaired: a["hearing_impaired"].as_bool().unwrap_or(false),
                fps: a["fps"].as_f64().unwrap_or(0.0),
                hash_match: a["moviehash_match"].as_bool().unwrap_or(false),
                trusted: a["from_trusted"].as_bool().unwrap_or(false),
                machine: a["ai_translated"].as_bool().unwrap_or(false)
                    || a["machine_translated"].as_bool().unwrap_or(false),
                title: feature_title(&a["feature_details"]),
                uploader: a["uploader"]["name"].as_str().unwrap_or("").to_string(),
                uploaded: a["upload_date"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(10)
                    .collect(),
            });
        }
    }
    sort_hits(&mut out);
    out
}

pub(crate) fn sort_hits(v: &mut [Hit]) {
    v.sort_by(|a, b| {
        b.hash_match
            .cmp(&a.hash_match)
            .then(a.machine.cmp(&b.machine))
            .then(b.downloads.cmp(&a.downloads))
    });
}

/// Hash results first (all marked as hash matches), then the name results not already
/// listed; sorted like [`parse_search`].
pub fn merge_hits(by_hash: Vec<Hit>, by_name: Vec<Hit>) -> Vec<Hit> {
    let mut out: Vec<Hit> = by_hash
        .into_iter()
        .map(|mut h| {
            h.hash_match = true;
            h
        })
        .collect();
    for h in by_name {
        if !out.iter().any(|x| x.file_id == h.file_id) {
            out.push(h);
        }
    }
    sort_hits(&mut out);
    out
}

#[derive(Clone)]
pub struct Creds {
    pub(crate) key: String,
    pub(crate) user: String,
    pub(crate) pass: String,
}

pub(crate) fn agent() -> ureq::Agent {
    crate::plugins::web::agent_as(20, &format!("Vivi v{}", env!("CARGO_PKG_VERSION")), false)
}

/// The API base: [`API`], or `VIVI_OPENSUBTITLES_API` (a local mock or a mirror) when it
/// passes [`safe_url`].
pub(crate) fn api_base() -> String {
    std::env::var(API_ENV)
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| safe_url(s))
        .unwrap_or_else(|| API.to_string())
}

/// URLs Vivi sends the API key to or downloads from: https, or plain http only to this
/// machine (`localhost`, `127.0.0.1`, `[::1]`). No user info in the authority.
pub(crate) fn safe_url(u: &str) -> bool {
    let l = u.trim().to_ascii_lowercase();
    let (https, rest) = match (l.strip_prefix("https://"), l.strip_prefix("http://")) {
        (Some(r), _) => (true, r),
        (_, Some(r)) => (false, r),
        _ => return false,
    };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    if auth.is_empty() || auth.contains('@') {
        return false;
    }
    if https {
        return true;
    }
    let host = if auth.starts_with('[') {
        auth.split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        auth.split(':').next().unwrap_or("").to_string()
    };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
}

/// A network error as a short message, never with a URL (download links carry a
/// one-time token).
pub(crate) fn net_err(e: ureq::Error) -> String {
    use ureq::Error as E;
    match e {
        E::StatusCode(c) => api_err(c, ""),
        E::Timeout(_) => "OpenSubtitles did not answer in time; try again".into(),
        E::HostNotFound | E::ConnectionFailed | E::Io(_) => {
            "Could not reach OpenSubtitles; check the internet connection".into()
        }
        E::TooManyRedirects | E::RedirectFailed => "OpenSubtitles: unexpected redirect".into(),
        E::BodyExceedsLimit(_) => "OpenSubtitles: answer too large".into(),
        _ => "OpenSubtitles: network error".into(),
    }
}

pub(crate) fn api_err(code: u16, body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v["message"]
                .as_str()
                .or(v["errors"][0].as_str())
                .map(String::from)
        })
        .unwrap_or_default();
    match code {
        401 | 403 if msg.is_empty() => "OpenSubtitles rejected the API key or login".into(),
        401 | 403 => format!("OpenSubtitles: {msg} (check the API key and login)"),
        406 if msg.is_empty() => "OpenSubtitles: daily download limit reached".into(),
        429 => "OpenSubtitles: too many requests, try again later".into(),
        _ if msg.is_empty() => format!("OpenSubtitles error {code}"),
        _ => format!("OpenSubtitles: {msg}"),
    }
}

/// What to search for: the file hash, an IMDb id or a text query (any combination),
/// with the season / episode or year found in the text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub hash: Option<String>,
    pub imdb: Option<u64>,
    pub text: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub year: Option<u32>,
}

/// Season and episode of a word: `S01E05`, `s1e5`, `S01E05E06` (first one), `1x05`.
fn episode_of(w: &str) -> Option<(u32, u32)> {
    let w = w.to_ascii_lowercase();
    let num = |s: &str, max: usize| -> Option<(u32, usize)> {
        let d: String = s.chars().take_while(char::is_ascii_digit).collect();
        if d.is_empty() || d.len() > max {
            return None;
        }
        Some((d.parse().ok()?, d.len()))
    };
    if let Some(r) = w.strip_prefix('s') {
        let (se, n) = num(r, 2)?;
        let r = r[n..].strip_prefix('e')?;
        let (ep, m) = num(r, 3)?;
        let rest = &r[m..];
        return (rest.is_empty() || rest.starts_with('e') || rest.starts_with('-'))
            .then_some((se, ep));
    }
    let (se, n) = num(&w, 2)?;
    let r = w[n..].strip_prefix('x')?;
    let (ep, m) = num(r, 3)?;
    r[m..].is_empty().then_some((se, ep))
}

impl Query {
    /// Text query split into the title and its season / episode ("The Wand S01E05",
    /// "Show 2x03") or release year ("Movie (2020)", "Movie 2020"). A year alone stays the
    /// title ("1917"); IMDb ids are kept as they are.
    pub fn from_text(text: &str) -> Query {
        let mut q = Query {
            imdb: imdb_id(text),
            text: text.trim().to_string(),
            ..Default::default()
        };
        if q.imdb.is_some() {
            return q;
        }
        let words: Vec<&str> = q.text.split_whitespace().collect();
        if let Some(i) = words.iter().position(|w| episode_of(w).is_some()) {
            if i > 0 {
                let (se, ep) = episode_of(words[i]).unwrap_or_default();
                q.season = Some(se);
                q.episode = Some(ep);
                q.text = words[..i].join(" ");
            }
            return q;
        }
        if words.len() > 1 {
            let last = words[words.len() - 1].trim_matches(['(', ')', '[', ']']);
            if let Ok(y) = last.parse::<u32>() {
                if last.len() == 4 && (1900..=2099).contains(&y) {
                    q.year = Some(y);
                    q.text = words[..words.len() - 1].join(" ");
                }
            }
        }
        q
    }
    /// Whether a season, episode or year narrows the text search.
    pub fn narrowed(&self) -> bool {
        self.season.is_some() || self.episode.is_some() || self.year.is_some()
    }
    /// The same search with the plain text only (when the narrowed one finds nothing).
    pub fn plain(&self, text: &str) -> Query {
        Query {
            hash: self.hash.clone(),
            imdb: self.imdb,
            text: text.trim().to_string(),
            ..Default::default()
        }
    }
}

/// Query parameters in the order the API wants (alphabetical, lowercase values).
pub(crate) fn search_params(q: &Query, langs: &str) -> Vec<(&'static str, String)> {
    let mut p = Vec::new();
    let text = q.imdb.is_none() && !q.text.trim().is_empty();
    if let Some(e) = q.episode.filter(|_| text) {
        p.push(("episode_number", e.to_string()));
    }
    if let Some(i) = q.imdb {
        p.push(("imdb_id", i.to_string()));
    }
    let l = norm_langs(langs);
    if !l.is_empty() {
        p.push(("languages", l));
    }
    if let Some(h) = &q.hash {
        p.push(("moviehash", h.to_lowercase()));
    }
    if text {
        p.push(("query", q.text.trim().to_lowercase()));
    }
    if let Some(s) = q.season.filter(|_| text) {
        p.push(("season_number", s.to_string()));
    }
    if let Some(y) = q.year.filter(|_| text) {
        p.push(("year", y.to_string()));
    }
    p
}

pub(crate) fn search(c: &Creds, q: &Query, langs: &str) -> Result<Vec<Hit>, String> {
    let a = agent();
    let mut req = a
        .get(format!("{}/subtitles", api_base()))
        .header("Api-Key", &c.key)
        .header("Accept", "application/json");
    // OpenSubtitles answers 301 unless parameters are lowercase and sorted by name.
    for (k, v) in search_params(q, langs) {
        req = req.query(k, v);
    }
    let mut r = req.call().map_err(net_err)?;
    let body = r.body_mut().read_to_string().map_err(net_err)?;
    if !r.status().is_success() {
        return Err(api_err(r.status().as_u16(), &body));
    }
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "OpenSubtitles: unreadable answer".to_string())?;
    Ok(parse_search(&v))
}

/// Hash search first, then by IMDb id / name; `partial` gets the hash results early.
/// Returns the merged list and an error of the name search (kept when hash hits exist).
pub fn search_all(
    c: &Creds,
    path: Option<&Path>,
    text: &str,
    langs: &str,
    partial: impl Fn(&[Hit]),
) -> Result<Vec<Hit>, String> {
    let hash = path.and_then(|p| moviehash(p).ok());
    let mut by_hash = Vec::new();
    if let Some(h) = &hash {
        let q = Query {
            hash: Some(h.clone()),
            ..Default::default()
        };
        match search(c, &q, langs) {
            Ok(v) => {
                by_hash = v.into_iter().filter(|h| h.hash_match).collect();
                if !by_hash.is_empty() {
                    partial(&by_hash);
                }
            }
            Err(e) if text.trim().is_empty() => return Err(e),
            Err(_) => {}
        }
    }
    let q = Query::from_text(text);
    if q.imdb.is_none() && q.text.trim().is_empty() {
        return Ok(by_hash);
    }
    // A season / episode or year that matches nothing: the plain text instead.
    let found = match search(c, &q, langs) {
        Ok(v) if v.is_empty() && q.narrowed() => search(c, &q.plain(text), langs),
        r => r,
    };
    match found {
        Ok(v) => Ok(merge_hits(by_hash, v)),
        Err(_) if !by_hash.is_empty() => Ok(merge_hits(by_hash, Vec::new())),
        Err(e) => Err(e),
    }
}

/// The exact (hash) match to load automatically: made for this file, not machine
/// translated, most downloaded (hearing-impaired ones only when there is nothing else).
pub fn best_exact(hits: &[Hit]) -> Option<Hit> {
    hits.iter()
        .filter(|h| h.hash_match && !h.machine)
        .max_by_key(|h| (!h.hearing_impaired, h.downloads))
        .cloned()
}

/// *Load exact matches automatically*: search `path` by its hash in `langs` and download
/// the [`best_exact`] match into `dir`. `None` when there is no exact match.
pub fn auto_pick(
    c: &Creds,
    path: &Path,
    langs: &str,
    dir: &Path,
) -> Result<Option<(Hit, PathBuf, Option<i64>)>, String> {
    let Ok(hash) = moviehash(path) else {
        return Ok(None);
    };
    let q = Query {
        hash: Some(hash),
        ..Default::default()
    };
    let Some(best) = best_exact(&search(c, &q, langs)?) else {
        return Ok(None);
    };
    let (p, left) = download(c, best.file_id, dir)?;
    Ok(Some((best, p, left)))
}

/// A subtitle already downloaded into the cache (`<file_id>-<name>`).
pub(crate) fn cached(dir: &Path, file_id: i64) -> Option<PathBuf> {
    let pre = format!("{file_id}-");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.starts_with(&pre) && e.path().is_file()).then(|| e.path())
    })
}

/// A login: its token and the API base it belongs to (VIP accounts get their own host).
#[derive(Clone)]
struct Session {
    who: u64,
    token: String,
    base: String,
    at: std::time::Instant,
}

/// Logins are reused for 12 hours (OpenSubtitles asks clients not to log in for every
/// download; tokens last 24 hours). In memory only.
static SESSION: std::sync::Mutex<Option<Session>> = std::sync::Mutex::new(None);
const SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(12 * 3600);

fn who(c: &Creds, base: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (base, &c.key, &c.user, &c.pass).hash(&mut h);
    h.finish()
}

/// POST JSON to the API; errors keep the HTTP status (0: network) for retries.
fn post(
    a: &ureq::Agent,
    key: &str,
    url: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> Result<serde_json::Value, (u16, String)> {
    let mut req = a
        .post(url)
        .header("Api-Key", key)
        .header("Accept", "application/json");
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let mut r = req.send_json(body).map_err(|e| (0, net_err(e)))?;
    let code = r.status().as_u16();
    let s = r.body_mut().read_to_string().map_err(|e| (0, net_err(e)))?;
    if !r.status().is_success() {
        return Err((code, api_err(code, &s)));
    }
    serde_json::from_str::<serde_json::Value>(&s)
        .map_err(|_| (code, "OpenSubtitles: unreadable answer".into()))
}

/// The cached login for these credentials, or a new one (`fresh`: always log in again).
fn login(a: &ureq::Agent, c: &Creds, base: &str, fresh: bool) -> Result<Session, String> {
    let id = who(c, base);
    let mut cache = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = cache
        .as_ref()
        .filter(|s| !fresh && s.who == id && s.at.elapsed() < SESSION_TTL)
    {
        return Ok(s.clone());
    }
    let v = post(
        a,
        &c.key,
        &format!("{base}/login"),
        None,
        serde_json::json!({ "username": c.user, "password": c.pass }),
    )
    .map_err(|(_, m)| m)?;
    let token = v["token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or("OpenSubtitles: login failed")?
        .to_string();
    let s = Session {
        who: id,
        token,
        base: session_base(base, v["base_url"].as_str().unwrap_or("")),
        at: std::time::Instant::now(),
    };
    *cache = Some(s.clone());
    Ok(s)
}

/// API base after a login: the account's own host (`vip-api.opensubtitles.com`) when
/// it is an OpenSubtitles host and the default API is used; else `base`.
pub(crate) fn session_base(base: &str, host: &str) -> String {
    let h = host
        .trim()
        .trim_start_matches("https://")
        .trim_end_matches('/');
    let h = h.split('/').next().unwrap_or("").to_ascii_lowercase();
    let ours = h == "opensubtitles.com" || h.ends_with(".opensubtitles.com");
    if base == API
        && ours
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
    {
        format!("https://{h}/api/v1")
    } else {
        base.to_string()
    }
}

/// Download a subtitle into `dir` (reusing a cached copy). Returns the file and the
/// downloads left today when the API said so.
pub fn download(c: &Creds, file_id: i64, dir: &Path) -> Result<(PathBuf, Option<i64>), String> {
    download_from(&api_base(), c, file_id, dir)
}

/// [`download`] from the API at `base`.
pub(crate) fn download_from(
    base: &str,
    c: &Creds,
    file_id: i64,
    dir: &Path,
) -> Result<(PathBuf, Option<i64>), String> {
    if let Some(p) = cached(dir, file_id) {
        return Ok((p, None));
    }
    let a = agent();
    let body = serde_json::json!({ "file_id": file_id });
    let v = if c.user.is_empty() {
        post(&a, &c.key, &format!("{base}/download"), None, body).map_err(|(_, m)| m)?
    } else {
        let s = login(&a, c, base, false)?;
        let url = |s: &Session| format!("{}/download", s.base);
        match post(&a, &c.key, &url(&s), Some(&s.token), body.clone()) {
            // An expired or revoked token: log in once more.
            Err((401, _)) => {
                let s = login(&a, c, base, true)?;
                post(&a, &c.key, &url(&s), Some(&s.token), body).map_err(|(_, m)| m)?
            }
            r => r.map_err(|(_, m)| m)?,
        }
    };
    let link = v["link"]
        .as_str()
        .filter(|l| safe_url(l))
        .ok_or_else(|| api_err(0, &v.to_string()))?;
    let remaining = v["remaining"].as_i64();
    let name = v["file_name"].as_str().unwrap_or("subtitle.srt");
    let mut name: String = name
        .chars()
        .map(|ch| {
            if "/\\:".contains(ch) || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .take(120)
        .collect();
    if !has_ext(Path::new(&name), SUB_EXT) {
        name.push_str(".srt");
    }
    let mut r = a.get(link).call().map_err(net_err)?;
    if !r.status().is_success() {
        return Err(format!(
            "Subtitle download failed (HTTP {})",
            r.status().as_u16()
        ));
    }
    let bytes = r
        .body_mut()
        .with_config()
        .limit(20 * 1024 * 1024)
        .read_to_vec()
        .map_err(net_err)?;
    if bytes.is_empty() {
        return Err("OpenSubtitles sent an empty subtitle file".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{file_id}-{name}"));
    // Temp file + rename: a cut-off download is never taken for a cached subtitle (the
    // temp name does not start with the file id).
    let tmp = dir.join(format!(".{file_id}.part"));
    std::fs::write(&tmp, &bytes)
        .and_then(|_| std::fs::rename(&tmp, &out))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })?;
    Ok((out, remaining))
}

/// A language code safe for a file name: lowercase letters, digits and `-` (at most 12).
pub(crate) fn safe_lang(lang: &str) -> String {
    let l = lang.trim().to_lowercase();
    if l.is_empty()
        || l.len() > 12
        || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        || l.starts_with('-')
    {
        return String::new();
    }
    l
}

/// Where to save a subtitle next to `video`: `<stem>.<lang>.<ext>`, or with the
/// OpenSubtitles file id when a different file already has that name.
pub fn sidecar_path(
    video: &Path,
    lang: &str,
    ext: &str,
    file_id: i64,
    taken: impl Fn(&Path) -> bool,
) -> PathBuf {
    let stem = video
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "subtitle".into());
    let dir = video.parent().unwrap_or(Path::new("."));
    let lang = safe_lang(lang);
    let base = if lang.is_empty() {
        stem
    } else {
        format!("{stem}.{lang}")
    };
    let p = dir.join(format!("{base}.{ext}"));
    if taken(&p) {
        dir.join(format!("{base}.{file_id}.{ext}"))
    } else {
        p
    }
}

/// First cues of a subtitle as "0:01:02  text" lines (SRT / VTT / ASS; other formats
/// as their first lines).
pub fn preview_text(text: &str, max: usize) -> String {
    let text = text.trim_start_matches('\u{feff}');
    let mut out: Vec<String> = Vec::new();
    let short = |t: &str| {
        // "00:01:02,500" / "0:01:02.50" → "0:01:02"
        let t = t.trim().split([',', '.']).next().unwrap_or("").to_string();
        t.strip_prefix('0')
            .filter(|r| r.starts_with(|c: char| c.is_ascii_digit()))
            .map(String::from)
            .unwrap_or(t)
    };
    let strip_tags = |s: &str| {
        let mut o = String::new();
        let mut depth = 0;
        for ch in s.chars() {
            match ch {
                '<' | '{' => depth += 1,
                '>' | '}' if depth > 0 => depth -= 1,
                _ if depth == 0 => o.push(ch),
                _ => {}
            }
        }
        o.replace("\\N", " ").replace("\\n", " ").trim().to_string()
    };
    if text.contains("[Events]") {
        for l in text.lines().filter(|l| l.starts_with("Dialogue:")) {
            let f: Vec<&str> = l.splitn(10, ',').collect();
            if f.len() == 10 {
                let t = strip_tags(f[9]);
                if !t.is_empty() {
                    out.push(format!("{}  {t}", short(f[1])));
                }
            }
            if out.len() >= max {
                break;
            }
        }
    } else if text.contains("-->") {
        let lines: Vec<&str> = text.lines().collect();
        let mut i = 0;
        while i < lines.len() && out.len() < max {
            if let Some((a, _)) = lines[i].split_once("-->") {
                let mut words = Vec::new();
                i += 1;
                while i < lines.len() && !lines[i].trim().is_empty() {
                    words.push(strip_tags(lines[i]));
                    i += 1;
                }
                let t = words.join(" ").trim().to_string();
                if !t.is_empty() {
                    out.push(format!("{}  {t}", short(a)));
                }
            }
            i += 1;
        }
    } else {
        out = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(max)
            .map(String::from)
            .collect();
    }
    out.join("\n")
}

/// Subtitle bytes as text (UTF-8, else Latin-1 / Windows-1252-ish).
pub(crate) fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}
