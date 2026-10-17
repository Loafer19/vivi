//! Subtitle search, per-file offset and picks.
//!
//! **OpenSubtitles** (REST API): the playing file is searched by its OpenSubtitles
//! hash first (exact release matches), then by a query built from the file name, an
//! edited query or an IMDb id (`tt0133093`, also as an imdb.com URL). Results of both
//! are merged, hash matches first, with rating, downloads, hearing-impaired, FPS,
//! trusted-uploader and machine-translation badges. *Preview* downloads the subtitle
//! once into `$XDG_CACHE_HOME/vivi/subtitles` and shows its first lines; *Load* reuses
//! that copy (one download from the daily quota per subtitle). Optionally the file is
//! saved next to the video as `<video>.<lang>.<ext>` (setting *Save next to the video*).
//! The pick is remembered per file and loaded again the next time it plays.
//!
//! The API needs a (free) API key from <https://www.opensubtitles.com/consumers>,
//! entered in Settings → Extensions (or `VIVI_OPENSUBTITLES_KEY`). Username and
//! password are optional; logged-in users get more downloads per day.
//!
//! **Offset**: `z` / `x` move subtitles 0.1 s earlier / later (`Shift`: 1 s), the side
//! panel has the same buttons and a reset; the value is the mpv `sub-delay`, saved per
//! file in `$XDG_DATA_HOME/vivi/subtitle-offsets.json` and applied when the file opens.
//! Lyrics keep `[` / `]`. *Auto-sync* hands the subtitle to the Subtitle Auto-Sync
//! package (alass / ffsubsync with the shared media ffmpeg); Vivi itself ships no ffmpeg.

use super::{Ctx, Manifest, Plugin, SettingSpec};
use crate::{Item, SubHit};
use slint::ComponentHandle;
use slint::{ModelRc, VecModel};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use std::time::Duration;

pub const ID: &str = "subtitle-search";
const API: &str = "https://api.opensubtitles.com/api/v1";
pub const NO_KEY: &str = "OpenSubtitles needs an API key. Get a free one at opensubtitles.com/consumers and enter it in Settings → Extensions → Subtitle Search.";
/// Environment fallback for the API key (Settings wins).
const KEY_ENV: &str = "VIVI_OPENSUBTITLES_KEY";
/// Package that aligns subtitles to the audio (alass / ffsubsync + shared ffmpeg).
const SYNC_PACKAGE: &str = "subtitle-sync";

/// Language chips of the dialog: OpenSubtitles code and name.
pub const LANGS: &[(&str, &str)] = &[
    ("en", "English"),
    ("uk", "Ukrainian"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt-br", "Portuguese (BR)"),
    ("pl", "Polish"),
    ("nl", "Dutch"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("ar", "Arabic"),
    ("zh-cn", "Chinese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
];

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

fn feature_title(f: &serde_json::Value) -> String {
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

fn sort_hits(v: &mut [Hit]) {
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
    key: String,
    user: String,
    pass: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(format!("Vivi v{}", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .build()
        .into()
}

fn api_err(code: u16, body: &str) -> String {
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

/// What to search for: the file hash, an IMDb id or a text query (any combination).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub hash: Option<String>,
    pub imdb: Option<u64>,
    pub text: String,
}

/// Query parameters in the order the API wants (alphabetical, lowercase values).
fn search_params(q: &Query, langs: &str) -> Vec<(&'static str, String)> {
    let mut p = Vec::new();
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
    if q.imdb.is_none() && !q.text.trim().is_empty() {
        p.push(("query", q.text.trim().to_lowercase()));
    }
    p
}

fn search(c: &Creds, q: &Query, langs: &str) -> Result<Vec<Hit>, String> {
    let a = agent();
    let mut req = a
        .get(format!("{API}/subtitles"))
        .header("Api-Key", &c.key)
        .header("Accept", "application/json");
    // OpenSubtitles answers 301 unless parameters are lowercase and sorted by name.
    for (k, v) in search_params(q, langs) {
        req = req.query(k, v);
    }
    let mut r = req.call().map_err(|e| e.to_string())?;
    let body = r.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(api_err(r.status().as_u16(), &body));
    }
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
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
    let q = Query {
        hash: None,
        imdb: imdb_id(text),
        text: text.to_string(),
    };
    if q.imdb.is_none() && q.text.trim().is_empty() {
        return Ok(by_hash);
    }
    match search(c, &q, langs) {
        Ok(v) => Ok(merge_hits(by_hash, v)),
        Err(_) if !by_hash.is_empty() => Ok(merge_hits(by_hash, Vec::new())),
        Err(e) => Err(e),
    }
}

/// A subtitle already downloaded into the cache (`<file_id>-<name>`).
fn cached(dir: &Path, file_id: i64) -> Option<PathBuf> {
    let pre = format!("{file_id}-");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.starts_with(&pre) && e.path().is_file()).then(|| e.path())
    })
}

/// Download a subtitle into `dir` (reusing a cached copy). Returns the file and the
/// downloads left today when the API said so.
pub fn download(c: &Creds, file_id: i64, dir: &Path) -> Result<(PathBuf, Option<i64>), String> {
    if let Some(p) = cached(dir, file_id) {
        return Ok((p, None));
    }
    let a = agent();
    let post = |path: &str, token: Option<&str>, body: serde_json::Value| {
        let mut req = a
            .post(format!("{API}{path}"))
            .header("Api-Key", &c.key)
            .header("Accept", "application/json");
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        let mut r = req.send_json(body).map_err(|e| e.to_string())?;
        let s = r.body_mut().read_to_string().map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(api_err(r.status().as_u16(), &s));
        }
        serde_json::from_str::<serde_json::Value>(&s).map_err(|e| e.to_string())
    };
    let token = if c.user.is_empty() {
        None
    } else {
        let v = post(
            "/login",
            None,
            serde_json::json!({ "username": c.user, "password": c.pass }),
        )?;
        v["token"].as_str().map(String::from)
    };
    let v = post(
        "/download",
        token.as_deref(),
        serde_json::json!({ "file_id": file_id }),
    )?;
    let link = v["link"]
        .as_str()
        .ok_or_else(|| api_err(0, &v.to_string()))?;
    let remaining = v["remaining"].as_i64();
    let name = v["file_name"].as_str().unwrap_or("subtitle.srt");
    let mut name: String = name
        .chars()
        .map(|ch| if "/\\:".contains(ch) { '_' } else { ch })
        .collect();
    if !has_ext(Path::new(&name), SUB_EXT) {
        name.push_str(".srt");
    }
    let mut r = a.get(link).call().map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("download failed ({})", r.status()));
    }
    let bytes = r
        .body_mut()
        .with_config()
        .limit(20 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let out = dir.join(format!("{file_id}-{name}"));
    std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
    Ok((out, remaining))
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
    let lang = lang.trim().to_lowercase();
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
fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

// ---------- per-file offsets and picks ----------

fn offsets_file() -> PathBuf {
    crate::store::data_dir().join("subtitle-offsets.json")
}
fn picks_file() -> PathBuf {
    crate::store::data_dir().join("subtitle-picks.json")
}

/// Round to 0.05 s and clamp to ±10 min.
pub fn clamp_offset(o: f64) -> f64 {
    ((o * 20.0).round() / 20.0).clamp(-600.0, 600.0)
}

/// "+0.1 s", "−1.25 s", "0.0 s".
pub fn offset_label(o: f64) -> String {
    if o.abs() < 0.025 {
        return "0.0 s".into();
    }
    let a = o.abs();
    let n = if ((a * 10.0).round() - a * 10.0).abs() < 1e-6 {
        format!("{a:.1}")
    } else {
        format!("{a:.2}")
    };
    format!("{}{n} s", if o > 0.0 { "+" } else { "−" })
}

/// The subtitle chosen for a file (remembered and loaded again).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Pick {
    pub file_id: i64,
    pub path: String,
    #[serde(default)]
    pub language: String,
}

