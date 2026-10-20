//! Network Streams browser (#67): Internet radio from the radio-browser.info directory
//! (no key), M3U / M3U8 / IPTV playlist import (file or URL, with `tvg-logo` and
//! `group-title`), saved streams and recently played streams, plus a stream info
//! readout (ICY now-playing title, codec, bitrate, buffer).
//!
//! Also: an on-demand XMLTV program guide (now / next per `tvg-id`, see [`super::epg`]),
//! favorites pinned on top, `group-title` chips, iptv-org category chips, custom M3U
//! playlists that refresh themselves ("My playlists"), a TV + radio search, and Play /
//! Add to queue / Mini player actions. Nothing is bundled: lists, logos and guides are
//! fetched when needed and cached under `$XDG_CACHE_HOME/vivi/{iptv,stream-logos,epg}`.
//!
//! It is its own dialog (the shared plugin dialog), separate from the Open Network
//! Stream dialog. Saved and recent streams live in
//! `$XDG_DATA_HOME/vivi/streams/{saved,recent,playlists}.json` (favorites are `saved`).

use super::web;
use super::{contrib, Ctx, Manifest, Plugin, SettingSpec};
use crate::Item;
use serde::{Deserialize, Serialize};
use slint::ComponentHandle;
use slint::{ModelRc, SharedString, VecModel};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

mod library;
mod sources;
pub use library::*;
pub use sources::*;

pub const ID: &str = "network-streams";
pub const SOURCES: [&str; 13] = [
    "Radio: search",
    "Radio: top voted",
    "Radio: by country",
    "Radio: by genre",
    "M3U / IPTV playlist",
    "TV: by country",
    "TV: by category",
    "Music: genres",
    "Music: SomaFM",
    "Favorites",
    "Recent",
    "My playlists",
    "Search all (TV + radio)",
];
const M3U: usize = 4;
const TV_COUNTRY: usize = 5;
const TV_CATEGORY: usize = 6;
const GENRES: usize = 7;
const SOMAFM: usize = 8;
const SAVED: usize = 9;
const RECENT: usize = 10;
const MY_PL: usize = 11;
const ALL: usize = 12;
/// iptv-org categories offered as chips (label, category id).
const CATEGORY_CHIPS: [(&str, &str); 12] = [
    ("News", "news"),
    ("Sports", "sports"),
    ("Movies", "movies"),
    ("Kids", "kids"),
    ("Music", "music"),
    ("Entertainment", "entertainment"),
    ("Documentary", "documentary"),
    ("Series", "series"),
    ("Comedy", "comedy"),
    ("Animation", "animation"),
    ("Education", "education"),
    ("Culture", "culture"),
];
const MAX_CHIPS: usize = 12;
/// Key prefix of "My playlists" picker entries.
const PL: &str = "pl:";
/// Sources whose list is filtered locally as the user types.
fn live_filter(mode: usize) -> bool {
    (TV_COUNTRY..=SOMAFM).contains(&mode) || mode == MY_PL
}
/// `source` of picker entries (a country / category / genre to open).
const PICK: &str = "pick";
const IPTV: &str = "https://iptv-org.github.io/iptv";
const IPTV_API: &str = "https://iptv-org.github.io/api";
/// radio-browser.info mirrors used when the server list cannot be fetched (`all` is the
/// DNS round robin of every mirror). The live list comes from [`radio_servers`].
const FALLBACK_SERVERS: [&str; 4] = [
    "https://all.api.radio-browser.info",
    "https://de1.api.radio-browser.info",
    "https://de2.api.radio-browser.info",
    "https://fi1.api.radio-browser.info",
];
const MAX_RECENT: usize = 30;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stream {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub logo: String,
    /// Genre tags or IPTV group.
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub source: String,
    /// `tvg-id` of IPTV channels (program guide key).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tvg_id: String,
    /// XMLTV guide URL from the playlist header (`url-tvg` / `x-tvg-url`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub epg: String,
    /// radio-browser.info station id (its click counter is `/json/url/<id>`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stationuuid: String,
}

impl Stream {
    fn label(&self) -> String {
        let mut p: Vec<String> = Vec::new();
        if !self.country.is_empty() {
            p.push(self.country.clone());
        }
        if !self.codec.is_empty() {
            p.push(if self.bitrate > 0 {
                format!("{} {} kbps", self.codec, self.bitrate)
            } else {
                self.codec.clone()
            });
        }
        if !self.group.is_empty() {
            p.push(self.group.chars().take(60).collect());
        }
        if p.is_empty() {
            p.push(self.url.clone());
        }
        p.join("  ·  ")
    }
    fn detail(&self) -> String {
        let mut s = format!("{}\n\n{}\n", display_name(&self.name), self.url);
        for (k, v) in [
            ("Country", &self.country),
            ("Codec", &self.codec),
            ("Tags / group", &self.group),
            ("Homepage", &self.homepage),
            ("Logo", &self.logo),
            ("From", &self.source),
        ] {
            if !v.is_empty() {
                s += &format!("\n{k}: {v}");
            }
        }
        if self.bitrate > 0 {
            s += &format!("\nBitrate: {} kbps", self.bitrate);
        }
        s
    }
}

/// Decode `%XX` escapes (file names taken from URLs); invalid escapes stay as they are.
pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Name for display: percent-decoded, trimmed.
pub fn display_name(n: &str) -> String {
    pct_decode(n).trim().to_string()
}

// ---------- plugin ----------

#[derive(Default)]
pub struct NetworkStreams {
    list: Vec<Stream>,
    /// Source index the current list came from.
    mode: usize,
    task: crate::util::Task<Result<Vec<Stream>, String>>,
    /// Status shown when the results arrive instead of the count (fallback searches).
    note: Option<String>,
    /// Source shown in the dropdown last tick (placeholder/button follow it).
    last_source: String,
    /// Station picked in the browser, for the now-playing card.
    playing: Option<Stream>,
    np_url: String,
    logo_task: crate::util::Task<(String, Option<PathBuf>)>,
    poll: u32,
    /// Unfiltered results; `list` is its view (group chip + live filter + favorites first).
    all: Vec<Stream>,
    filtered_by: String,
    /// Row logos (by logo URL) and their background loader.
    logos: std::collections::HashMap<String, slint::Image>,
    row_logo_rx: Option<Receiver<(String, Option<PathBuf>)>>,
    logos_dirty: bool,
    /// Active group chip ("" = all).
    chip: String,
    /// Chips are iptv-org categories (open a playlist) rather than groups (filter).
    category_chips: bool,
    /// Program guide (merged guides) and the guide URLs already requested.
    epg: super::epg::Guide,
    epg_asked: std::collections::HashSet<String>,
    epg_rx: Option<Receiver<Result<super::epg::Guide, String>>>,
    /// Favorite URLs.
    favs: std::collections::HashSet<String>,
    /// Playlist path / URL the current M3U results came from.
    loaded_src: String,
    selected: Option<usize>,
    /// Results shown as logo tiles (the "view" setting); row logos are then decoded at
    /// tile size.
    grid: bool,
    /// URL last checked by `is_station` and whether it is one of our stations.
    known: (String, bool),
}

