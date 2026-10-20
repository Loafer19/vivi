//! Subtitle search and picks.
//!
//! **OpenSubtitles** (REST API): the playing file is searched by its OpenSubtitles
//! hash first (exact release matches), then by a query built from the file name, an
//! edited query or an IMDb id (`tt0133093`, also as an imdb.com URL). Results of both
//! are merged, hash matches first, with rating, downloads, hearing-impaired, FPS,
//! trusted-uploader and machine-translation badges. *Preview* downloads the subtitle
//! once into `$XDG_CACHE_HOME/vivi/subtitles` and shows its first lines; *Load* reuses
//! that copy (one download from the daily quota per subtitle). Optionally the file is
//! saved next to the video as `<video>.<lang>.<ext>` (setting *Save next to the video*).
//! The pick is remembered per file and loaded again the next time it plays. A query
//! with an episode (`Show S01E05`, `Show 1x05`) or a year (`Movie (2020)`) is sent as the
//! title plus `season_number` / `episode_number` or `year` (the plain text again when that
//! finds nothing). *Load exact matches automatically* (off by default): a local video that
//! opens without subtitles in the chosen languages gets the most downloaded exact (hash)
//! match, one download per new file, tried once per file and session.
//!
//! The API needs a (free) API key from <https://www.opensubtitles.com/consumers>,
//! entered in Settings → Extensions (or `VIVI_OPENSUBTITLES_KEY`). Username and
//! password are optional; logged-in users get more downloads per day. A login is reused
//! for 12 hours (in memory) and VIP accounts use their own API host. The key goes in the
//! `Api-Key` header only, never in a URL, a log line or an error message; download links
//! (one-time tokens) are not shown either. `VIVI_OPENSUBTITLES_API` points the plugin at
//! another API base (https, or http on this machine only; for tests with a local mock).
//!
//! The per-file **offset** (`z` / `x`, `Alt+Z` / `Alt+X`, side panel) is the player's
//! own (src/suboffset.rs) and works with this plugin off; *Auto-sync* hands the subtitle
//! to the Subtitle Auto-Sync package (alass / ffsubsync with the shared media ffmpeg).

use super::{contrib, Ctx, Manifest, Plugin, SettingSpec};
use crate::{Item, SubHit};
use slint::ComponentHandle;
use slint::{ModelRc, VecModel};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

mod api;
mod files;
pub use api::*;
pub use files::*;

pub const ID: &str = "subtitle-search";
const API: &str = "https://api.opensubtitles.com/api/v1";
pub const NO_KEY: &str = "OpenSubtitles needs an API key. Get a free one at opensubtitles.com/consumers and enter it in Settings → Extensions → Subtitle Search.";
/// Environment fallback for the API key (Settings wins).
const KEY_ENV: &str = "VIVI_OPENSUBTITLES_KEY";
/// Environment override of the API base (see [`api_base`]).
const API_ENV: &str = "VIVI_OPENSUBTITLES_API";
/// Package that aligns subtitles to the audio (alass / ffsubsync + shared ffmpeg).
use crate::suboffset::SYNC_PACKAGE;

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

/// Result of *Load exact matches automatically* for the file with key `key`.
struct Auto {
    key: String,
    r: Result<Option<(Hit, PathBuf, Option<i64>)>, String>,
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
    /// "From Computer": scanned folder, its subtitle files (To This File) with the
    /// checked ones, or its videos paired with subtitles (To This Folder).
    dir: Option<PathBuf>,
    subs: Vec<PathBuf>,
    checked: Vec<bool>,
    pairs: Vec<(PathBuf, Option<PathBuf>)>,
    /// Subtitles assigned with "To This Folder", attached when that video loads.
    assigned: HashMap<PathBuf, PathBuf>,
    /// Automatic exact-match lookup running for the playing file.
    auto_rx: Option<Receiver<Auto>>,
    /// Files tried automatically this session (one lookup per file).
    auto_tried: std::collections::HashSet<String>,
}