/// Side-panel line for a remembered pick.
pub fn pick_label(p: &Pick, video: Option<&Path>) -> String {
    let next = video
        .and_then(Path::parent)
        .is_some_and(|d| Path::new(&p.path).parent() == Some(d));
    let lang = if p.language.is_empty() {
        String::new()
    } else {
        format!("{} · ", lang_name(&p.language))
    };
    format!(
        "{lang}{}",
        if next {
            "saved next to the video"
        } else {
            "in the cache"
        }
    )
}

pub fn load_picks(file: &Path) -> HashMap<String, Pick> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_pick(file: &Path, key: &str, pick: Option<Pick>) {
    let mut m = load_picks(file);
    match pick {
        Some(p) => m.insert(key.to_string(), p),
        None => m.remove(key),
    };
    if let Ok(s) = serde_json::to_string_pretty(&m) {
        let _ = crate::store::write_atomic(file, &s);
    }
}

const SUB_EXT: &[&str] = &["srt", "ass", "ssa", "vtt", "sub"];
const VIDEO_EXT: &[&str] = &[
    "mp4", "mkv", "webm", "avi", "mov", "m4v", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
];

fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Files of `dir` with one of `exts`, sorted by name.
fn files_with(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.retain(|p| p.is_file() && has_ext(p, exts));
    v.sort_by_key(|p| file_name(p).to_lowercase());
    v
}

/// Whether subtitle `sub` belongs to `video`: same stem, or the stem followed by a
/// separator (`Movie.en.srt`, `Movie_English.srt`), case-insensitive.
pub fn sub_matches(video: &Path, sub: &Path) -> bool {
    let v = video
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let s = sub
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    !v.is_empty()
        && (s == v
            || s.strip_prefix(&v)
                .is_some_and(|r| r.starts_with(['.', '_', '-', ' ', '['])))
}

/// Pair each video of a folder with its best subtitle (exact stem first).
pub fn pair_folder(videos: &[PathBuf], subs: &[PathBuf]) -> Vec<(PathBuf, Option<PathBuf>)> {
    videos
        .iter()
        .map(|v| {
            let stem = v.file_stem().map(|s| s.to_string_lossy().to_lowercase());
            let exact = subs
                .iter()
                .find(|s| s.file_stem().map(|x| x.to_string_lossy().to_lowercase()) == stem);
            let sub = exact.or_else(|| subs.iter().find(|s| sub_matches(v, s)));
            (v.clone(), sub.cloned())
        })
        .collect()
}

/// "Videos / Shows / The Wand / S01": the last four components of a folder.
fn breadcrumb(dir: &Path) -> String {
    let parts: Vec<String> = dir
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let n = parts.len().saturating_sub(4);
    parts[n..].join(" / ")
}

enum Msg {
    /// Hash matches, sent before the name search finishes.
    Partial(Vec<Hit>),
    Results(Result<Vec<Hit>, String>),
    Got {
        file_id: i64,
        preview: bool,
        r: Result<(PathBuf, Option<i64>), String>,
    },
}

#[derive(Default)]
pub struct SubtitleSearch {
    hits: Vec<Hit>,
    selected: Option<usize>,
    rx: Option<Receiver<Msg>>,
    /// Subtitle files fetched for Preview (file id → cached file), reused by Load.
    fetched: HashMap<i64, PathBuf>,
    /// The playing file's key (path) and its remembered pick.
    key: Option<String>,
    pick: Option<Pick>,
    /// Offset this plugin applied to the current file (to undo it for the next one).
    applied: Option<f64>,
    /// "From Computer": scanned folder, its subtitle files (To This File) with the
    /// checked ones, or its videos paired with subtitles (To This Folder).
    dir: Option<PathBuf>,
    subs: Vec<PathBuf>,
    checked: Vec<bool>,
    pairs: Vec<(PathBuf, Option<PathBuf>)>,
    /// Subtitles assigned with "To This Folder", attached when that video loads.
    assigned: HashMap<PathBuf, PathBuf>,
}

fn lang_name(code: &str) -> String {
    LANGS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| code.to_uppercase())
}