impl NetworkStreams {
    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app
            .global::<crate::MusicDialogState>()
            .set_status(status.into());
        cx.app.global::<crate::MusicDialogState>().set_busy(busy);
    }
    fn reload_favs(&mut self) {
        self.favs = load_list("saved.json").into_iter().map(|s| s.url).collect();
    }
    fn ttl(cx: &Ctx) -> u64 {
        match cx.get("refresh").as_str() {
            "6 hours" => 6 * 3600,
            "7 days" => 7 * 86400,
            _ => 24 * 3600,
        }
    }
    /// Size row logos are decoded at for the current view.
    fn logo_size(&self) -> u32 {
        if self.grid {
            crate::images::TILE
        } else {
            crate::images::ROW
        }
    }
    /// Switch between rows and tiles: remember it and decode the logos again at the
    /// size of the new view.
    fn set_view(&mut self, cx: &Ctx, grid: bool) {
        cx.app.global::<crate::MusicDialogState>().set_grid(grid);
        if grid == self.grid {
            return;
        }
        self.grid = grid;
        cx.set("view", if grid { "Grid" } else { "List" });
        self.logos.clear();
        self.load_row_logos();
    }
    fn is_picker(&self) -> bool {
        self.all.first().is_some_and(|s| s.source == PICK)
    }
    fn show(&mut self, cx: &Ctx) {
        self.selected = None;
        self.show_rows(cx);
        self.load_row_logos();
        self.show_chips(cx);
        self.buttons(cx);
    }
    /// Rebuild `list` from `all`: group chip, live filter, favorites first.
    fn apply_view(&mut self, cx: &Ctx) {
        let mut v = by_group(&self.all, &self.chip);
        if live_filter(self.mode) {
            v = filter(&v, &self.filtered_by);
        }
        if !self.is_picker() && self.mode != SAVED && self.mode != RECENT {
            pin_favorites(&mut v, &self.favs);
        }
        self.list = v;
        self.show(cx);
    }
    fn show_chips(&mut self, cx: &Ctx) {
        let groups = if self.is_picker() {
            Vec::new()
        } else {
            top_groups(&self.all, MAX_CHIPS)
        };
        let chips: Vec<SharedString> = if groups.len() >= 2 {
            self.category_chips = false;
            std::iter::once("All".to_string())
                .chain(groups)
                .map(Into::into)
                .collect()
        } else if self.mode == TV_CATEGORY {
            self.category_chips = true;
            CATEGORY_CHIPS.iter().map(|c| c.0.into()).collect()
        } else {
            self.category_chips = false;
            Vec::new()
        };
        cx.app
            .global::<crate::MusicDialogState>()
            .set_chips(ModelRc::new(VecModel::from(chips)));
        cx.app.global::<crate::MusicDialogState>().set_chip(
            if self.chip.is_empty() && !self.category_chips {
                "All".into()
            } else {
                self.chip.as_str().into()
            },
        );
    }
    /// Start fetching logos of the first rows that are not cached yet.
    fn load_row_logos(&mut self) {
        let want: Vec<String> = self
            .list
            .iter()
            .take(80)
            .map(|s| s.logo.clone())
            .filter(|l| l.starts_with("http") && !self.logos.contains_key(l))
            .collect();
        if want.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        self.row_logo_rx = Some(rx);
        std::thread::spawn(move || {
            for l in want {
                if tx.send((l.clone(), fetch_logo(&l))).is_err() {
                    break;
                }
            }
        });
    }
    fn guide_for(&self, s: &Stream) -> Option<&Vec<super::epg::Prog>> {
        super::epg::lookup(&self.epg, &s.tvg_id)
    }
    fn show_rows(&self, cx: &Ctx) {
        let logos: Vec<slint::Image> = self
            .list
            .iter()
            .map(|s| self.logos.get(&s.logo).cloned().unwrap_or_default())
            .collect();
        cx.app
            .global::<crate::MusicDialogState>()
            .set_logos(ModelRc::new(VecModel::from(logos)));
        let now = super::epg::now();
        let items: Vec<Item> = self
            .list
            .iter()
            .map(|s| {
                let star = if s.source != PICK && self.favs.contains(&s.url) {
                    "★ "
                } else {
                    ""
                };
                let mut label = s.label();
                if let Some(v) = self.guide_for(s) {
                    if let Some(p) = super::epg::now_next(v, now).0 {
                        label = format!("▶ {}  ·  {label}", p.title);
                    }
                }
                Item {
                    key: format!("{star}{}", display_name(&s.name)).into(),
                    label: label.into(),
                    selected: false,
                }
            })
            .collect();
        cx.app
            .global::<crate::MusicDialogState>()
            .set_results(ModelRc::new(VecModel::from(items)));
    }
    /// Secondary button and the extra actions for the current selection.
    fn buttons(&self, cx: &Ctx) {
        let sel = self.selected.and_then(|i| self.list.get(i));
        let picker = sel.is_some_and(|s| s.source == PICK);
        let t = match (self.mode, sel) {
            (SAVED | RECENT, _) => "Remove",
            (MY_PL, Some(_)) if picker => "Remove",
            (M3U, None) => "Choose file…",
            (_, Some(s)) if !picker && self.favs.contains(&s.url) => "Unfavorite",
            _ => "Favorite",
        };
        cx.app
            .global::<crate::MusicDialogState>()
            .set_secondary(t.into());
        let mut acts: Vec<SharedString> = Vec::new();
        if sel.is_some() && !picker {
            acts.push("Add to queue".into());
            acts.push("Mini player".into());
        }
        if self.mode == MY_PL && picker {
            acts.push("Refresh".into());
        }
        if self.mode == M3U && !self.loaded_src.is_empty() && !self.all.is_empty() {
            acts.push("Add to My playlists".into());
        }
        cx.app
            .global::<crate::MusicDialogState>()
            .set_actions(ModelRc::new(VecModel::from(acts)));
    }
    fn mode(cx: &Ctx) -> usize {
        let s = cx.app.global::<crate::MusicDialogState>().get_source();
        SOURCES.iter().position(|x| *x == s.as_str()).unwrap_or(0)
    }
    fn local(&mut self, cx: &Ctx, mode: usize) {
        self.mode = mode;
        self.reload_favs();
        self.chip.clear();
        self.loaded_src.clear();
        self.all = load_list(if mode == SAVED {
            "saved.json"
        } else {
            "recent.json"
        });
        self.apply_view(cx);
        self.want_epg(cx);
        cx.app
            .global::<crate::MusicDialogState>()
            .set_detail("".into());
        let what = if mode == SAVED { "favorite" } else { "recent" };
        Self::ui(
            cx,
            &if self.list.is_empty() {
                format!("No {what} streams yet")
            } else {
                format!("{} {what} streams", self.list.len())
            },
            false,
        );
    }
    /// Run `job` in the background; its result fills the list in `tick`.
    fn spawn(&mut self, job: impl FnOnce() -> Result<Vec<Stream>, String> + Send + 'static) {
        self.task = crate::util::Task::spawn(job);
    }
    fn clear(&mut self, cx: &Ctx) {
        self.all.clear();
        self.list.clear();
        self.chip.clear();
        self.show(cx);
        cx.app
            .global::<crate::MusicDialogState>()
            .set_detail("".into());
    }
    /// Request guides for the channels shown (playlist `url-tvg` + the EPG setting).
    fn want_epg(&mut self, cx: &Ctx) {
        if !self.all.iter().any(|s| !s.tvg_id.is_empty()) {
            return;
        }
        let mut urls: Vec<String> = self
            .all
            .iter()
            .map(|s| s.epg.clone())
            .chain(cx.get("epg").split_whitespace().map(String::from))
            .filter(|u| u.starts_with("http") && !self.epg_asked.contains(u))
            .collect();
        urls.sort();
        urls.dedup();
        if urls.is_empty() || self.epg_rx.is_some() {
            return;
        }
        self.epg_asked.extend(urls.iter().cloned());
        let (tx, rx) = channel();
        self.epg_rx = Some(rx);
        std::thread::spawn(move || {
            for u in urls {
                if tx.send(super::epg::load(&u)).is_err() {
                    break;
                }
            }
        });
    }
    /// Play `s`: 0 = now, 1 = add to queue, 2 = now in the mini player.
    fn play(&mut self, cx: &Ctx, s: &Stream, how: u8) {
        crate::loading::set_name(&s.url, &display_name(&s.name));
        crate::store::recent_hint(crate::store::RecentEntry {
            target: s.url.clone(),
            kind: crate::store::RecentKind::Stream,
            title: display_name(&s.name),
            sub: s.group.clone(),
            image: s.logo.clone(),
        });
        if how == 1 {
            cx.player.enqueue(&s.url);
            cx.toast(&format!("Added {} to the queue", display_name(&s.name)));
            Self::ui(cx, &format!("Queued {}", s.name), false);
            return;
        }
        cx.player.cmd("loadfile", &[&s.url]);
        self.playing = Some(s.clone());
        self.known.0.clear();
        if s.source == "radio-browser.info" {
            radio_click(&s.stationuuid);
        }
        let mut r = load_list("recent.json");
        push_front(&mut r, s, MAX_RECENT);
        save_list("recent.json", &r);
        cx.toast(&format!("Playing {}", s.name));
        Self::ui(cx, &format!("Playing {}", s.name), false);
        if how == 2 {
            cx.app.global::<crate::Overlays>().set_md_open(false);
            if !cx.app.get_mini() {
                cx.app.global::<crate::Playback>().invoke_toggle_mini();
            }
        }
    }
    fn open_playlist(&mut self, cx: &Ctx, name: &str, key: String) {
        self.clear(cx);
        cx.app
            .global::<crate::MusicDialogState>()
            .set_query("".into());
        self.filtered_by.clear();
        Self::ui(cx, &format!("Loading {name}…"), true);
        self.note = Some(format!(
            "{name}: {{n}} streams. Type to filter, Go to go back."
        ));
        let ttl = Self::ttl(cx);
        self.spawn(move || {
            if let Some(src) = key.strip_prefix(PL) {
                fetch_playlist(src, ttl, false)
            } else if key.starts_with("http") {
                iptv_playlist(&key)
            } else {
                genre_stations(&key)
            }
        });
    }
}