/// mpv / Matroska track languages (ISO 639-2) for the chip codes.
const ISO3: &[(&str, &[&str])] = &[
    ("en", &["eng"]),
    ("uk", &["ukr"]),
    ("es", &["spa"]),
    ("fr", &["fre", "fra"]),
    ("de", &["ger", "deu"]),
    ("it", &["ita"]),
    ("pt", &["por"]),
    ("pl", &["pol"]),
    ("nl", &["dut", "nld"]),
    ("tr", &["tur"]),
    ("ru", &["rus"]),
    ("ar", &["ara"]),
    ("zh", &["chi", "zho"]),
    ("ja", &["jpn"]),
    ("ko", &["kor"]),
];

/// Whether a track language (`en`, `eng`, `pt-BR`, `por`) is one of the codes in `langs`.
pub(crate) fn track_lang_in(track: &str, langs: &str) -> bool {
    let t = track.trim().to_lowercase();
    let t = t.split(['-', '_']).next().unwrap_or("");
    norm_langs(langs)
        .split(',')
        .filter(|c| !c.is_empty())
        .any(|c| {
            let base = c.split('-').next().unwrap_or(c);
            t == base
                || ISO3
                    .iter()
                    .any(|(two, three)| *two == base && three.contains(&t))
        })
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
        let folder = cx.app.global::<crate::Subtitles>().get_target() == 1;
        let Some(dir) = self.dir.clone() else {
            cx.app
                .global::<crate::Subtitles>()
                .set_folder("Choose a folder…".into());
            cx.app
                .global::<crate::Subtitles>()
                .set_local(ModelRc::default());
            Self::status(
                cx,
                "Open a local file or click the folder bar to choose a folder.",
                false,
            );
            return;
        };
        cx.app
            .global::<crate::Subtitles>()
            .set_folder(breadcrumb(&dir).into());
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
        cx.app
            .global::<crate::Subtitles>()
            .set_local(ModelRc::new(VecModel::from(items)));
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
        if cx.app.global::<crate::Subtitles>().get_target() == 0 {
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
        cx.app.global::<crate::Subtitles>().set_status(s.into());
        cx.app.global::<crate::Subtitles>().set_busy(busy);
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
        cx.app
            .global::<crate::Subtitles>()
            .set_hits(ModelRc::new(VecModel::from(rows)));
        self.hits = hits;
        self.select(cx, None);
    }
    fn select(&mut self, cx: &Ctx, i: Option<usize>) {
        self.selected = i.filter(|i| *i < self.hits.len());
        cx.app
            .global::<crate::Subtitles>()
            .set_selected(self.selected.map(|i| i as i32).unwrap_or(-1));
        let h = self.selected.and_then(|i| self.hits.get(i));
        cx.app
            .global::<crate::Subtitles>()
            .set_details(h.map(details).unwrap_or_default().into());
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
        cx.app.global::<crate::Subtitles>().set_preview(pre.into());
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
        cx.app
            .global::<crate::Subtitles>()
            .set_langs(ModelRc::new(VecModel::from(items)));
        cx.app
            .global::<crate::Subtitles>()
            .set_lang(norm_langs(langs).into());
    }
    fn set_langs(&mut self, cx: &Ctx, langs: &str) {
        let l = norm_langs(langs);
        cx.set("languages", &l);
        Self::show_langs(cx, &l);
    }
    /// Key of the playing file for picks (canonical path or URL).
    fn file_key(cx: &Ctx) -> Option<String> {
        if let Some(p) = cx.local_path() {
            let p = std::fs::canonicalize(&p).unwrap_or(p);
            return Some(p.to_string_lossy().into_owned());
        }
        cx.player.get("path").filter(|p| !p.is_empty())
    }
    /// "English · saved next to the video" (the pick loads again next time).
    fn show_pick(cx: &Ctx, pick: Option<&Pick>, video: Option<&Path>) {
        cx.app.global::<crate::Subtitles>().set_pick_label(
            pick.map(|p| pick_label(p, video))
                .unwrap_or_default()
                .into(),
        );
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
    /// `auto`: loaded by *Load exact matches automatically* (the dialog stays as it is).
    fn use_file(&mut self, cx: &Ctx, cachedp: &Path, file_id: i64, lang: &str, auto: bool) {
        let mut path = cachedp.to_path_buf();
        let mut note = String::new();
        if cx.flag("save-next") {
            let data = std::fs::read(cachedp).ok().filter(|d| !d.is_empty());
            if let (Some(video), Some(data)) = (cx.local_path(), data) {
                let ext = cachedp
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .filter(|e| has_ext(Path::new(&format!("x.{e}")), SUB_EXT))
                    .unwrap_or_else(|| "srt".into());
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
            cx.app.global::<crate::Subtitles>().set_has_pick(true);
            Self::show_pick(cx, self.pick.as_ref(), cx.local_path().as_deref());
        }
        if auto {
            let l = Some(lang)
                .filter(|l| !l.is_empty())
                .map(|l| format!(": {}", lang_name(l)))
                .unwrap_or_default();
            cx.toast(&format!("Exact-match subtitles loaded{l}{note}"));
            return;
        }
        Self::status(cx, "", false);
        cx.app.global::<crate::Overlays>().set_subs_open(false);
        cx.toast(&format!("Subtitles loaded{note}"));
    }
    /// Whether the playing file has a subtitle track that may be in `langs` (one with
    /// another known language does not count; external files and unknown languages do).
    fn has_sub_in(cx: &Ctx, langs: &str) -> bool {
        let n: usize = cx
            .player
            .get("track-list/count")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        (0..n).any(|i| {
            let g = |k: &str| {
                cx.player
                    .get(&format!("track-list/{i}/{k}"))
                    .unwrap_or_default()
            };
            if g("type") != "sub" {
                return false;
            }
            let lang = g("lang");
            g("external") == "yes"
                || lang.is_empty()
                || langs.is_empty()
                || track_lang_in(&lang, langs)
        })
    }
    /// *Load exact matches automatically*: look up the playing local file by its hash
    /// once per session, in the background.
    fn auto_exact(&mut self, cx: &Ctx) {
        if !cx.flag("auto-exact") || self.pick.is_some() {
            return;
        }
        let (Some(key), Some(path)) = (self.key.clone(), cx.local_path()) else {
            return;
        };
        let langs = norm_langs(&cx.get("languages"));
        let c = Self::creds(cx);
        if c.key.is_empty() || Self::has_sub_in(cx, &langs) || !self.auto_tried.insert(key.clone())
        {
            return;
        }
        let (tx, rx) = channel();
        self.auto_rx = Some(rx);
        let dir = crate::store::cache_dir().join("subtitles");
        std::thread::spawn(move || {
            let r = auto_pick(&c, &path, &langs, &dir);
            let _ = tx.send(Auto { key, r });
        });
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
                self.use_file(cx, &p, h.file_id, &h.language, false);
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
        let query = cx.app.global::<crate::Subtitles>().get_query().to_string();
        let langs = norm_langs(&cx.app.global::<crate::Subtitles>().get_lang());
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
            "Find subtitles on OpenSubtitles (by file hash, name or IMDb id) in the Add Subtitles dialog; the pick is remembered per file.",
            "subtitles",
        )
        .contribute(contrib::MENU_SUBTITLES, "Search online…", "search-online")
        .contribute(contrib::PALETTE_COMMAND, "Search subtitles online", "search-online")
        .contribute(contrib::CONTROLBAR_BUTTON, "Search subtitles online", "search-online")
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
        .setting(SettingSpec::toggle(
            "auto-exact",
            "Load exact matches automatically",
            "When a local video opens without subtitles in your languages, load the most downloaded subtitle made for that exact file (hash match). Uses one download per new file.",
            false,
        ))
    }
    fn enable(&mut self, cx: &Ctx) {
        cx.app.global::<crate::Subtitles>().set_plugin(true);
    }
    fn disable(&mut self, cx: &Ctx) {
        cx.app.global::<crate::Subtitles>().set_plugin(false);
        cx.app.global::<crate::Overlays>().set_subs_open(false);
        self.rx = None;
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let arg = args.first().map(String::as_str).unwrap_or("");
        let idx = arg.parse::<usize>().ok();
        match name {
            // Subtitles menu / command palette: open the dialog itself.
            "search-online" => {
                cx.app.global::<crate::Overlays>().set_subs_open(true);
                self.command(cx, "open", args);
            }
            "open" => {
                let path = cx.player.get("path").unwrap_or_default();
                cx.app.global::<crate::Subtitles>().set_query(query_from_name(&path).into());
                Self::show_langs(cx, &cx.get("languages"));
                cx.app.global::<crate::Subtitles>().set_save_next(cx.flag("save-next"));
                cx.app.global::<crate::Subtitles>().set_auto_exact(cx.flag("auto-exact"));
                cx.app.global::<crate::Subtitles>().set_sync(Self::sync_available(cx).is_some());
                self.set_hits(cx, Vec::new());
                self.dir = cx
                    .local_path()
                    .and_then(|p| p.parent().map(Path::to_path_buf));
                // Without an API key only the local source works.
                if Self::creds(cx).key.is_empty() && std::env::var_os("VIVI_SUBS").is_none() {
                    cx.app.global::<crate::Subtitles>().set_source(1);
                }
                if cx.app.global::<crate::Subtitles>().get_source() == 1 {
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
                let l = toggle_lang(&cx.app.global::<crate::Subtitles>().get_lang(), arg);
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
                cx.app.global::<crate::Subtitles>().set_save_next(on);
            }
            "auto-exact" => {
                let on = !cx.flag("auto-exact");
                cx.set("auto-exact", if on { "true" } else { "false" });
                cx.app.global::<crate::Subtitles>().set_auto_exact(on);
            }
            "mode" => {
                if cx.app.global::<crate::Subtitles>().get_source() == 1 {
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
                if cx.app.global::<crate::Subtitles>().get_target() == 0 {
                    if let Some(c) = self.checked.get_mut(i) {
                        *c = !*c;
                    }
                    cx.app.global::<crate::Subtitles>().set_local(ModelRc::new(VecModel::from(self.local_items())));
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
                self.use_file(cx, Path::new(p), id, &lang, false);
            }
            "forget" => {
                if let Some(k) = &self.key {
                    save_pick(&picks_file(), k, None);
                }
                self.pick = None;
                cx.app.global::<crate::Subtitles>().set_has_pick(false);
                cx.toast("Subtitle pick forgotten for this file");
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
            _ => None,
        }
    }
    fn file_loaded(&mut self, cx: &Ctx) {
        self.key = Self::file_key(cx);
        self.fetched.clear();
        self.auto_rx = None;
        cx.app
            .global::<crate::Subtitles>()
            .set_sync(Self::sync_available(cx).is_some());
        // The remembered pick, unless mpv already loaded that file (sub-auto).
        self.pick = self
            .key
            .as_ref()
            .and_then(|k| load_picks(&picks_file()).remove(k))
            .filter(|p| Path::new(&p.path).is_file());
        cx.app
            .global::<crate::Subtitles>()
            .set_has_pick(self.pick.is_some());
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
        } else {
            self.auto_exact(cx);
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        if let Some(a) = self.auto_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.auto_rx = None;
            // Still the same file, and nothing picked meanwhile.
            if Some(&a.key) == self.key.as_ref() && self.pick.is_none() {
                match a.r {
                    Ok(Some((h, p, _))) => self.use_file(cx, &p, h.file_id, &h.language, true),
                    Ok(None) => {}
                    Err(e) => eprintln!("vivi: subtitle-search: automatic lookup: {e}"),
                }
            }
        }
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
                                self.use_file(cx, &p, file_id, &lang, false);
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
            text: " Sintel ".into(),
            ..Default::default()
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
        let q = Query::from_text("tt0133093");
        let p = search_params(&q, "");
        assert_eq!(p, vec![("imdb_id", "133093".to_string())]);
        // Season / episode and year go in their own parameters, still sorted.
        let p = search_params(&Query::from_text("The Wand S01E05"), "en");
        assert_eq!(
            p,
            vec![
                ("episode_number", "5".to_string()),
                ("languages", "en".to_string()),
                ("query", "the wand".to_string()),
                ("season_number", "1".to_string()),
            ]
        );
        let p = search_params(&Query::from_text("Sintel (2010)"), "");
        assert_eq!(
            p,
            vec![
                ("query", "sintel".to_string()),
                ("year", "2010".to_string())
            ]
        );
    }

    #[test]
    fn episode_and_year_queries() {
        let q = |t: &str| {
            let q = Query::from_text(t);
            (q.text, q.season, q.episode, q.year)
        };
        assert_eq!(
            q("The Wand S01E05"),
            ("The Wand".into(), Some(1), Some(5), None)
        );
        assert_eq!(q("show s2e10e11"), ("show".into(), Some(2), Some(10), None));
        assert_eq!(q("Show 3x07"), ("Show".into(), Some(3), Some(7), None));
        assert_eq!(q("Movie (2020)"), ("Movie".into(), None, None, Some(2020)));
        assert_eq!(
            q("Blade Runner 2049"),
            ("Blade Runner".into(), None, None, Some(2049))
        );
        // A year alone is a title; a leading episode code keeps the text; others stay.
        assert_eq!(q("1917"), ("1917".into(), None, None, None));
        assert_eq!(q("S01E05"), ("S01E05".into(), None, None, None));
        assert_eq!(q("Room 101"), ("Room 101".into(), None, None, None));
        assert_eq!(
            q("Sex Education 4x"),
            ("Sex Education 4x".into(), None, None, None)
        );
        assert!(!Query::from_text("Sintel").narrowed());
        let n = Query::from_text("Movie 2020");
        assert!(n.narrowed());
        assert!(!n.plain("Movie 2020").narrowed());
        assert_eq!(n.plain("Movie 2020").text, "Movie 2020");
    }

    #[test]
    fn urls_and_languages_are_checked() {
        assert!(safe_url("https://api.opensubtitles.com/api/v1"));
        assert!(safe_url("http://127.0.0.1:8099/api/v1"));
        assert!(safe_url("http://localhost/x"));
        assert!(safe_url("http://[::1]:80/x"));
        assert!(!safe_url("http://example.com/api"));
        assert!(!safe_url("http://127.0.0.1.evil.net/api"));
        assert!(!safe_url("https://user:pw@host/x"));
        assert!(!safe_url("http://localhost@evil.net/x"));
        assert!(!safe_url("file:///etc/passwd"));
        assert!(!safe_url("ftp://x"));
        assert_eq!(safe_lang(" EN "), "en");
        assert_eq!(safe_lang("pt-BR"), "pt-br");
        assert_eq!(safe_lang("../../x"), "");
        assert_eq!(safe_lang("en/../../x"), "");
        assert_eq!(safe_lang("-rf"), "");
        // A hostile language never leaves the video's folder.
        assert_eq!(
            sidecar_path(Path::new("/m/a.mkv"), "/../../evil", "srt", 1, |_| false),
            PathBuf::from("/m/a.srt")
        );
        assert_eq!(
            session_base(API, "vip-api.opensubtitles.com"),
            "https://vip-api.opensubtitles.com/api/v1"
        );
        assert_eq!(session_base(API, "evil.net"), API);
        assert_eq!(session_base(API, "opensubtitles.com.evil.net"), API);
        assert_eq!(session_base(API, ""), API);
        assert_eq!(
            session_base("http://127.0.0.1:1/api/v1", "vip-api.opensubtitles.com"),
            "http://127.0.0.1:1/api/v1"
        );
    }

    #[test]
    fn auto_pick_choice() {
        let h = |id, dl, hash, machine, hi| Hit {
            file_id: id,
            downloads: dl,
            hash_match: hash,
            machine,
            hearing_impaired: hi,
            ..Default::default()
        };
        let v = vec![
            h(1, 900, false, false, false),
            h(2, 500, true, true, false),
            h(3, 300, true, false, true),
            h(4, 100, true, false, false),
        ];
        assert_eq!(best_exact(&v).map(|x| x.file_id), Some(4));
        assert_eq!(best_exact(&v[..3]).map(|x| x.file_id), Some(3));
        assert_eq!(best_exact(&v[..2]), None);
        assert!(track_lang_in("eng", "en,uk"));
        assert!(track_lang_in("ukr", "en,uk"));
        assert!(track_lang_in("pt-BR", "pt-br"));
        assert!(track_lang_in("por", "pt-br"));
        assert!(track_lang_in("ger", "de"));
        assert!(!track_lang_in("fre", "en,uk"));
        assert!(!track_lang_in("eng", ""));
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

/// Download against a local mock of the OpenSubtitles API (no network, no account).
#[cfg(test)]
mod mock {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// Serve `n` requests; `answer(method, path, auth)` gives the status and body.
    /// Returns the base URL and the log of "METHOD path auth api-key" lines.
    fn serve(
        n: usize,
        answer: impl Fn(&str, &str, &str, u16) -> (u16, String) + Send + 'static,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let lg = log.clone();
        std::thread::spawn(move || {
            for s in l.incoming().take(n) {
                let mut s = s.unwrap();
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let (mut auth, mut key, mut len) = (String::new(), String::new(), 0usize);
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    let h = h.trim_end().to_string();
                    if h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap_or((&h, ""));
                    match k.to_ascii_lowercase().as_str() {
                        "authorization" => auth = v.trim().to_string(),
                        "api-key" => key = v.trim().to_string(),
                        "content-length" => len = v.trim().parse().unwrap_or(0),
                        _ => {}
                    }
                }
                let mut body = vec![0u8; len];
                std::io::Read::read_exact(&mut r, &mut body).unwrap();
                let mut it = first.split_whitespace();
                let (m, path) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
                lg.lock().unwrap().push(format!("{m} {path} {auth} {key}"));
                let (code, out) = answer(m, path, &auth, port);
                let _ = write!(
                    s,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                    out.len()
                );
            }
        });
        (format!("http://127.0.0.1:{port}/api/v1"), log)
    }

    fn creds(user: &str) -> Creds {
        Creds {
            key: "KEY123".into(),
            user: user.into(),
            pass: "pw".into(),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vivi-subs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn login_reused_and_renewed_once() {
        let logins = Arc::new(Mutex::new(0));
        let lg = logins.clone();
        let (base, log) = serve(7, move |m, path, auth, port| match (m, path) {
            ("POST", "/api/v1/login") => {
                let mut n = lg.lock().unwrap();
                *n += 1;
                (
                    200,
                    format!(r#"{{"token":"T{n}","base_url":"evil.example"}}"#),
                )
            }
            // The first token is refused (expired): Vivi logs in again once.
            ("POST", "/api/v1/download") if auth == "Bearer T1" => (401, "{}".into()),
            ("POST", "/api/v1/download") => (
                200,
                format!(
                    r#"{{"link":"http://127.0.0.1:{port}/f/tok","file_name":"../a/b.srt","remaining":7}}"#
                ),
            ),
            ("GET", "/f/tok") => (200, "1\n00:00:01,000 --> 00:00:02,000\nHi\n".into()),
            _ => (404, "{}".into()),
        });
        let d = tmp("login");
        let (p, left) = download_from(&base, &creds("u"), 7, &d).unwrap();
        assert_eq!(p, d.join("7-.._a_b.srt"));
        assert_eq!(left, Some(7));
        assert!(std::fs::read_to_string(&p).unwrap().contains("Hi"));
        // A second file reuses the new login (no third login).
        download_from(&base, &creds("u"), 8, &d).unwrap();
        assert_eq!(*logins.lock().unwrap(), 2);
        let log = log.lock().unwrap().join("\n");
        // The key only ever went in the Api-Key header, never in a path.
        assert!(
            !log.contains("/api/v1/download KEY123") && !log.contains("?"),
            "{log}"
        );
        assert!(log
            .lines()
            .filter(|l| l.starts_with("POST"))
            .all(|l| l.ends_with("KEY123")));
        assert!(log
            .lines()
            .filter(|l| l.starts_with("GET"))
            .all(|l| !l.contains("KEY123")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unsafe_link_and_errors_hide_secrets() {
        let (base, _) = serve(1, |_, _, _, _| {
            (
                200,
                r#"{"link":"http://example.com/f/SECRETTOKEN","file_name":"x.srt"}"#.into(),
            )
        });
        let d = tmp("link");
        let e = download_from(&base, &creds(""), 9, &d).unwrap_err();
        assert!(
            !e.contains("SECRETTOKEN") && !e.contains("example.com"),
            "{e}"
        );
        assert!(cached(&d, 9).is_none());
        // Quota answers keep the API's message; network errors name no URL.
        let (base, _) = serve(1, |_, _, _, _| {
            (
                406,
                r#"{"message":"You have downloaded your allowed 20 subtitles"}"#.into(),
            )
        });
        let e = download_from(&base, &creds(""), 10, &d).unwrap_err();
        assert!(e.contains("allowed 20 subtitles"), "{e}");
        let e = download_from("http://127.0.0.1:9/api/v1", &creds(""), 11, &d).unwrap_err();
        assert!(!e.contains("127.0.0.1") && !e.contains("KEY123"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
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