/// Details shown beside the results for the selected subtitle.
pub fn details(h: &Hit) -> String {
    let mut l = Vec::new();
    if !h.title.is_empty() {
        l.push(h.title.clone());
    }
    if !h.release.is_empty() && h.release != h.title {
        l.push(format!("Release: {}", h.release));
    }
    l.push(format!("File: {}", h.file_name));
    l.push(format!(
        "Language: {}  ·  {} downloads",
        lang_name(&h.language),
        h.downloads
    ));
    if h.rating > 0.0 {
        l.push(format!("Rating: {:.1} / 10 ({} votes)", h.rating, h.votes));
    }
    if h.fps > 0.0 {
        l.push(format!("Frame rate: {:.3} fps", h.fps));
    }
    let mut flags = Vec::new();
    if h.hash_match {
        flags.push("matches this exact file (hash)");
    }
    if h.hearing_impaired {
        flags.push("hearing impaired (sound descriptions)");
    }
    if h.trusted {
        flags.push("trusted uploader");
    }
    if h.machine {
        flags.push("machine translated");
    }
    if !flags.is_empty() {
        l.push(format!("Notes: {}", flags.join(", ")));
    }
    match (h.uploader.is_empty(), h.uploaded.is_empty()) {
        (false, false) => l.push(format!("Uploaded by {} on {}", h.uploader, h.uploaded)),
        (false, true) => l.push(format!("Uploaded by {}", h.uploader)),
        (true, false) => l.push(format!("Uploaded on {}", h.uploaded)),
        _ => {}
    }
    l.join("\n")
}

fn compact(n: i64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 10_000 => format!("{}k", n / 1000),
        n if n >= 1000 => format!("{:.1}k", n as f64 / 1000.0),
        n => n.to_string(),
    }
}