/// Station of `url` in the saved and recent lists.
fn known_station(url: &str) -> Option<Stream> {
    load_list("saved.json")
        .into_iter()
        .chain(load_list("recent.json"))
        .find(|s| s.url == url)
}

/// Largest logo side kept in the cache; logos are shown at 40-96 px.
pub(crate) const LOGO_MAX: u32 = 128;

fn logo_dir() -> PathBuf {
    crate::store::cache_dir().join("stream-logos")
}

/// What downloaded logo bytes are, from their first bytes (servers often send HTML error
/// pages or icons with an image content type).
#[derive(Debug, PartialEq)]
pub(crate) enum LogoKind {
    /// PNG or JPEG: decoded, shrunk to [`LOGO_MAX`] and stored as PNG.
    Raster,
    Svg,
    /// Anything else (HTML, ICO, GIF, WebP, ...): not stored.
    Other,
}

pub(crate) fn sniff_logo(b: &[u8]) -> LogoKind {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") || b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return LogoKind::Raster;
    }
    let head = String::from_utf8_lossy(&b[..b.len().min(1024)]).to_ascii_lowercase();
    if head.contains("<svg") {
        LogoKind::Svg
    } else {
        LogoKind::Other
    }
}

/// Write logo bytes next to `base` (the extension-less cache name) with the extension of
/// their real format; raster logos are shrunk to [`LOGO_MAX`]. `None` = not an image.
fn store_logo(base: &Path, b: &[u8]) -> Option<PathBuf> {
    match sniff_logo(b) {
        LogoKind::Raster => {
            let img = image::load_from_memory(b).ok()?;
            let img = if img.width() > LOGO_MAX || img.height() > LOGO_MAX {
                img.thumbnail(LOGO_MAX, LOGO_MAX)
            } else {
                img
            };
            let out = base.with_extension("png");
            let tmp = base.with_extension("tmp");
            img.to_rgba8()
                .save_with_format(&tmp, image::ImageFormat::Png)
                .ok()?;
            std::fs::rename(&tmp, &out).ok()?;
            Some(out)
        }
        LogoKind::Svg => {
            let out = base.with_extension("svg");
            std::fs::write(&out, b).ok()?;
            Some(out)
        }
        LogoKind::Other => None,
    }
}

/// Station logo already in the cache (no download, no decoding).
pub(crate) fn cached_logo(url: &str) -> Option<PathBuf> {
    cached_with_ext(&logo_dir(), url, &["png", "svg"])
}

/// Older versions saved logos as they came, without an extension, so Slint could not tell
/// their format and every one failed to load. Convert those once (shrinking raster logos)
/// and delete the ones that are not images. Returns how many logos became usable.
pub(crate) fn migrate_logo_cache() -> usize {
    let Ok(rd) = std::fs::read_dir(logo_dir()) else {
        return 0;
    };
    let mut n = 0;
    for p in rd.flatten().map(|e| e.path()) {
        if p.extension().is_some() || !p.is_file() {
            continue;
        }
        if let Ok(b) = std::fs::read(&p) {
            n += usize::from(store_logo(&p, &b).is_some());
        }
        let _ = std::fs::remove_file(&p);
    }
    n
}

thread_local! {
    /// Logos decoded by the image cache: (now-playing card?, key, image), drained in `tick`.
    static READY: std::cell::RefCell<Vec<(bool, String, slint::Image)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The logo at `p` from the image cache, or `None` while it is decoded (it then arrives
/// through [`READY`] under `key`).
fn logo_image(p: &Path, size: u32, now_playing: bool, key: String) -> Option<slint::Image> {
    crate::images::get(p, size, move |i| {
        READY.with(|r| r.borrow_mut().push((now_playing, key, i)))
    })
}

/// Download a station logo into the cache (once); `None` when it cannot be fetched or
/// is not an image.
fn fetch_logo(url: &str) -> Option<PathBuf> {
    if let Some(p) = cached_logo(url) {
        return Some(p);
    }
    let dir = logo_dir();
    let base = dir.join(hash_name(url));
    let miss = hashed_file(&dir, url, |h| format!("{h}.none"));
    // Broken logo URLs are remembered for a week instead of being retried every time.
    if std::fs::metadata(&miss)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e.as_secs() < 7 * 86400)
    {
        return None;
    }
    std::fs::create_dir_all(&dir).ok()?;
    let stored = match web::get_bytes(url, 2 * 1024 * 1024) {
        Ok(Some(b)) if !b.is_empty() => store_logo(&base, &b),
        _ => None,
    };
    if stored.is_none() {
        let _ = std::fs::write(&miss, b"");
    }
    stored
}

/// Short stable file name for a URL: FNV-1a 64 of its bytes, 16 hex digits. Used for
/// every URL-keyed cache file (station logos, guides, covers, web thumbnails, playlist
/// copies), so it must never change between builds.
pub fn hash_name(url: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The name Vivi 0.12 and older gave `url`: `DefaultHasher` (SipHash-1-3 with zero keys)
/// of `str::hash`, i.e. the bytes and a `0xff` terminator. The standard library may change
/// that hasher in any release, so it is spelled out here to keep finding old cache files.
pub fn legacy_hash_name(url: &str) -> String {
    let mut m = url.as_bytes().to_vec();
    m.push(0xff);
    format!("{:016x}", siphash13(&m))
}

/// SipHash-1-3 with zero keys, as `std::hash::SipHasher13::new()` (length in the last byte).
fn siphash13(m: &[u8]) -> u64 {
    let mut v = [
        0x736f_6d65_7073_6575u64,
        0x646f_7261_6e64_6f6du64,
        0x6c79_6765_6e65_7261u64,
        0x7465_6462_7974_6573u64,
    ];
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13) ^ v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16) ^ v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21) ^ v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17) ^ v[2];
        v[2] = v[2].rotate_left(32);
    }
    let (words, rest) = m.as_chunks::<8>();
    for c in words {
        let w = u64::from_le_bytes(*c);
        v[3] ^= w;
        round(&mut v);
        v[0] ^= w;
    }
    let mut last = [0u8; 8];
    last[..rest.len()].copy_from_slice(rest);
    let w = u64::from_le_bytes(last) | ((m.len() as u64 & 0xff) << 56);
    v[3] ^= w;
    round(&mut v);
    v[0] ^= w;
    v[2] ^= 0xff;
    for _ in 0..3 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

/// `dir/name(hash_name(url))`. When only the file of the old name
/// ([`legacy_hash_name`]) exists, it is renamed first, so caches written by Vivi 0.12 and
/// older (logos, guides, covers, thumbnails, last good playlist copies) keep working.
pub fn hashed_file(dir: &Path, url: &str, name: impl Fn(&str) -> String) -> PathBuf {
    let new = dir.join(name(&hash_name(url)));
    if !new.exists() {
        let old = dir.join(name(&legacy_hash_name(url)));
        if old.is_file() {
            let _ = std::fs::rename(&old, &new);
        }
    }
    new
}

/// The first of `dir/<hash>.<ext>` for `exts` that exists (old names migrated).
pub fn cached_with_ext(dir: &Path, url: &str, exts: &[&str]) -> Option<PathBuf> {
    exts.iter()
        .map(|e| hashed_file(dir, url, |h| format!("{h}.{e}")))
        .find(|p| p.is_file())
}

/// Keep the logo cache under `max` bytes (oldest files go first).
fn prune_logos(max: u64) {
    let dir = crate::store::cache_dir().join("stream-logos");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut v: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = v.iter().map(|x| x.1).sum();
    v.sort();
    for (_, n, p) in v {
        if total <= max {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= n;
        }
    }
}

