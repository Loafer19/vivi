//! Subtitle search: finds subtitles on OpenSubtitles (REST API) by the file's
//! OpenSubtitles hash and a query built from the file name, downloads the chosen one
//! to `$XDG_CACHE_HOME/vivi/subtitles` and loads it with mpv `sub-add`.
//!
//! The API needs a (free) API key from <https://www.opensubtitles.com/consumers>,
//! entered in Settings → Extensions. Username and password are optional; OpenSubtitles
//! allows more downloads per day for logged-in users.

use super::{Ctx, Manifest, Plugin, SettingSpec};
use crate::Item;
use slint::{ModelRc, VecModel};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

const API: &str = "https://api.opensubtitles.com/api/v1";
pub const NO_KEY: &str = "OpenSubtitles needs an API key. Get a free one at opensubtitles.com/consumers and enter it in Settings → Extensions → Subtitle Search.";

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

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub file_id: i64,
    pub file_name: String,
    pub language: String,
    pub downloads: i64,
    pub hash_match: bool,
}

/// Parse `/subtitles` JSON into hits (hash matches first, then by downloads).
pub fn parse_search(v: &serde_json::Value) -> Vec<Hit> {
    let mut out = Vec::new();
    for d in v["data"].as_array().into_iter().flatten() {
        let a = &d["attributes"];
        for f in a["files"].as_array().into_iter().flatten() {
            let Some(id) = f["file_id"].as_i64() else {
                continue;
            };
            out.push(Hit {
                file_id: id,
                file_name: f["file_name"]
                    .as_str()
                    .or(a["release"].as_str())
                    .unwrap_or("subtitle")
                    .to_string(),
                language: a["language"].as_str().unwrap_or("").to_string(),
                downloads: a["download_count"].as_i64().unwrap_or(0),
                hash_match: a["moviehash_match"].as_bool().unwrap_or(false),
            });
        }
    }
    out.sort_by(|a, b| {
        b.hash_match
            .cmp(&a.hash_match)
            .then(b.downloads.cmp(&a.downloads))
    });
    out
}

#[derive(Clone)]
struct Creds {
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
        429 => "OpenSubtitles: too many requests, try again later".into(),
        _ if msg.is_empty() => format!("OpenSubtitles error {code}"),
        _ => format!("OpenSubtitles: {msg}"),
    }
}

/// Query parameters in the order the API wants (alphabetical, lowercase values).
fn search_params(hash: Option<&str>, query: &str, langs: &str) -> Vec<(&'static str, String)> {
    let mut p = Vec::new();
    if !langs.trim().is_empty() {
        let mut l: Vec<String> = langs
            .split(',')
            .map(|x| x.trim().to_lowercase())
            .filter(|x| !x.is_empty())
            .collect();
        l.sort();
        l.dedup();
        p.push(("languages", l.join(",")));
    }
    if let Some(h) = hash {
        p.push(("moviehash", h.to_lowercase()));
    }
    if !query.trim().is_empty() {
        p.push(("query", query.trim().to_lowercase()));
    }
    p
}