impl SubtitleSearch {
    /// Rescan the "From Computer" folder and show it in the dialog.
    fn scan_local(&mut self, cx: &Ctx) {
        let folder = cx.app.get_subs_target() == 1;
        let Some(dir) = self.dir.clone() else {
            cx.app.set_subs_folder("Choose a folder…".into());
            cx.app.set_subs_local(ModelRc::default());
            Self::status(
                cx,
                "Open a local file or click the folder bar to choose a folder.",
                false,
            );
            return;
        };
        cx.app.set_subs_folder(breadcrumb(&dir).into());
        self.subs = files_with(&dir, SUB_EXT);
        let items: Vec<Item> = if folder {
            self.pairs = pair_folder(&files_with(&dir, VIDEO_EXT), &self.subs);
            self.pairs
                .iter()
                .map(|(v, s)| Item {
                    key: file_name(v).into(),
                    label: s.as_deref().map(file_name).unwrap_or_default().into(),
                    selected: s.is_some(),
                })
                .collect()
        } else {
            let video = cx.local_path();
            // Preselect the subtitle mpv would pick: the exact name, else the first match.
            let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_lowercase());
            let pick = video.as_deref().and_then(|v| {
                self.subs
                    .iter()
                    .position(|s| stem(s) == stem(v))
                    .or_else(|| self.subs.iter().position(|s| sub_matches(v, s)))
            });
            self.checked = (0..self.subs.len()).map(|i| Some(i) == pick).collect();
            self.local_items()
        };
        let empty = items.is_empty();
        cx.app.set_subs_local(ModelRc::new(VecModel::from(items)));
        Self::status(
            cx,
            match (empty, folder) {
                (true, false) => "No subtitle files in this folder.",
                (true, true) => "No videos in this folder.",
                _ => "",
            },
            false,
        );
    }
    fn local_items(&self) -> Vec<Item> {
        self.subs
            .iter()
            .zip(&self.checked)
            .map(|(s, c)| Item {
                key: file_name(s).into(),
                label: "".into(),
                selected: *c,
            })
            .collect()
    }
    fn save_local(&mut self, cx: &Ctx) {
        let current = cx.local_path().and_then(|p| std::fs::canonicalize(p).ok());
        let mut loaded = 0;
        if cx.app.get_subs_target() == 0 {
            for (s, _) in self.subs.iter().zip(&self.checked).filter(|(_, c)| **c) {
                let flag = if loaded == 0 { "select" } else { "auto" };
                cx.player.cmd("sub-add", &[&s.to_string_lossy(), flag]);
                loaded += 1;
            }
            if loaded == 0 {
                Self::status(cx, "Select at least one subtitle file", false);
                return;
            }
            cx.toast(&format!(
                "{loaded} subtitle file{} added",
                if loaded == 1 { "" } else { "s" }
            ));
        } else {
            let mut n = 0;
            for (v, s) in &self.pairs {
                let (Some(s), Ok(v)) = (s, std::fs::canonicalize(v)) else {
                    continue;
                };
                if current.as_ref() == Some(&v) {
                    cx.player.cmd("sub-add", &[&s.to_string_lossy(), "select"]);
                }
                self.assigned.insert(v, s.clone());
                n += 1;
            }
            cx.toast(&format!(
                "Subtitles assigned to {n} video{}",
                if n == 1 { "" } else { "s" }
            ));
        }
        Self::status(cx, "", false);
        cx.app.global::<crate::Overlays>().set_subs_open(false);
    }
    fn creds(cx: &Ctx) -> Creds {
        let mut key = cx.get("api-key").trim().to_string();
        if key.is_empty() {
            key = std::env::var(KEY_ENV)
                .unwrap_or_default()
                .trim()
                .to_string();
        }
        Creds {
            key,
            user: cx.get("username").trim().to_string(),
            pass: cx.get("password"),
        }
    }
    fn status(cx: &Ctx, s: &str, busy: bool) {
        cx.app.set_subs_status(s.into());
        cx.app.set_subs_busy(busy);
    }
    fn set_hits(&mut self, cx: &Ctx, hits: Vec<Hit>) {
        let pick = self.pick.as_ref().map(|p| p.file_id);
        let rows: Vec<SubHit> = hits
            .iter()
            .map(|h| SubHit {
                name: h.file_name.as_str().into(),
                title: h.title.as_str().into(),
                lang: h.language.to_uppercase().into(),
                downloads: compact(h.downloads).into(),
                rating: if h.rating > 0.0 {
                    format!("{:.1}", h.rating).into()
                } else {
                    "".into()
                },
                hi: h.hearing_impaired,
                fps: if h.fps > 0.0 {
                    format!("{:.3}", h.fps)
                        .trim_end_matches('0')
                        .trim_end_matches('.')
                        .into()
                } else {
                    "".into()
                },
                hash: h.hash_match,
                trusted: h.trusted,
                ai: h.machine,
                pick: Some(h.file_id) == pick,
            })
            .collect();
        cx.app.set_subs_hits(ModelRc::new(VecModel::from(rows)));
        self.hits = hits;
        self.select(cx, None);
    }
    fn select(&mut self, cx: &Ctx, i: Option<usize>) {
        self.selected = i.filter(|i| *i < self.hits.len());
        cx.app
            .set_subs_selected(self.selected.map(|i| i as i32).unwrap_or(-1));
        let h = self.selected.and_then(|i| self.hits.get(i));
        cx.app
            .set_subs_details(h.map(details).unwrap_or_default().into());
        // Downloaded for Preview / Load before (also in an earlier session).
        let pre = h
            .and_then(|h| {
                self.fetched
                    .get(&h.file_id)
                    .cloned()
                    .or_else(|| cached(&crate::store::cache_dir().join("subtitles"), h.file_id))
            })
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| preview_text(&decode(&b), 60))
            .unwrap_or_default();
        cx.app.set_subs_preview(pre.into());
    }
    /// Language chips: the common ones plus any other code in the list.
    fn show_langs(cx: &Ctx, langs: &str) {
        let on: Vec<String> = norm_langs(langs)
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        let mut items: Vec<Item> = LANGS
            .iter()
            .map(|(c, n)| Item {
                key: (*c).into(),
                label: (*n).into(),
                selected: on.iter().any(|x| x == c),
            })
            .collect();
        for c in on.iter().filter(|c| !LANGS.iter().any(|(x, _)| x == c)) {
            items.push(Item {
                key: c.as_str().into(),
                label: c.to_uppercase().into(),
                selected: true,
            });
        }
        cx.app.set_subs_langs(ModelRc::new(VecModel::from(items)));
        cx.app.set_subs_lang(norm_langs(langs).into());
    }
    fn set_langs(&mut self, cx: &Ctx, langs: &str) {
        let l = norm_langs(langs);
        cx.set("languages", &l);
        Self::show_langs(cx, &l);
    }
    /// Key of the playing file for offsets and picks (canonical path or URL).
    fn file_key(cx: &Ctx) -> Option<String> {
        if let Some(p) = cx.local_path() {
            let p = std::fs::canonicalize(&p).unwrap_or(p);
            return Some(p.to_string_lossy().into_owned());
        }
        cx.player.get("path").filter(|p| !p.is_empty())
    }
    /// "English · saved next to the video" (the pick loads again next time).
    fn show_pick(cx: &Ctx, pick: Option<&Pick>, video: Option<&Path>) {
        cx.app.set_subs_pick_label(
            pick.map(|p| pick_label(p, video))
                .unwrap_or_default()
                .into(),
        );
    }
    fn show_offset(cx: &Ctx, o: f64) {
        cx.app.set_subs_offset(offset_label(o).into());
        cx.app.set_pv_sub_delay(o as f32);
    }
    /// Set (`save`: and remember) the offset of the playing file.
    fn set_offset(&mut self, cx: &Ctx, o: f64, toast: bool) {
        let o = clamp_offset(o);
        cx.player.set("sub-delay", &format!("{o}"));
        if let Some(k) = &self.key {
            crate::plugins::lyrics::save_offset(&offsets_file(), k, o);
        }
        self.applied = Some(o);
        Self::show_offset(cx, o);
        if toast {
            let what = if o == 0.0 {
                "Subtitle offset reset".to_string()
            } else {
                format!(
                    "Subtitle offset {}{}",
                    offset_label(o),
                    if self.key.is_some() {
                        " · saved for this file"
                    } else {
                        ""
                    }
                )
            };
            cx.toast(&what);
        }
    }
    fn current_offset(cx: &Ctx) -> f64 {
        cx.player
            .get("sub-delay")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    }
    fn sync_available(cx: &Ctx) -> Option<String> {
        cx.package_script(SYNC_PACKAGE)
    }
    /// External subtitle files mpv already has for this item.
    fn loaded_subs(cx: &Ctx) -> Vec<PathBuf> {
        let n: usize = cx
            .player
            .get("track-list/count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        (0..n)
            .filter_map(|i| cx.player.get(&format!("track-list/{i}/external-filename")))
            .filter(|f| !f.is_empty())
            .map(|f| {
                let p = PathBuf::from(f);
                std::fs::canonicalize(&p).unwrap_or(p)
            })
            .collect()
    }
    /// Put a downloaded subtitle in place (next to the video when that setting is on),
    /// load it, remember it for this file and close the dialog.
    fn use_file(&mut self, cx: &Ctx, cachedp: &Path, file_id: i64, lang: &str) {
        let mut path = cachedp.to_path_buf();
        let mut note = String::new();
        if cx.flag("save-next") {
            if let Some(video) = cx.local_path() {
                let ext = cachedp
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_else(|| "srt".into());
                let data = std::fs::read(cachedp).unwrap_or_default();
                let dest = sidecar_path(&video, lang, &ext, file_id, |p| {
                    p.exists() && std::fs::read(p).ok().as_deref() != Some(&data[..])
                });
                match std::fs::write(&dest, &data) {
                    Ok(()) => {
                        note = format!(" · saved as {}", file_name(&dest));
                        path = dest;
                    }
                    Err(e) => note = format!(" · could not save next to the video ({e})"),
                }
            }
        }
        cx.player
            .cmd("sub-add", &[&path.to_string_lossy(), "select"]);
        if let Some(k) = &self.key {
            let p = Pick {
                file_id,
                path: path.to_string_lossy().into_owned(),
                language: lang.to_string(),
            };
            save_pick(&picks_file(), k, Some(p.clone()));
            self.pick = Some(p);
            cx.app.set_subs_has_pick(true);
            Self::show_pick(cx, self.pick.as_ref(), cx.local_path().as_deref());
        }
        Self::status(cx, "", false);
        cx.app.global::<crate::Overlays>().set_subs_open(false);
        cx.toast(&format!("Subtitles loaded{note}"));
    }
    fn fetch(&mut self, cx: &Ctx, i: Option<usize>, preview: bool) {
        let Some(h) = i.and_then(|i| self.hits.get(i)).cloned() else {
            Self::status(cx, "Select a subtitle first", false);
            return;
        };
        if let Some(p) = self
            .fetched
            .get(&h.file_id)
            .filter(|p| p.is_file())
            .cloned()
        {
            if preview {
                self.select(cx, i);
            } else {
                self.use_file(cx, &p, h.file_id, &h.language);
            }
            return;
        }
        let c = Self::creds(cx);
        if c.key.is_empty() {
            Self::status(cx, NO_KEY, false);
            return;
        }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        Self::status(
            cx,
            if preview {
                "Fetching preview…"
            } else {
                "Downloading…"
            },
            true,
        );
        let dir = crate::store::cache_dir().join("subtitles");
        std::thread::spawn(move || {
            let r = download(&c, h.file_id, &dir);
            let _ = tx.send(Msg::Got {
                file_id: h.file_id,
                preview,
                r,
            });
        });
    }
    fn start_search(&mut self, cx: &Ctx) {
        let c = Self::creds(cx);
        if c.key.is_empty() {
            Self::status(cx, NO_KEY, false);
            return;
        }
        let query = cx.app.get_subs_query().to_string();
        let langs = norm_langs(&cx.app.get_subs_lang());
        let local = cx.local_path();
        let (tx, rx) = channel::<Msg>();
        self.rx = Some(rx);
        self.set_hits(cx, Vec::new());
        Self::status(
            cx,
            if local.is_some() {
                "Searching OpenSubtitles by file hash…"
            } else {
                "Searching OpenSubtitles…"
            },
            true,
        );
        std::thread::spawn(move || {
            let t: Sender<Msg> = tx.clone();
            let r = search_all(&c, local.as_deref(), &query, &langs, |h| {
                let _ = t.send(Msg::Partial(h.to_vec()));
            });
            let _ = tx.send(Msg::Results(r));
        });
    }
    /// Context for the control socket's subtitle tools (they run off the UI thread).
    /// Internal only: the socket refuses `__` commands from clients.
    fn mcp_context(cx: &Ctx) -> serde_json::Value {
        let c = Self::creds(cx);
        serde_json::json!({
            "key": c.key, "user": c.user, "pass": c.pass,
            "path": cx.local_path().map(|p| p.to_string_lossy().into_owned()),
            "languages": cx.get("languages"),
            "query": query_from_name(&cx.player.get("path").unwrap_or_default()),
        })
    }
}