/// Whether an audio-only URL plays like a radio station (the station card) rather than a
/// song file on a server (Free Music, a podcast episode): a station of ours, a stream
/// title (ICY), or no known length.
pub(crate) fn looks_like_station(known: bool, get: impl Fn(&str) -> Option<String>) -> bool {
    if known {
        return true;
    }
    let has = |k: &str| get(k).is_some_and(|v| !v.trim().is_empty());
    if has("metadata/by-key/icy-name") || has("metadata/by-key/icy-title") {
        return true;
    }
    !get("duration")
        .and_then(|d| d.parse::<f64>().ok())
        .is_some_and(|d| d > 0.0)
}

/// The side drawer panel: the stream playing now (station, ICY title, codec, buffer) or
/// a hint, and how many favorites / recent streams there are (the manifest description
/// used to be shown there).
pub fn drawer_text(
    station: Option<&Stream>,
    stream: bool,
    info: &str,
    favorites: usize,
    recent: usize,
) -> String {
    let mut s = String::new();
    if let Some(st) = station {
        s += &format!("{}\n", display_name(&st.name));
    }
    if stream {
        s += info;
    } else {
        s += "No network stream is playing. Browse streams to pick a radio station or TV channel.";
    }
    let n = |k: usize, one: &str| format!("{k} {one}{}", if k == 1 { "" } else { "s" });
    s += &format!(
        "\n\n{} · {}",
        n(favorites, "favorite"),
        n(recent, "recent stream")
    );
    s
}

impl NetworkStreams {
    /// [`looks_like_station`] for `path` (cached per URL once mpv knows its length).
    fn is_station(&mut self, path: &str, get: impl Fn(&str) -> Option<String>) -> bool {
        if self.known.0 != path {
            let known = self.playing.as_ref().is_some_and(|s| s.url == path)
                || self.list.iter().any(|s| s.url == path)
                || known_station(path).is_some();
            self.known = (path.to_string(), known);
        }
        looks_like_station(self.known.1, get)
    }
    /// Fill the now-playing card while an audio-only network stream plays.
    fn now_playing(&mut self, cx: &Ctx) {
        let a = cx.app;
        let path = cx.player.get("path").unwrap_or_default();
        if super::host::has_meta(&path) {
            return; // a package described this URL: the host fills the card
        }
        let show = a.global::<crate::Playback>().get_has_media()
            && a.global::<crate::Viz>().get_audio()
            && path.contains("://")
            && self.is_station(&path, |k| cx.player.get(k));
        if !show {
            if a.global::<crate::NowPlaying>().get_show() {
                a.global::<crate::NowPlaying>().set_show(false);
            }
            return;
        }
        if path != self.np_url {
            self.np_url = path.clone();
            let st = self
                .playing
                .clone()
                .filter(|s| s.url == path)
                .or_else(|| self.list.iter().find(|s| s.url == path).cloned())
                .or_else(|| known_station(&path));
            a.global::<crate::NowPlaying>().set_has_logo(false);
            a.global::<crate::NowPlaying>().set_music(false);
            self.logo_task.cancel();
            let name = st
                .as_ref()
                .map(|s| display_name(&s.name))
                .unwrap_or_default();
            a.global::<crate::NowPlaying>().set_station(name.into());
            if let Some(logo) = st.map(|s| s.logo).filter(|l| l.starts_with("http")) {
                let u = path.clone();
                self.logo_task = crate::util::Task::spawn(move || (u, fetch_logo(&logo)));
            }
        }
        let g = |k: &str| cx.player.get(k).filter(|v| !v.trim().is_empty());
        if a.global::<crate::NowPlaying>().get_station().is_empty() {
            let n = g("metadata/by-key/icy-name")
                .or_else(|| g("media-title"))
                .unwrap_or_else(|| display_name(path.rsplit('/').next().unwrap_or(&path)));
            a.global::<crate::NowPlaying>().set_station(n.into());
        }
        let track = g("metadata/by-key/icy-title").unwrap_or_default();
        let mut info = Vec::new();
        if let Some(c) = g("audio-codec-name") {
            info.push(c.to_uppercase());
        }
        if let Some(b) = g("audio-bitrate")
            .and_then(|b| b.parse::<f64>().ok())
            .filter(|b| *b > 0.0)
        {
            info.push(format!("{:.0} kbps", b / 1000.0));
        }
        if let Some(r) = g("audio-params/samplerate").and_then(|r| r.parse::<f64>().ok()) {
            info.push(format!("{:.1} kHz", r / 1000.0));
        }
        a.global::<crate::NowPlaying>().set_track(track.into());
        a.global::<crate::NowPlaying>()
            .set_info(info.join(" · ").into());
        if !a.global::<crate::NowPlaying>().get_show() {
            a.global::<crate::NowPlaying>().set_show(true);
        }
    }
}