fn search(c: &Creds, hash: Option<String>, query: &str, langs: &str) -> Result<Vec<Hit>, String> {
    let a = agent();
    let mut req = a
        .get(format!("{API}/subtitles"))
        .header("Api-Key", &c.key)
        .header("Accept", "application/json");
    // OpenSubtitles answers 301 unless parameters are lowercase and sorted by name.
    for (k, v) in search_params(hash.as_deref(), query, langs) {
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

fn download(c: &Creds, file_id: i64, dir: &Path) -> Result<PathBuf, String> {
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
    let name = v["file_name"].as_str().unwrap_or("subtitle.srt");
    let name: String = name
        .chars()
        .map(|ch| if "/\\:".contains(ch) { '_' } else { ch })
        .collect();
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
    Ok(out)
}

enum Msg {
    Results(Result<Vec<Hit>, String>),
    Downloaded(Result<PathBuf, String>),
}

#[derive(Default)]
pub struct SubtitleSearch {
    hits: Vec<Hit>,
    rx: Option<Receiver<Msg>>,
}

impl SubtitleSearch {
    fn creds(cx: &Ctx) -> Creds {
        Creds {
            key: cx.get("api-key").trim().to_string(),
            user: cx.get("username").trim().to_string(),
            pass: cx.get("password"),
        }
    }
    fn status(cx: &Ctx, s: &str, busy: bool) {
        cx.app.set_subs_status(s.into());
        cx.app.set_subs_busy(busy);
    }
    fn set_hits(&mut self, cx: &Ctx, hits: Vec<Hit>, selected: Option<usize>) {
        let items: Vec<Item> = hits
            .iter()
            .enumerate()
            .map(|(i, h)| Item {
                key: h.file_name.as_str().into(),
                label: format!(
                    "{}{}  ·  {} downloads",
                    h.language.to_uppercase(),
                    if h.hash_match {
                        "  ·  exact match"
                    } else {
                        ""
                    },
                    h.downloads
                )
                .into(),
                selected: Some(i) == selected,
            })
            .collect();
        cx.app.set_subs_results(ModelRc::new(VecModel::from(items)));
        self.hits = hits;
    }
}

impl Plugin for SubtitleSearch {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            "subtitle-search",
            "Subtitle Search",
            "Find and download subtitles from OpenSubtitles in the Add Subtitles dialog.",
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
            "Comma-separated ISO 639-1 codes, e.g. en,uk.",
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
    }
    fn enable(&mut self, cx: &Ctx) {
        cx.app.set_subs_plugin(true);
    }
    fn disable(&mut self, cx: &Ctx) {
        cx.app.set_subs_plugin(false);
        cx.app.set_subs_open(false);
        self.rx = None;
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        match name {
            "open" => {
                let path = cx.player.get("path").unwrap_or_default();
                cx.app.set_subs_query(query_from_name(&path).into());
                cx.app.set_subs_lang(cx.get("languages").into());
                self.set_hits(cx, Vec::new(), None);
                if Self::creds(cx).key.is_empty() {
                    Self::status(cx, NO_KEY, false);
                } else {
                    Self::status(cx, "", false);
                    self.command(cx, "search", &[]);
                }
            }
            "search" => {
                let c = Self::creds(cx);
                if c.key.is_empty() {
                    Self::status(cx, NO_KEY, false);
                    return;
                }
                let query = cx.app.get_subs_query().to_string();
                let langs = cx.app.get_subs_lang().to_string();
                let local = cx.local_path();
                let (tx, rx) = channel();
                self.rx = Some(rx);
                self.set_hits(cx, Vec::new(), None);
                Self::status(cx, "Searching OpenSubtitles…", true);
                std::thread::spawn(move || {
                    let hash = local.and_then(|p| moviehash(&p).ok());
                    let mut r = search(&c, hash.clone(), &query, &langs);
                    // The hash alone can over-filter: retry with the name only.
                    if hash.is_some() && matches!(&r, Ok(v) if v.is_empty()) && !query.is_empty() {
                        r = search(&c, None, &query, &langs);
                    }
                    let _ = tx.send(Msg::Results(r));
                });
            }
            "select" => {
                let i = args.first().and_then(|a| a.parse::<usize>().ok());
                let hits = std::mem::take(&mut self.hits);
                self.set_hits(cx, hits, i);
            }
            "download" => {
                let hit = args.first().and_then(|a| a.parse::<usize>().ok());
                let Some(id) = hit.and_then(|i| self.hits.get(i)).map(|h| h.file_id) else {
                    Self::status(cx, "Select a subtitle first", false);
                    return;
                };
                let c = Self::creds(cx);
                let (tx, rx) = channel();
                self.rx = Some(rx);
                Self::status(cx, "Downloading…", true);
                let dir = crate::store::cache_dir().join("subtitles");
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::Downloaded(download(&c, id, &dir)));
                });
            }
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        let Some(rx) = &self.rx else { return };
        let Ok(m) = rx.try_recv() else { return };
        self.rx = None;
        match m {
            Msg::Results(Ok(h)) => {
                let s = if h.is_empty() {
                    "No subtitles found. Try another query or language.".to_string()
                } else {
                    format!("{} subtitles found", h.len())
                };
                self.set_hits(cx, h, None);
                Self::status(cx, &s, false);
            }
            Msg::Downloaded(Ok(p)) => {
                cx.player.cmd("sub-add", &[&p.to_string_lossy(), "select"]);
                Self::status(cx, "", false);
                cx.app.set_subs_open(false);
                cx.toast("Subtitles loaded");
            }
            Msg::Results(Err(e)) | Msg::Downloaded(Err(e)) => Self::status(cx, &e, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let p = search_params(Some("ABC"), " Sintel ", "uk, EN");
        let keys: Vec<&str> = p.iter().map(|(k, _)| *k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(p[0].1, "en,uk");
        assert_eq!(p[1].1, "abc");
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
}