fn ctx_creds(ctx: &serde_json::Value) -> Result<Creds, String> {
    let s = |k: &str| ctx[k].as_str().unwrap_or("").to_string();
    let c = Creds {
        key: s("key"),
        user: s("user"),
        pass: s("pass"),
    };
    if c.key.is_empty() {
        return Err(NO_KEY.into());
    }
    Ok(c)
}

/// Control socket `search_subtitles`: `query` (default: from the file name, IMDb ids
/// accepted), `languages` (default: the plugin setting), `limit`.
pub fn mcp_search(
    ctx: &serde_json::Value,
    args: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let c = ctx_creds(ctx)?;
    let path = ctx["path"].as_str().map(PathBuf::from);
    let query = args["query"]
        .as_str()
        .filter(|q| !q.trim().is_empty())
        .or(ctx["query"].as_str())
        .unwrap_or("")
        .to_string();
    let langs = args["languages"]
        .as_str()
        .or(ctx["languages"].as_str())
        .unwrap_or("")
        .to_string();
    let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
    let hits = search_all(&c, path.as_deref(), &query, &langs, |_| {})?;
    Ok(serde_json::json!({
        "query": query, "languages": norm_langs(&langs), "hashed": path.is_some(),
        "count": hits.len(),
        "items": hits.iter().take(limit).map(Hit::to_json).collect::<Vec<_>>(),
    }))
}

/// Control socket `load_subtitle`: download `file_id` (or reuse the cached copy).
pub fn mcp_download(
    ctx: &serde_json::Value,
    args: &serde_json::Value,
) -> Result<(PathBuf, i64, Option<i64>), String> {
    let c = ctx_creds(ctx)?;
    let id = args["file_id"]
        .as_i64()
        .ok_or("missing \"file_id\" (see search_subtitles)")?;
    let (p, left) = download(&c, id, &crate::store::cache_dir().join("subtitles"))?;
    Ok((p, id, left))
}