impl Plugin for NetworkStreams {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Network Streams",
            "Internet radio (radio-browser.info, SomaFM), iptv-org TV by country and category, your own M3U/IPTV playlists that refresh themselves, favorites, Search all (TV + radio), group and category chips, channel logos as a list or a grid, an XMLTV program guide (now / next) and stream info.",
            "streams",
        )
        .source("open")
        .contribute(contrib::PALETTE_COMMAND, "Browse network streams", "open")
        .contribute(contrib::CONTEXT_PLAYER, "Stream info", "info")
        .contribute(contrib::DRAWER_PANEL, "Network Streams", "drawer")
        .contribute(contrib::HOME_WIDGET, "Radio stations", "open")
        .setting(SettingSpec::action(
            "open",
            "Browse streams",
            "Opens the streams browser.",
            "Browse…",
        ))
        .setting(SettingSpec::action(
            "info",
            "Stream info",
            "Shows the now-playing title, codec, bitrate and buffer of the current stream.",
            "Show",
        ))
        .setting(SettingSpec::text(
            "epg",
            "Program guide (XMLTV)",
            "Optional XMLTV guide URL(s) (.xml or .xml.gz) for channels with a tvg-id, in addition to the playlist's own url-tvg. Only the next 12 hours are kept.",
            "https://example.com/guide.xml.gz",
            "",
        ))
        .setting(SettingSpec::choice(
            "view",
            "Browser view",
            "Show stations as a list or as a grid of logos. The buttons in the browser's header switch it too.",
            &["List", "Grid"],
        ))
        .setting(SettingSpec::choice(
            "refresh",
            "Refresh my playlists",
            "How often saved M3U playlist URLs are downloaded again. A broken link keeps the last good copy.",
            &["24 hours", "6 hours", "7 days"],
        ))
    }
    fn disable(&mut self, cx: &Ctx) {
        self.task.cancel();
        cx.app.global::<crate::NowPlaying>().set_show(false);
        if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            cx.app.global::<crate::Overlays>().set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let idx = || {
            args.first()
                .and_then(|a| a.parse::<i64>().ok())
                .filter(|i| *i >= 0)
                .map(|i| i as usize)
        };
        match name {
            "drawer" => {
                let path = cx.player.get("path").unwrap_or_default();
                let station = path.contains("://").then(|| known_station(&path)).flatten();
                let text = drawer_text(
                    station.as_ref(),
                    path.contains("://"),
                    &now_info(|p| cx.player.get(p)),
                    load_list("saved.json").len(),
                    load_list("recent.json").len(),
                );
                cx.app.global::<crate::Drawer>().set_info(text.into());
            }
            "open" => {
                let a = cx.app;
                a.global::<crate::Overlays>().set_settings_open(false);
                // From the side drawer: the browser replaces it.
                a.global::<crate::Overlays>().set_drawer_open(false);
                a.global::<crate::MusicDialogState>().set_owner(ID.into());
                a.global::<crate::MusicDialogState>()
                    .set_caps(crate::MdCaps {
                        logos: true,
                        chips: true,
                        rich: false,
                        grid: true,
                    });
                self.grid = cx.get("view") == "Grid";
                a.global::<crate::MusicDialogState>().set_grid(self.grid);
                a.global::<crate::MusicDialogState>()
                    .set_title("Network Streams".into());
                a.global::<crate::MusicDialogState>()
                    .set_placeholder(placeholder(Self::mode(cx)).into());
                self.last_source.clear(); // the next tick sets placeholder and button
                a.global::<crate::MusicDialogState>()
                    .set_search_label("Go".into());
                a.global::<crate::MusicDialogState>()
                    .set_primary("Play".into());
                let src: Vec<SharedString> = SOURCES.iter().map(|s| (*s).into()).collect();
                a.global::<crate::MusicDialogState>()
                    .set_sources(ModelRc::new(VecModel::from(src)));
                if !SOURCES.contains(&a.global::<crate::MusicDialogState>().get_source().as_str()) {
                    a.global::<crate::MusicDialogState>()
                        .set_source(SOURCES[0].into());
                }
                a.global::<crate::Overlays>().set_md_open(true);
                std::thread::spawn(|| prune_logos(64 * 1024 * 1024));
                self.local(cx, SAVED);
                if self.list.is_empty() {
                    Self::ui(
                        cx,
                        "Pick a source, type and press Go. Favorites show here.",
                        false,
                    );
                }
            }
            "view" => {
                let grid = args.first().is_some_and(|a| a == "grid");
                self.set_view(cx, grid);
            }
            "search" => {
                let mode = Self::mode(cx);
                if mode == SAVED || mode == RECENT {
                    self.local(cx, mode);
                    return;
                }
                self.reload_favs();
                let q = cx
                    .app
                    .global::<crate::MusicDialogState>()
                    .get_query()
                    .trim()
                    .to_string();
                self.loaded_src.clear();
                if live_filter(mode) {
                    // Go = (re)load the picker; typing filters it.
                    self.filtered_by = q.clone();
                    self.mode = mode;
                    self.clear(cx);
                    Self::ui(
                        cx,
                        if mode == MY_PL {
                            "Refreshing your playlists…"
                        } else {
                            "Loading…"
                        },
                        true,
                    );
                    let ttl = Self::ttl(cx);
                    if mode == MY_PL && playlists().is_empty() {
                        Self::ui(
                            cx,
                            "No playlists yet. Load an M3U URL in the M3U / IPTV source, then Add to My playlists.",
                            false,
                        );
                        return;
                    }
                    self.spawn(move || match mode {
                        TV_COUNTRY => iptv_index(true),
                        TV_CATEGORY => iptv_index(false),
                        GENRES => genre_list(),
                        MY_PL => my_playlists(ttl),
                        _ => somafm(),
                    });
                    return;
                }
                if q.is_empty() && mode != 1 {
                    Self::ui(cx, "Type something first", false);
                    return;
                }
                self.note = None;
                let tv_search = mode == M3U && !is_playlist_source(&q);
                if tv_search {
                    self.note = Some(format!(
                        "TV channels named “{q}” from iptv-org. Paste a URL or use Choose file… to import a playlist."
                    ));
                } else if mode == M3U {
                    self.loaded_src = q.clone();
                }
                self.mode = mode;
                self.clear(cx);
                Self::ui(
                    cx,
                    if tv_search {
                        "Searching iptv-org channels…"
                    } else if mode == M3U {
                        "Loading playlist…"
                    } else if mode == ALL {
                        "Searching TV and radio…"
                    } else {
                        "Searching radio-browser.info…"
                    },
                    true,
                );
                self.spawn(move || match mode {
                    M3U if tv_search => iptv_search(&q),
                    M3U => load_m3u(&q),
                    ALL => search_all(&q),
                    0 => radio_any(&q),
                    _ => radio_search(mode, &q),
                });
            }
            "chip" => {
                let c = args.first().cloned().unwrap_or_default();
                if self.category_chips {
                    if let Some((label, id)) = CATEGORY_CHIPS.iter().find(|x| x.0 == c) {
                        self.chip.clear();
                        self.open_playlist(cx, label, format!("{IPTV}/categories/{id}.m3u"));
                    }
                    return;
                }
                self.chip = if c == "All" { String::new() } else { c };
                self.apply_view(cx);
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_detail("".into());
                Self::ui(
                    cx,
                    &format!("{} of {}", self.list.len(), self.all.len()),
                    false,
                );
            }
            "select" => {
                self.selected = idx().filter(|i| *i < self.list.len());
                self.buttons(cx);
                if let Some(s) = self.selected.and_then(|i| self.list.get(i)) {
                    let mut d = s.detail();
                    if !s.tvg_id.is_empty() {
                        d += &format!("\nGuide id: {}", s.tvg_id);
                    }
                    if let Some(v) = self.guide_for(s) {
                        let l = super::epg::describe(v, super::epg::now());
                        if !l.is_empty() {
                            d += &format!("\n\n— Program guide —\n{}", l.join("\n"));
                        }
                    }
                    if cx.player.get("path").as_deref() == Some(s.url.as_str()) {
                        d += &format!("\n\n— Playing —\n{}", now_info(|p| cx.player.get(p)));
                    }
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_detail(d.into());
                }
            }
            "primary" => {
                let Some(s) = idx().and_then(|i| self.list.get(i)).cloned() else {
                    Self::ui(cx, "Select a stream first", false);
                    return;
                };
                if s.source == PICK {
                    self.open_playlist(cx, &s.name, s.url.clone());
                    return;
                }
                self.play(cx, &s, 0);
            }
            n if n.starts_with("act:") => {
                let sel = idx().and_then(|i| self.list.get(i)).cloned();
                match &n[4..] {
                    "Add to queue" | "Mini player" => {
                        let Some(s) = sel.filter(|s| s.source != PICK) else {
                            Self::ui(cx, "Select a stream first", false);
                            return;
                        };
                        self.play(cx, &s, if n.ends_with("queue") { 1 } else { 2 });
                    }
                    "Refresh" => {
                        let Some(src) = sel.and_then(|s| s.url.strip_prefix(PL).map(String::from))
                        else {
                            return;
                        };
                        Self::ui(cx, "Refreshing…", true);
                        self.note = Some("Refreshed. {n} playlists.".into());
                        self.clear(cx);
                        let ttl = Self::ttl(cx);
                        self.spawn(move || {
                            let r = fetch_playlist(&src, ttl, true);
                            let picks = playlist_picks(&playlists());
                            r.map(|_| picks)
                        });
                    }
                    "Add to My playlists" => {
                        let src = self.loaded_src.clone();
                        let mut v = playlists();
                        if v.iter().any(|p| p.url == src) {
                            Self::ui(cx, "Already in My playlists", false);
                            return;
                        }
                        v.push(Playlist {
                            name: playlist_name(&src),
                            url: src.clone(),
                            added: now_secs(),
                            ..Default::default()
                        });
                        save_playlists(&v);
                        let ttl = Self::ttl(cx);
                        std::thread::spawn(move || {
                            let _ = fetch_playlist(&src, ttl, true);
                        });
                        Self::ui(
                            cx,
                            "Added to My playlists. It refreshes itself; see the My playlists source.",
                            false,
                        );
                        self.buttons(cx);
                    }
                    _ => {}
                }
            }
            "secondary"
                if cx
                    .app
                    .global::<crate::MusicDialogState>()
                    .get_secondary()
                    .as_str()
                    == "Choose file…" =>
            {
                let Some(f) = rfd::FileDialog::new()
                    .set_title("Import a playlist")
                    .add_filter("Playlists", &["m3u", "m3u8"])
                    .pick_file()
                else {
                    return;
                };
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_query(f.to_string_lossy().as_ref().into());
                self.command(cx, "search", &[]);
            }
            "secondary" => {
                let Some(i) = idx().filter(|i| *i < self.list.len()) else {
                    Self::ui(cx, "Select a stream first", false);
                    return;
                };
                let s = self.list[i].clone();
                if self.mode == SAVED || self.mode == RECENT {
                    let file = if self.mode == SAVED {
                        "saved.json"
                    } else {
                        "recent.json"
                    };
                    self.all.retain(|o| o.url != s.url);
                    save_list(file, &self.all);
                    self.reload_favs();
                    self.apply_view(cx);
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_detail("".into());
                    Self::ui(cx, &format!("Removed {}", s.name), false);
                } else if s.source == PICK {
                    if let Some(src) = s.url.strip_prefix(PL) {
                        let mut v = playlists();
                        v.retain(|p| p.url != src);
                        save_playlists(&v);
                        let _ = std::fs::remove_file(playlist_cache(src));
                        self.all.retain(|o| o.url != s.url);
                        self.apply_view(cx);
                        Self::ui(cx, &format!("Removed playlist {}", s.name), false);
                    } else {
                        Self::ui(cx, "Open it first, then favorite a stream", false);
                    }
                } else {
                    let mut v = load_list("saved.json");
                    let was = v.iter().any(|o| o.url == s.url);
                    v.retain(|o| o.url != s.url);
                    if !was {
                        v.push(s.clone());
                    }
                    save_list("saved.json", &v);
                    self.reload_favs();
                    self.show_rows(cx);
                    self.selected = Some(i);
                    self.buttons(cx);
                    Self::ui(
                        cx,
                        &if was {
                            format!("Removed {} from favorites", s.name)
                        } else {
                            format!("★ {} added to favorites", s.name)
                        },
                        false,
                    );
                }
            }
            "info" => cx.toast(&now_info(|p| cx.player.get(p)).replace('\n', " · ")),
            "close" => self.task.cancel(),
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        self.poll = self.poll.wrapping_add(1);
        if cx.app.global::<crate::Overlays>().get_md_open()
            && cx.app.global::<crate::MusicDialogState>().get_owner() == ID
        {
            let src = cx.app.global::<crate::MusicDialogState>().get_source();
            if src.as_str() != self.last_source {
                self.last_source = src.to_string();
                let m = Self::mode(cx);
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_placeholder(placeholder(m).into());
                if self.mode != m {
                    cx.app.global::<crate::MusicDialogState>().set_secondary(
                        if m == M3U {
                            "Choose file…"
                        } else {
                            "Favorite"
                        }
                        .into(),
                    );
                }
            }
        }
        if live_filter(self.mode)
            && !self.all.is_empty()
            && cx.app.global::<crate::Overlays>().get_md_open()
        {
            let q = cx
                .app
                .global::<crate::MusicDialogState>()
                .get_query()
                .to_string();
            if q != self.filtered_by {
                self.filtered_by = q.clone();
                self.apply_view(cx);
                Self::ui(
                    cx,
                    &format!("{} of {}", self.list.len(), self.all.len()),
                    false,
                );
            }
        }
        if let Some(rx) = &self.row_logo_rx {
            let mut got = false;
            while let Ok((url, p)) = rx.try_recv() {
                let size = self.logo_size();
                let img = p.and_then(|p| logo_image(&p, size, false, url.clone()));
                self.logos.insert(url, img.unwrap_or_default());
                got = true;
            }
            self.logos_dirty |= got;
        }
        for (now_playing, key, img) in READY.with(|r| std::mem::take(&mut *r.borrow_mut())) {
            if !now_playing {
                self.logos.insert(key, img);
                self.logos_dirty = true;
            } else if key == self.np_url {
                cx.app.global::<crate::NowPlaying>().set_logo(img);
                cx.app.global::<crate::NowPlaying>().set_has_logo(true);
            }
        }
        if let Some(rx) = &self.epg_rx {
            loop {
                match rx.try_recv() {
                    Ok(Ok(g)) => {
                        self.epg.extend(g);
                        self.logos_dirty = true;
                    }
                    Ok(Err(e)) => eprintln!("vivi: EPG: {e}"),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(_) => {
                        self.epg_rx = None;
                        break;
                    }
                }
            }
        }
        if self.logos_dirty && self.poll.is_multiple_of(10) {
            self.logos_dirty = false;
            self.show_rows(cx);
        }
        if self.poll.is_multiple_of(5) {
            self.now_playing(cx);
        }
        if let Some((url, path)) = self.logo_task.poll() {
            if url == self.np_url {
                let key = url.clone();
                match path.and_then(|p| logo_image(&p, crate::images::TILE, true, key)) {
                    Some(img) => {
                        cx.app.global::<crate::NowPlaying>().set_logo(img);
                        cx.app.global::<crate::NowPlaying>().set_has_logo(true);
                    }
                    // Not a picture, or still decoding (shown when READY has it).
                    None => cx.app.global::<crate::NowPlaying>().set_has_logo(false),
                }
            }
        }
        let Some(r) = self.task.poll() else { return };
        match r {
            Ok(v) => {
                let n = v.len();
                self.all = v;
                self.chip.clear();
                if live_filter(self.mode) {
                    self.filtered_by = cx
                        .app
                        .global::<crate::MusicDialogState>()
                        .get_query()
                        .to_string();
                }
                self.apply_view(cx);
                self.want_epg(cx);
                Self::ui(
                    cx,
                    &if let Some(note) = self.note.take() {
                        note.replace("{n}", &n.to_string())
                    } else if n == 0 {
                        "Nothing found".to_string()
                    } else {
                        format!("{n} streams. Double-click to play.")
                    },
                    false,
                );
            }
            Err(e) => {
                self.note = None;
                Self::ui(cx, &e, false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logos_are_sniffed_shrunk_and_migrated() {
        assert_eq!(
            sniff_logo(b"<html><body>404</body></html>"),
            LogoKind::Other
        );
        assert_eq!(
            sniff_logo(b"<?xml version=\"1.0\"?><svg xmlns=\"\"/>"),
            LogoKind::Svg
        );
        assert_eq!(sniff_logo(&[0xFF, 0xD8, 0xFF, 0xE0]), LogoKind::Raster);
        let dir = std::env::temp_dir().join(format!("vivi-logos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(600, 300, image::Rgba([200, 10, 10, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(sniff_logo(&png), LogoKind::Raster);
        let out = store_logo(&dir.join("a"), &png).unwrap();
        assert_eq!(out, dir.join("a.png"));
        assert_eq!(image::image_dimensions(&out).unwrap(), (128, 64));
        assert!(store_logo(&dir.join("b"), b"<html>").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn m3u_iptv() {
        let t = "#EXTM3U\n#EXTINF:-1 tvg-id=\"a\" tvg-logo=\"http://l/a.png\" group-title=\"News, World\",Channel A\nhttp://s/a.m3u8\n#EXTVLCOPT:http-user-agent=x\n#EXTINF:-1,\nhttps://s/b/radio.mp3\nrelative.mp3\nhttp://s/c\n";
        let v = parse_m3u(t);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "Channel A");
        assert_eq!(v[0].logo, "http://l/a.png");
        assert_eq!(v[0].group, "News, World");
        assert_eq!(v[1].name, "radio.mp3");
        assert_eq!(v[2].url, "http://s/c");
    }

    #[test]
    fn names_and_sources() {
        assert_eq!(
            pct_decode("01.%20J.%20S.%20BACH%20-%20Air.mp3"),
            "01. J. S. BACH - Air.mp3"
        );
        assert_eq!(pct_decode("100%25 %zz %"), "100% %zz %");
        let v = parse_m3u("#EXTM3U\nhttp://h/My%20Song.mp3\n");
        assert_eq!(v[0].name, "My Song.mp3");
        assert!(!is_playlist_source("folk"));
        assert!(is_playlist_source("https://x/a.m3u"));
        let f = std::env::temp_dir().join(format!("vivi-pl-{}.m3u", std::process::id()));
        std::fs::write(&f, "http://a/b\n").unwrap();
        assert!(is_playlist_source(f.to_str().unwrap()));
        let _ = std::fs::remove_file(f);
        assert!(placeholder(M3U).contains("Choose file"));
    }

    #[test]
    fn stations_json() {
        let v = serde_json::json!([
            {"name": " Jazz ", "url": "http://a", "url_resolved": "http://b", "tags": "jazz,smooth",
             "country": "France", "codec": "MP3", "bitrate": 128, "favicon": "", "homepage": "h"},
            {"name": "broken", "url": "", "url_resolved": ""}
        ]);
        let s = parse_stations(&v);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "Jazz");
        assert_eq!(s[0].url, "http://b");
        assert_eq!(s[0].group, "jazz, smooth");
        assert_eq!(s[0].label(), "France  ·  MP3 128 kbps  ·  jazz, smooth");
        assert_eq!(top_groups(&s, 5), ["jazz", "smooth"]);
    }

    #[test]
    fn recent_dedup() {
        let a = Stream {
            url: "a".into(),
            ..Default::default()
        };
        let b = Stream {
            url: "b".into(),
            ..Default::default()
        };
        let mut l = vec![a.clone(), b.clone()];
        push_front(&mut l, &b, 2);
        assert_eq!(l, vec![b, a.clone()]);
        push_front(
            &mut l,
            &Stream {
                url: "c".into(),
                ..Default::default()
            },
            2,
        );
        assert_eq!(l.len(), 2);
        assert_eq!(
            l[1],
            Stream {
                url: "b".into(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn info_text() {
        let s = now_info(|p| match p {
            "metadata/by-key/icy-title" => Some("Artist - Song".into()),
            "audio-codec-name" => Some("mp3".into()),
            "audio-bitrate" => Some("128000".into()),
            _ => None,
        });
        assert_eq!(s, "Now playing: Artist - Song\nAudio: mp3 128 kbps");
        assert_eq!(now_info(|_| None), "Nothing is playing");
    }

    #[test]
    fn pickers_and_filter() {
        let v = serde_json::json!([{"code":"UA","name":"Ukraine","flag":"🇺🇦"},{"code":"AD","name":"Andorra"}]);
        let c = parse_iptv_index(&v, true);
        assert_eq!(c[0].name, "Andorra");
        assert_eq!(c[1].url, format!("{IPTV}/countries/ua.m3u"));
        assert_eq!(c[1].source, PICK);
        let t = parse_tags(
            &serde_json::json!([{"name":"jazz","stationcount":5},{"name":"x","stationcount":3}]),
        );
        assert_eq!(t.len(), 3);
        assert_eq!(t[2].url, "jazz");
        let all = parse_m3u("#EXTINF:-1 group-title=\"News\",BBC One\nhttp://a\n#EXTINF:-1,Euronews\nhttp://b\n#EXTINF:-1 group-title=\"BBC\",Other\nhttp://c\n");
        let f = filter(&all, "bbc");
        assert_eq!(
            f.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            ["http://a", "http://c"]
        );
        assert_eq!(filter(&all, " ").len(), 3);
        assert!(live_filter(TV_COUNTRY) && !live_filter(M3U) && SOURCES[SAVED] == "Favorites");
    }

    #[test]
    fn epg_groups_favorites_playlists() {
        let t = "#EXTM3U x-tvg-url=\"https://g/a.xml.gz,https://g/b.xml\"\n#EXTINF:-1 tvg-id=\"A.uk@HD\" group-title=\"News;General\",A\nhttp://a\n#EXTINF:-1 group-title=\"News\",B\nhttp://b\n#EXTINF:-1 group-title=\"Kids\",C\nhttp://c\n";
        let v = parse_m3u(t);
        assert_eq!(v[0].tvg_id, "A.uk@HD");
        assert_eq!(v[0].epg, "https://g/a.xml.gz");
        assert_eq!(
            epg_url("#EXTM3U url-tvg=\"http://x/g.xml\""),
            "http://x/g.xml"
        );
        assert_eq!(top_groups(&v, 5), ["News", "General", "Kids"]);
        assert_eq!(by_group(&v, "news").len(), 2);
        assert_eq!(by_group(&v, "").len(), 3);
        let mut w = v.clone();
        let favs = ["http://c".to_string()].into_iter().collect();
        pin_favorites(&mut w, &favs);
        assert_eq!(w[0].url, "http://c");
        assert_eq!(w[1].url, "http://a");
        assert_eq!(ago(0, 100), "never");
        assert_eq!(ago(100, 7300), "2 h ago");
        assert_eq!(playlist_name("https://h/p/My%20TV.m3u8?x=1"), "My TV");
        assert_eq!(
            playlist_picks(&[Playlist {
                name: "P".into(),
                url: "u".into(),
                count: 3,
                error: "404".into(),
                ..Default::default()
            }])[0]
                .url,
            "pl:u"
        );
        // Saved favorites stay readable without the new fields.
        let old: Stream = serde_json::from_str(r#"{"name":"n","url":"u"}"#).unwrap();
        assert!(old.tvg_id.is_empty());
        assert!(!serde_json::to_string(&old).unwrap().contains("tvg_id"));
        assert!(live_filter(MY_PL) && SOURCES[ALL].starts_with("Search all"));
    }

    #[test]
    fn somafm_json() {
        let v = serde_json::json!({"channels":[{"id":"groovesalad","title":"Groove Salad","genre":"ambient|electronica","description":"d","image":"http://i/s.png","largeimage":"",
            "playlists":[{"url":"http://p/gs.pls","format":"aac","quality":"highest"},{"url":"http://p/gs130.pls","format":"mp3","quality":"high"}]}]});
        let s = parse_somafm(&v);
        assert_eq!(s[0].url, "http://p/gs130.pls");
        assert_eq!(s[0].logo, "http://i/s.png");
        assert!(s[0].group.starts_with("ambient, electronica"));
        // Chips are the genres, not the description.
        assert_eq!(top_groups(&s, 5), ["ambient", "electronica"]);
        assert_eq!(by_group(&s, "electronica").len(), 1);
    }

    #[test]
    fn cache_names_are_stable_and_old_ones_migrate() {
        // FNV-1a 64 test vectors: the names must never change between builds.
        assert_eq!(hash_name(""), "cbf29ce484222325");
        assert_eq!(hash_name("a"), "af63dc4c8601ec8c");
        assert_eq!(hash_name("foobar"), "85944171f73967e8");
        // The spelled-out legacy name is what DefaultHasher gave (Vivi 0.12 and older).
        for u in [
            "",
            "a",
            "https://i.ytimg.com/vi/abcdefghijk/hqdefault.jpg",
            "1234567",
        ] {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            u.hash(&mut h);
            assert_eq!(legacy_hash_name(u), format!("{:016x}", h.finish()), "{u:?}");
        }
        let dir = std::env::temp_dir().join(format!("vivi-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = "https://example.com/logo.png";
        let old = dir.join(format!("{}.svg", legacy_hash_name(url)));
        std::fs::write(&old, b"<svg/>").unwrap();
        let p = cached_with_ext(&dir, url, &["png", "svg"]).unwrap();
        assert_eq!(p, dir.join(format!("{}.svg", hash_name(url))));
        assert_eq!(std::fs::read(&p).unwrap(), b"<svg/>");
        assert!(!old.exists());
        // A new file wins over an old one; nothing found stays None.
        let old2 = dir.join(format!("custom-{}.m3u", legacy_hash_name(url)));
        std::fs::write(&old2, "old").unwrap();
        let new2 = dir.join(format!("custom-{}.m3u", hash_name(url)));
        std::fs::write(&new2, "new").unwrap();
        let f = hashed_file(&dir, url, |h| format!("custom-{h}.m3u"));
        assert_eq!(std::fs::read_to_string(f).unwrap(), "new");
        assert!(cached_with_ext(&dir, "https://other", &["png"]).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn station_ids_and_click_path() {
        let v = serde_json::json!([{"name": "A", "url": "http://a",
            "stationuuid": "510506E7-6bc0-4b91-b6a1-fc024ccad1a8"}]);
        let s = parse_stations(&v);
        assert_eq!(s[0].stationuuid, "510506E7-6bc0-4b91-b6a1-fc024ccad1a8");
        assert_eq!(
            click_path(&s[0].stationuuid).unwrap(),
            "/json/url/510506e7-6bc0-4b91-b6a1-fc024ccad1a8"
        );
        assert!(click_path("").is_none());
        assert!(click_path("../../json/stations?x=1aaaaaaaaaaaaaaaaaaaaaaaa").is_none());
        // Favorites keep the id; older ones without it still load.
        let j = serde_json::to_string(&s[0]).unwrap();
        assert!(j.contains("stationuuid"));
        let old: Stream = serde_json::from_str(r#"{"name":"n","url":"u"}"#).unwrap();
        assert!(old.stationuuid.is_empty());
    }

    /// A one-shot HTTP server on localhost answering every request with `status` / `body`;
    /// returns its base URL and a counter of requests it got.
    fn serve(
        status: u16,
        body: &'static str,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n2 = n.clone();
        std::thread::spawn(move || {
            for mut c in l.incoming().flatten() {
                n2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = c.read(&mut buf);
                let _ = write!(
                    c,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base, n)
    }

    #[test]
    fn radio_servers_fail_over() {
        let v = serde_json::json!([
            {"ip": "1.2.3.4", "name": "de1.api.radio-browser.info"},
            {"ip": "::1", "name": "de1.api.radio-browser.info"},
            {"ip": "5.6.7.8", "name": "NL1.api.radio-browser.info"},
            {"ip": "9.9.9.9", "name": "evil.example.com"},
            {"ip": "9.9.9.9", "name": "x/y.api.radio-browser.info"}
        ]);
        assert_eq!(
            parse_servers(&v),
            [
                "https://de1.api.radio-browser.info",
                "https://nl1.api.radio-browser.info"
            ]
        );
        let mut a: Vec<u32> = (0..20).collect();
        shuffle(&mut a, 42);
        assert_ne!(a, (0..20).collect::<Vec<_>>());
        a.sort();
        assert_eq!(a, (0..20).collect::<Vec<_>>());
        // A refused connection and a busy mirror are skipped; the failed ones are reported.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let (busy, _) = serve(503, "{}");
        let (good, hits) = serve(200, r#"[{"name":"S","url":"http://s"}]"#);
        let mut failed = Vec::new();
        let v = radio_get_from(
            &[dead.clone(), busy.clone(), good.clone()],
            "/json/stations/search",
            &[("name", "s")],
            |s| failed.push(s.to_string()),
        )
        .unwrap();
        assert_eq!(parse_stations(&v)[0].url, "http://s");
        assert_eq!(failed, [dead, busy]);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A 4xx answer is about the request: the next mirror is not asked.
        let (bad, _) = serve(404, "{}");
        let (unused, unused_hits) = serve(200, "[]");
        let e = radio_get_from(&[bad, unused], "/json/url/x", &[], |_| {}).unwrap_err();
        assert!(e.contains("404"), "{e}");
        assert_eq!(unused_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn search_all_merges_somafm_and_splits_kinds() {
        let soma = parse_somafm(
            &serde_json::json!({"channels":[{"id":"groovesalad","title":"Groove Salad","genre":"ambient","description":"d","image":"",
            "playlists":[{"url":"http://p/gs130.pls","format":"mp3","quality":"high"}]}]}),
        );
        let tv = iptv_playlist_from("#EXTINF:-1 tvg-id=\"g.uk\",Groove TV\nhttp://t/g.m3u8\n");
        let mine = parse_m3u("#EXTINF:-1,Groove FM\nhttp://r/live.mp3\n#EXTINF:-1,Groove News HD\nhttp://t/news.m3u8\n");
        let radio = vec![Stream {
            name: "Groove Radio".into(),
            url: "http://rb/1".into(),
            source: "radio-browser.info".into(),
            ..Default::default()
        }];
        let parts = || {
            vec![
                Ok(mine.clone()),
                Ok(tv.clone()),
                Ok(filter(&soma, "groove")),
                Ok(radio.clone()),
                Err("iptv-org: offline".to_string()),
            ]
        };
        let all = merge_results(parts(), None).unwrap();
        assert_eq!(all.len(), 5);
        assert!(all.iter().any(|s| s.source == "SomaFM"));
        let names = |k| {
            merge_results(parts(), Some(k))
                .unwrap()
                .into_iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(Kind::Radio),
            ["Groove FM", "Groove Salad", "Groove Radio"]
        );
        assert_eq!(names(Kind::Tv), ["Groove News HD", "Groove TV"]);
        assert_eq!(Kind::parse("TV"), Some(Kind::Tv));
        assert_eq!(Kind::parse("podcast"), None);
        // Duplicates by URL are dropped; nothing found + an error = the error.
        assert_eq!(
            merge_results(vec![Ok(radio.clone()), Ok(radio.clone())], None)
                .unwrap()
                .len(),
            1
        );
        assert!(merge_results(vec![Ok(vec![]), Err("down".into())], None).is_err());
    }

    fn iptv_playlist_from(t: &str) -> Vec<Stream> {
        let mut v = parse_m3u(t);
        for s in &mut v {
            s.source = "iptv-org".into();
        }
        v
    }

    #[test]
    fn missing_playlist_files_are_errors_not_channel_names() {
        let tmp = std::env::temp_dir().join(format!("vivi-missing-{}", std::process::id()));
        let missing = tmp.join("nope.m3u");
        let q = missing.to_string_lossy().to_string();
        assert_eq!(m3u_query(&q), M3uQuery::Missing(missing.clone()));
        assert!(is_playlist_source(&q));
        let e = load_m3u(&q).unwrap_err();
        assert!(e.starts_with("Playlist file not found"), "{e}");
        for q in [
            "~/no/such.m3u8",
            "./list.m3u",
            "C:\\TV\\list.m3u",
            "file:///no/such.m3u",
            "channels.M3U8",
        ] {
            assert!(matches!(m3u_query(q), M3uQuery::Missing(_)), "{q}");
        }
        for q in ["bbc news", "News 24/7", "Euronews"] {
            assert_eq!(m3u_query(q), M3uQuery::Name, "{q}");
        }
        assert_eq!(m3u_query("https://x/a.m3u"), M3uQuery::Url);
        std::fs::create_dir_all(&tmp).unwrap();
        let f = tmp.join("my list.m3u");
        std::fs::write(&f, "http://a/b\n").unwrap();
        assert_eq!(m3u_query(f.to_str().unwrap()), M3uQuery::File(f.clone()));
        let url = format!("file://{}", f.to_string_lossy().replace(' ', "%20"));
        assert_eq!(load_m3u(&url).unwrap()[0].url, "http://a/b");
        assert!(load_m3u(tmp.to_str().unwrap())
            .unwrap_err()
            .starts_with("Not a playlist file"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn songs_on_servers_are_not_stations() {
        let props = |d: Option<&'static str>, icy: bool| {
            move |k: &str| match k {
                "duration" => d.map(String::from),
                "metadata/by-key/icy-title" if icy => Some("A - B".into()),
                _ => None,
            }
        };
        // A Free Music / podcast file: known length, no stream title.
        assert!(!looks_like_station(false, props(Some("215.3"), false)));
        // Live radio: no length (or 0), or a stream title, or one of our stations.
        assert!(looks_like_station(false, props(None, false)));
        assert!(looks_like_station(false, props(Some("0"), false)));
        assert!(looks_like_station(false, props(Some("215.3"), true)));
        assert!(looks_like_station(true, props(Some("215.3"), false)));
    }

    #[test]
    fn drawer_panel_text() {
        let st = Stream {
            name: "Groove%20Salad".into(),
            ..Default::default()
        };
        let t = drawer_text(Some(&st), true, "Now playing: A - B", 1, 3);
        assert_eq!(
            t,
            "Groove Salad\nNow playing: A - B\n\n1 favorite · 3 recent streams"
        );
        let t = drawer_text(None, false, "Nothing is playing", 0, 1);
        assert!(t.starts_with("No network stream is playing."));
        assert!(t.ends_with("0 favorites · 1 recent stream"));
        // The panel asks for it when it opens.
        let m = NetworkStreams::default().manifest();
        assert!(m
            .contributes
            .iter()
            .any(|c| c.point == contrib::DRAWER_PANEL && c.command == "drawer"));
    }

    /// Live iptv-org and SomaFM. `--ignored`.
    #[test]
    #[ignore]
    fn live_iptv_somafm() {
        let c = iptv_index(true).unwrap();
        let ua = c.iter().find(|s| s.name == "Ukraine").unwrap();
        let ch = iptv_playlist(&ua.url).unwrap();
        eprintln!(
            "UA: {} channels, {} with logos",
            ch.len(),
            ch.iter().filter(|s| !s.logo.is_empty()).count()
        );
        assert!(!ch.is_empty());
        assert!(!iptv_index(false).unwrap().is_empty());
        assert!(!iptv_search("news").unwrap().is_empty());
        assert!(genre_list().unwrap().len() > 10);
        assert!(!genre_stations("@topclick").unwrap().is_empty());
        assert!(somafm().unwrap().len() > 10);
        let all = search_all("groove").unwrap();
        assert!(
            all.iter().any(|s| s.source == "SomaFM"),
            "SomaFM in Search all"
        );
    }

    /// Live radio-browser.info search (no key). `--ignored`.
    #[test]
    #[ignore]
    fn live_radio_browser() {
        let servers = radio_servers();
        eprintln!("servers: {servers:?}");
        assert!(!servers.is_empty());
        let jazz = radio_search(0, "jazz").unwrap();
        assert!(click_path(&jazz[0].stationuuid).is_some());
        let v = radio_get(&click_path(&jazz[0].stationuuid).unwrap(), &[]).unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert!(!radio_any("folk").unwrap().is_empty());
        for (m, q) in [(0, "jazz"), (1, ""), (2, "UA"), (3, "rock")] {
            let v = radio_search(m, q).unwrap();
            assert!(!v.is_empty(), "{m} {q}");
            eprintln!("{m} {q}: {} e.g. {:?}", v.len(), v[0].name);
        }
    }
}