impl Plugin for SubtitleSearch {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Subtitle Search",
            "Find subtitles on OpenSubtitles (by file hash, name or IMDb id) in the Add Subtitles dialog, and keep a subtitle offset per file (z / x).",
            "subtitles",
        )
        .setting(SettingSpec::secret(
            "api-key",
            "OpenSubtitles API key",
            "Required. Create a free consumer key at opensubtitles.com/consumers.",
        ))
        .setting(SettingSpec::text(
            "languages",
            "Languages",
            "Comma-separated codes, e.g. en,uk (also picked in the dialog).",
            "en",
            "en",
        ))
        .setting(SettingSpec::text(
            "username",
            "Username (optional)",
            "Logged-in users get more downloads per day.",
            "",
            "",
        ))
        .setting(SettingSpec::secret("password", "Password (optional)", ""))
        .setting(SettingSpec::toggle(
            "save-next",
            "Save next to the video",
            "Keep downloaded subtitles as <video>.<lang>.srt beside the file (else in the cache).",
            false,
        ))
    }
    fn enable(&mut self, cx: &Ctx) {
        cx.app.set_subs_plugin(true);
        // Tells the Subtitle Auto-Sync package that Vivi keeps the per-file offsets.
        cx.player.cmd(
            "change-list",
            &["script-opts", "append", "vivi-sub-offsets=1"],
        );
    }
    fn disable(&mut self, cx: &Ctx) {
        cx.app.set_subs_plugin(false);
        cx.app.global::<crate::Overlays>().set_subs_open(false);
        cx.player.cmd(
            "change-list",
            &["script-opts", "append", "vivi-sub-offsets=0"],
        );
        self.rx = None;
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let arg = args.first().map(String::as_str).unwrap_or("");
        let idx = arg.parse::<usize>().ok();
        match name {
            "open" => {
                let path = cx.player.get("path").unwrap_or_default();
                cx.app.set_subs_query(query_from_name(&path).into());
                Self::show_langs(cx, &cx.get("languages"));
                cx.app.set_subs_save_next(cx.flag("save-next"));
                cx.app.set_subs_sync(Self::sync_available(cx).is_some());
                self.set_hits(cx, Vec::new());
                self.dir = cx
                    .local_path()
                    .and_then(|p| p.parent().map(Path::to_path_buf));
                // Without an API key only the local source works.
                if Self::creds(cx).key.is_empty() && std::env::var_os("VIVI_SUBS").is_none() {
                    cx.app.set_subs_source(1);
                }
                if cx.app.get_subs_source() == 1 {
                    self.scan_local(cx);
                } else if Self::creds(cx).key.is_empty() {
                    Self::status(cx, NO_KEY, false);
                } else {
                    Self::status(cx, "", false);
                    self.start_search(cx);
                }
            }
            "search" => self.start_search(cx),
            "lang" => {
                let l = toggle_lang(&cx.app.get_subs_lang(), arg);
                self.set_langs(cx, &l);
                self.start_search(cx);
            }
            "lang-text" => {
                self.set_langs(cx, arg);
                self.start_search(cx);
            }
            "save-next" => {
                let on = !cx.flag("save-next");
                cx.set("save-next", if on { "true" } else { "false" });
                cx.app.set_subs_save_next(on);
            }
            "mode" => {
                if cx.app.get_subs_source() == 1 {
                    self.scan_local(cx);
                } else {
                    let key = Self::creds(cx).key;
                    Self::status(cx, if key.is_empty() { NO_KEY } else { "" }, false);
                }
            }
            "browse" => {
                let mut d = rfd::FileDialog::new().set_title("Folder with subtitles");
                if let Some(dir) = &self.dir {
                    d = d.set_directory(dir);
                }
                if let Some(dir) = d.pick_folder() {
                    self.dir = Some(dir);
                    self.scan_local(cx);
                }
            }
            "toggle" => {
                let Some(i) = idx else { return };
                if cx.app.get_subs_target() == 0 {
                    if let Some(c) = self.checked.get_mut(i) {
                        *c = !*c;
                    }
                    cx.app
                        .set_subs_local(ModelRc::new(VecModel::from(self.local_items())));
                }
            }
            "save-local" => self.save_local(cx),
            "select" => self.select(cx, idx),
            "preview" => self.fetch(cx, idx, true),
            "download" => self.fetch(cx, idx, false),
            // MCP / control socket: a subtitle downloaded off the UI thread.
            "load-file" => {
                let (Some(p), Some(id)) = (args.first(), args.get(1).and_then(|s| s.parse().ok()))
                else {
                    return;
                };
                let lang = args.get(2).cloned().unwrap_or_default();
                self.use_file(cx, Path::new(p), id, &lang);
            }
            "forget" => {
                if let Some(k) = &self.key {
                    save_pick(&picks_file(), k, None);
                }
                self.pick = None;
                cx.app.set_subs_has_pick(false);
                cx.toast("Subtitle pick forgotten for this file");
            }
            // z / x, side panel buttons, `vivi-sub-offset` from the Auto-Sync package.
            "offset" => {
                let o = match arg {
                    "reset" => 0.0,
                    a if a.starts_with("set:") => a[4..].parse().unwrap_or(0.0),
                    a => Self::current_offset(cx) + a.parse::<f64>().unwrap_or(0.0),
                };
                self.set_offset(cx, o, true);
            }
            // The side panel's delay slider (already applied to mpv).
            "offset-changed" => {
                let o = arg.parse().unwrap_or(0.0);
                self.set_offset(cx, o, false);
            }
            "sync" => match Self::sync_available(cx) {
                Some(script) => cx.player.cmd("script-message-to", &[&script, "sync"]),
                None => cx.toast(
                    "Install Subtitle Auto-Sync (Settings → Extensions → Browse) to align subtitles to the audio",
                ),
            },
            _ => {}
        }
    }
    fn query(&mut self, cx: &Ctx, name: &str) -> Option<Box<dyn std::any::Any>> {
        match name {
            "mcp-context" => Some(Box::new(Self::mcp_context(cx))),
            "offset" => Some(Box::new(Self::current_offset(cx))),
            _ => None,
        }
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.key = Self::file_key(cx);
        self.fetched.clear();
        // Offset saved for this file; otherwise undo the previous file's one.
        let saved = self
            .key
            .as_ref()
            .and_then(|k| crate::plugins::lyrics::load_offsets(&offsets_file()).remove(k));
        match saved {
            Some(o) => {
                cx.player.set("sub-delay", &format!("{o}"));
                self.applied = Some(o);
                Self::show_offset(cx, o);
            }
            None => {
                let cur = Self::current_offset(cx);
                if self.applied.is_some_and(|a| (a - cur).abs() < 1e-6) {
                    cx.player.set("sub-delay", "0");
                    Self::show_offset(cx, 0.0);
                } else {
                    Self::show_offset(cx, cur);
                }
                self.applied = None;
            }
        }
        cx.app.set_subs_sync(Self::sync_available(cx).is_some());
        // The remembered pick, unless mpv already loaded that file (sub-auto).
        self.pick = self
            .key
            .as_ref()
            .and_then(|k| load_picks(&picks_file()).remove(k))
            .filter(|p| Path::new(&p.path).is_file());
        cx.app.set_subs_has_pick(self.pick.is_some());
        Self::show_pick(cx, self.pick.as_ref(), cx.local_path().as_deref());
        let loaded = Self::loaded_subs(cx);
        let mut want: Vec<PathBuf> = Vec::new();
        if let Some(p) = &self.pick {
            want.push(PathBuf::from(&p.path));
        }
        if let Some(v) = cx.local_path().and_then(|p| std::fs::canonicalize(p).ok()) {
            if let Some(s) = self.assigned.get(&v) {
                want.push(s.clone());
            }
        }
        if let Some(s) = want.first() {
            let c = std::fs::canonicalize(s).unwrap_or(s.clone());
            if !loaded.contains(&c) {
                cx.player.cmd("sub-add", &[&s.to_string_lossy(), "select"]);
            }
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        loop {
            let Some(rx) = &self.rx else { return };
            let Ok(m) = rx.try_recv() else { return };
            match m {
                Msg::Partial(h) => {
                    let n = h.len();
                    self.set_hits(cx, h);
                    Self::status(
                        cx,
                        &format!(
                            "{n} exact match{} for this file · searching by name…",
                            if n == 1 { "" } else { "es" }
                        ),
                        true,
                    );
                }
                Msg::Results(r) => {
                    self.rx = None;
                    match r {
                        Ok(h) => {
                            let exact = h.iter().filter(|h| h.hash_match).count();
                            let s = if h.is_empty() {
                                "No subtitles found. Try another query, an IMDb id (tt…) or more languages.".to_string()
                            } else if exact > 0 {
                                format!("{} subtitles · {exact} match this exact file", h.len())
                            } else {
                                format!("{} subtitles found", h.len())
                            };
                            // Keep the pick selected when it is in the list.
                            let pick = self.pick.as_ref().map(|p| p.file_id);
                            let sel = h.iter().position(|x| Some(x.file_id) == pick);
                            self.set_hits(cx, h);
                            self.select(cx, sel);
                            Self::status(cx, &s, false);
                        }
                        Err(e) => Self::status(cx, &e, false),
                    }
                }
                Msg::Got {
                    file_id,
                    preview,
                    r,
                } => {
                    self.rx = None;
                    match r {
                        Ok((p, left)) => {
                            self.fetched.insert(file_id, p.clone());
                            let left = left
                                .map(|n| format!(" · {n} downloads left today"))
                                .unwrap_or_default();
                            if preview {
                                let i = self.hits.iter().position(|h| h.file_id == file_id);
                                self.select(cx, i);
                                Self::status(cx, &format!("Preview ready{left}"), false);
                            } else {
                                let lang = self
                                    .hits
                                    .iter()
                                    .find(|h| h.file_id == file_id)
                                    .map(|h| h.language.clone())
                                    .unwrap_or_default();
                                self.use_file(cx, &p, file_id, &lang);
                            }
                        }
                        Err(e) => Self::status(cx, &e, false),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_pairing() {
        let p = |s: &str| PathBuf::from(s);
        assert!(sub_matches(&p("/a/Ep01.mp4"), &p("/a/Ep01.srt")));
        assert!(sub_matches(&p("/a/Ep01.mp4"), &p("/a/Ep01_English.srt")));
        assert!(sub_matches(&p("/a/ep01.mp4"), &p("/a/EP01.en.srt")));
        assert!(!sub_matches(&p("/a/Ep01.mp4"), &p("/a/Ep010.srt")));
        let pairs = pair_folder(
            &[p("/a/Ep01.mp4"), p("/a/Ep02.mp4")],
            &[p("/a/Ep01_English.srt"), p("/a/Ep01.srt")],
        );
        assert_eq!(pairs[0].1, Some(p("/a/Ep01.srt")));
        assert_eq!(pairs[1].1, None);
        assert_eq!(
            breadcrumb(&p("/home/u/Videos/Shows/The Wand/S01")),
            "Videos / Shows / The Wand / S01"
        );
    }

    #[test]
    fn hash_known_value() {
        // 128 KiB of zeros plus one 0x01 byte at the start: size + 1.
        let p = std::env::temp_dir().join(format!("vivi-hash-{}", std::process::id()));
        let mut data = vec![0u8; 131072 + 10];
        data[0] = 1;
        std::fs::write(&p, &data).unwrap();
        assert_eq!(
            moviehash(&p).unwrap(),
            format!("{:016x}", data.len() as u64 + 1)
        );
        std::fs::write(&p, b"tiny").unwrap();
        assert!(moviehash(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn params_sorted() {
        let q = Query {
            hash: Some("ABC".into()),
            imdb: None,
            text: " Sintel ".into(),
        };
        let p = search_params(&q, "uk, EN");
        let keys: Vec<&str> = p.iter().map(|(k, _)| *k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(p[0].1, "en,uk");
        assert_eq!(p[1].1, "abc");
        assert_eq!(p[2], ("query", "sintel".to_string()));
        // An IMDb id replaces the text query and sorts first.
        let q = Query {
            hash: None,
            imdb: Some(133093),
            text: "tt0133093".into(),
        };
        let p = search_params(&q, "");
        assert_eq!(p, vec![("imdb_id", "133093".to_string())]);
    }

    #[test]
    fn imdb_ids() {
        assert_eq!(imdb_id("tt0133093"), Some(133093));
        assert_eq!(imdb_id(" TT0133093 "), Some(133093));
        assert_eq!(
            imdb_id("https://www.imdb.com/title/tt0133093/?ref_=x"),
            Some(133093)
        );
        assert_eq!(imdb_id("1917"), None);
        assert_eq!(imdb_id("ttfoo"), None);
        assert_eq!(imdb_id("tt123abc"), None);
        assert_eq!(imdb_id("The Matrix"), None);
    }

    #[test]
    fn languages() {
        assert_eq!(norm_langs(" uk, EN;en  pt-BR"), "en,pt-br,uk");
        assert_eq!(norm_langs(""), "");
        assert_eq!(toggle_lang("en,uk", "uk"), "en");
        assert_eq!(toggle_lang("en", "UK"), "en,uk");
        assert_eq!(toggle_lang("", "fr"), "fr");
        assert_eq!(toggle_lang("fr", "fr"), "");
    }

    #[test]
    fn merge_hash_first() {
        let h = |id, dl, hash| Hit {
            file_id: id,
            downloads: dl,
            hash_match: hash,
            ..Default::default()
        };
        let m = merge_hits(vec![h(1, 5, false)], vec![h(2, 900, false), h(1, 5, false)]);
        assert_eq!(m.iter().map(|x| x.file_id).collect::<Vec<_>>(), [1, 2]);
        assert!(m[0].hash_match && !m[1].hash_match);
        // Machine translations sink below human ones.
        let mut ai = h(3, 1000, false);
        ai.machine = true;
        let m = merge_hits(vec![], vec![ai, h(4, 10, false)]);
        assert_eq!(m[0].file_id, 4);
    }

    #[test]
    fn sidecar_names() {
        let v = Path::new("/m/Movie.2020.mkv");
        assert_eq!(
            sidecar_path(v, "EN", "srt", 7, |_| false),
            PathBuf::from("/m/Movie.2020.en.srt")
        );
        assert_eq!(
            sidecar_path(v, "en", "srt", 7, |_| true),
            PathBuf::from("/m/Movie.2020.en.7.srt")
        );
        assert_eq!(
            sidecar_path(v, "", "ass", 7, |_| false),
            PathBuf::from("/m/Movie.2020.ass")
        );
    }

    #[test]
    fn previews() {
        let srt = "\u{feff}1\n00:00:01,000 --> 00:00:02,000\n<i>Hello</i>\nthere\n\n2\n01:02:03,500 --> 01:02:04,000\nBye\n";
        assert_eq!(preview_text(srt, 10), "0:00:01  Hello there\n1:02:03  Bye");
        assert_eq!(preview_text(srt, 1), "0:00:01  Hello there");
        let vtt = "WEBVTT\n\n00:05.000 --> 00:06.000\nHi\n";
        assert_eq!(preview_text(vtt, 5), "0:05  Hi");
        let ass = "[Script Info]\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:03.00,0:00:04.00,Default,,0,0,0,,{\\i1}One\\Ntwo, three\n";
        assert_eq!(preview_text(ass, 5), "0:00:03  One two, three");
        assert_eq!(decode(&[0x48, 0xe9]), "Hé");
    }

    #[test]
    fn offsets_and_labels() {
        assert_eq!(clamp_offset(0.123), 0.1);
        assert_eq!(clamp_offset(0.13), 0.15);
        assert_eq!(clamp_offset(-9999.0), -600.0);
        assert_eq!(offset_label(0.0), "0.0 s");
        assert_eq!(offset_label(0.1), "+0.1 s");
        assert_eq!(offset_label(-1.25), "−1.25 s");
        assert_eq!(offset_label(2.0), "+2.0 s");
    }

    #[test]
    fn picks_roundtrip() {
        let f = std::env::temp_dir().join(format!("vivi-picks-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&f);
        let p = Pick {
            file_id: 9,
            path: "/m/a.en.srt".into(),
            language: "en".into(),
        };
        save_pick(&f, "/m/a.mkv", Some(p.clone()));
        assert_eq!(load_picks(&f).get("/m/a.mkv"), Some(&p));
        save_pick(&f, "/m/a.mkv", None);
        assert!(load_picks(&f).is_empty());
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn pick_labels() {
        let p = Pick {
            file_id: 1,
            path: "/m/a.en.srt".into(),
            language: "en".into(),
        };
        assert_eq!(
            pick_label(&p, Some(Path::new("/m/a.mkv"))),
            "English · saved next to the video"
        );
        assert!(pick_label(&p, Some(Path::new("/x/a.mkv"))).contains("in the cache"));
    }

    #[test]
    fn cache_reuse() {
        let d = std::env::temp_dir().join(format!("vivi-subcache-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("42-Movie.srt"), "x").unwrap();
        assert_eq!(cached(&d, 42), Some(d.join("42-Movie.srt")));
        assert_eq!(cached(&d, 4), None);
        // A cached file is returned without any network request.
        let c = Creds {
            key: "unused".into(),
            user: String::new(),
            pass: String::new(),
        };
        assert_eq!(
            download(&c, 42, &d).unwrap(),
            (d.join("42-Movie.srt"), None)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn details_text() {
        let h = Hit {
            file_id: 1,
            file_name: "Movie.srt".into(),
            title: "Movie (2020)".into(),
            language: "en".into(),
            downloads: 1200,
            rating: 8.5,
            votes: 3,
            fps: 23.976,
            hash_match: true,
            hearing_impaired: true,
            ..Default::default()
        };
        let d = details(&h);
        assert!(d.starts_with("Movie (2020)\nFile: Movie.srt\nLanguage: English"));
        assert!(d.contains("Rating: 8.5 / 10 (3 votes)"));
        assert!(d.contains("23.976 fps"));
        assert!(d.contains("exact file (hash), hearing impaired"));
        assert_eq!(compact(1200), "1.2k");
        assert_eq!(compact(45_000), "45k");
        assert_eq!(compact(2_500_000), "2.5M");
    }

    #[test]
    fn query() {
        assert_eq!(
            query_from_name("/v/The.Wand.S01E05.1080p.WEB-DL.x264.mkv"),
            "The Wand S01E05"
        );
        assert_eq!(query_from_name("Movie (2020).mp4"), "Movie (2020)");
    }

    #[test]
    fn parse() {
        let v = serde_json::json!({"data": [
            {"attributes": {"language": "en", "download_count": 5, "moviehash_match": false,
                "files": [{"file_id": 1, "file_name": "a.srt"}]}},
            {"attributes": {"language": "uk", "download_count": 1, "moviehash_match": true,
                "release": "Rel", "files": [{"file_id": 2}]}}
        ]});
        let h = parse_search(&v);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].file_id, h[0].file_name.as_str()), (2, "Rel"));
        assert!(h[0].hash_match);
        assert_eq!(h[1].language, "en");
    }

    #[test]
    fn parse_badges() {
        let v = serde_json::json!({"data": [
            {"attributes": {"language": "en", "download_count": 5, "ratings": 7.5, "votes": 2,
                "hearing_impaired": true, "fps": 23.976, "from_trusted": true,
                "ai_translated": false, "machine_translated": true, "release": "Show.S01E02.WEB",
                "uploader": {"name": "bob"}, "upload_date": "2024-05-01T10:00:00Z",
                "feature_details": {"title": "Pilot", "parent_title": "Show", "season_number": 1,
                    "episode_number": 2, "year": 2024},
                "files": [{"file_id": 3, "file_name": "show.s01e02.srt"}]}},
            {"attributes": {"language": "uk", "feature_details": {"title": "Film", "year": 1999},
                "files": [{"file_id": 4}]}}
        ]});
        let h = parse_search(&v);
        let a = h.iter().find(|x| x.file_id == 3).unwrap();
        assert!(a.hearing_impaired && a.trusted && a.machine);
        assert_eq!((a.rating, a.votes, a.fps), (7.5, 2, 23.976));
        assert_eq!(a.title, "Show S01E02 · Pilot");
        assert_eq!(
            (a.uploader.as_str(), a.uploaded.as_str()),
            ("bob", "2024-05-01")
        );
        let b = h.iter().find(|x| x.file_id == 4).unwrap();
        assert_eq!(
            (b.title.as_str(), b.file_name.as_str()),
            ("Film (1999)", "subtitle")
        );
        // Human translation first even with fewer downloads.
        assert_eq!(h[0].file_id, 4);
    }
}

/// Live OpenSubtitles check: `VIVI_OPENSUBTITLES_KEY=… cargo test live_opensubtitles -- --ignored`.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore = "network + API key"]
    fn live_opensubtitles() {
        let key = std::env::var(KEY_ENV).expect("set VIVI_OPENSUBTITLES_KEY");
        let c = Creds {
            key,
            user: String::new(),
            pass: String::new(),
        };
        let h = search_all(&c, None, "tt0133093", "en", |_| {}).expect("search");
        assert!(!h.is_empty(), "no results for The Matrix");
        assert!(h.iter().all(|x| x.language == "en"));
        assert!(
            h.iter().any(|x| x.title.contains("Matrix")),
            "{:?}",
            h[0].title
        );
        let h2 = search_all(&c, None, "Sintel", "en,uk", |_| {}).expect("search by name");
        assert!(!h2.is_empty());
        eprintln!(
            "imdb: {} hits, first {:?} ↓{} ★{} fps {}; name: {} hits",
            h.len(),
            h[0].file_name,
            h[0].downloads,
            h[0].rating,
            h[0].fps,
            h2.len()
        );
    }
}
