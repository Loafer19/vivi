//! Network Streams browser (#67): Internet radio from the radio-browser.info directory
//! (no key), M3U / M3U8 / IPTV playlist import (file or URL, with `tvg-logo` and
//! `group-title`), saved streams and recently played streams, plus a stream info
//! readout (ICY now-playing title, codec, bitrate, buffer).
//!
//! It is its own dialog (the shared plugin dialog), separate from the Open Network
//! Stream dialog. Saved and recent streams live in
//! `$XDG_DATA_HOME/vivi/streams/{saved,recent}.json`.

use super::web;
use super::{Ctx, Manifest, Plugin, SettingSpec};
use crate::Item;
use serde::{Deserialize, Serialize};
use slint::{ModelRc, SharedString, VecModel};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

pub const ID: &str = "network-streams";
pub const SOURCES: [&str; 11] = [
    "Radio: search",
    "Radio: top voted",
    "Radio: by country",
    "Radio: by genre",
    "M3U / IPTV playlist",
    "TV: by country",
    "TV: by category",
    "Music: genres",
    "Music: SomaFM",
    "Saved",
    "Recent",
];
const M3U: usize = 4;
const TV_COUNTRY: usize = 5;
const TV_CATEGORY: usize = 6;
const GENRES: usize = 7;
const SOMAFM: usize = 8;
const SAVED: usize = 9;
const RECENT: usize = 10;
/// Sources whose list is filtered locally as the user types.
fn live_filter(mode: usize) -> bool {
    (TV_COUNTRY..=SOMAFM).contains(&mode)
}
/// `source` of picker entries (a country / category / genre to open).
const PICK: &str = "pick";
const IPTV: &str = "https://iptv-org.github.io/iptv";
const IPTV_API: &str = "https://iptv-org.github.io/api";
/// radio-browser.info mirrors, tried in order.
const SERVERS: [&str; 3] = [
    "https://de1.api.radio-browser.info",
    "https://fi1.api.radio-browser.info",
    "https://at1.api.radio-browser.info",
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

// ---------- radio-browser.info ----------

/// Stations from a radio-browser JSON array.
pub fn parse_stations(v: &serde_json::Value) -> Vec<Stream> {
    let s = |x: &serde_json::Value| x.as_str().unwrap_or("").trim().to_string();
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let url = Some(s(&e["url_resolved"]))
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| s(&e["url"]));
            if url.is_empty() {
                return None;
            }
            Some(Stream {
                name: s(&e["name"]),
                url,
                logo: s(&e["favicon"]),
                group: s(&e["tags"]).replace(',', ", "),
                country: s(&e["country"]),
                codec: s(&e["codec"]),
                bitrate: e["bitrate"].as_u64().unwrap_or(0) as u32,
                homepage: s(&e["homepage"]),
                source: "radio-browser.info".into(),
            })
        })
        .collect()
}

/// Query the directory. `mode` is a [`SOURCES`] index 0..=3.
pub fn radio_search(mode: usize, q: &str) -> Result<Vec<Stream>, String> {
    let q = q.trim();
    let (path, mut params): (&str, Vec<(&str, &str)>) = match mode {
        1 => ("/json/stations/topvote", vec![]),
        2 if q.len() == 2 => ("/json/stations/search", vec![("countrycode", q)]),
        2 => ("/json/stations/search", vec![("country", q)]),
        3 => ("/json/stations/search", vec![("tag", q)]),
        _ => ("/json/stations/search", vec![("name", q)]),
    };
    params.extend([
        ("limit", "100"),
        ("hidebroken", "true"),
        ("order", "votes"),
        ("reverse", "true"),
    ]);
    let mut err = String::new();
    for srv in SERVERS {
        match web::get_json(&format!("{srv}{path}"), &params, "radio-browser.info") {
            Ok(v) => return Ok(parse_stations(&v)),
            Err(e) => err = e,
        }
    }
    Err(err)
}

/// Stations whose name or tag matches `q` (name matches first, no duplicates).
pub fn radio_any(q: &str) -> Result<Vec<Stream>, String> {
    let mut v = radio_search(0, q)?;
    if let Ok(t) = radio_search(3, &q.to_lowercase()) {
        for s in t {
            if !v.iter().any(|o| o.url == s.url) {
                v.push(s);
            }
        }
    }
    Ok(v)
}

/// Whether `q` names a playlist we can load (URL or existing file).
pub fn is_playlist_source(q: &str) -> bool {
    let q = q.trim();
    if q.starts_with("http://") || q.starts_with("https://") {
        return true;
    }
    let p = match q.strip_prefix("~/") {
        Some(r) => crate::store::home().join(r),
        None => PathBuf::from(q),
    };
    p.is_file()
}

/// Placeholder of the query field per source.
pub fn placeholder(mode: usize) -> &'static str {
    match mode {
        0 => "Station name, e.g. jazz fm",
        1 => "Press Go for the most popular stations",
        2 => "Country or code, e.g. Ukraine or UA",
        3 => "Genre tag, e.g. folk, rock, news",
        M3U => "Playlist path or URL (.m3u / .m3u8), Choose file…, or a TV channel name",
        TV_COUNTRY => "Filter countries, then channels by name (Go = back to countries)",
        TV_CATEGORY => "Filter categories, then channels by name (Go = back to categories)",
        GENRES => "Filter genres, then stations (Go = back to genres)",
        SOMAFM => "Filter SomaFM channels by name or genre",
        SAVED => "Press Go to show saved streams",
        _ => "Press Go to show recently played streams",
    }
}

/// Tell the directory a station was played (its click counter); best effort.
fn radio_click(url: &str) {
    let url = url.to_string();
    std::thread::spawn(move || {
        let _ = web::get_json(
            &format!("{}/json/url", SERVERS[0]),
            &[("url", &url)],
            "radio-browser.info",
        );
    });
}

/// Picker of genres: Popular, Trending, then the biggest radio-browser tags.
pub fn parse_tags(v: &serde_json::Value) -> Vec<Stream> {
    let mut out = vec![
        pick("Popular", "Most voted stations", "@topvote"),
        pick("Trending", "Most played right now", "@topclick"),
    ];
    for t in v.as_array().into_iter().flatten() {
        let name = t["name"].as_str().unwrap_or("").trim();
        let n = t["stationcount"].as_u64().unwrap_or(0);
        if name.len() > 1 && n > 0 {
            out.push(pick(name, &format!("{n} stations"), name));
        }
    }
    out
}

fn pick(name: &str, label: &str, key: &str) -> Stream {
    Stream {
        name: name.into(),
        url: key.into(),
        group: label.into(),
        source: PICK.into(),
        ..Default::default()
    }
}

fn genre_list() -> Result<Vec<Stream>, String> {
    let p = [
        ("order", "stationcount"),
        ("reverse", "true"),
        ("limit", "120"),
        ("hidebroken", "true"),
    ];
    let mut err = String::new();
    for srv in SERVERS {
        match web::get_json(&format!("{srv}/json/tags"), &p, "radio-browser.info") {
            Ok(v) => return Ok(parse_tags(&v)),
            Err(e) => err = e,
        }
    }
    Err(err)
}

/// Stations of a genre picker key (`@topvote`, `@topclick` or a tag).
fn genre_stations(key: &str) -> Result<Vec<Stream>, String> {
    let path = match key {
        "@topvote" => "/json/stations/topvote",
        "@topclick" => "/json/stations/topclick",
        _ => return radio_search(3, key),
    };
    let mut err = String::new();
    for srv in SERVERS {
        let p = [("limit", "100"), ("hidebroken", "true")];
        match web::get_json(&format!("{srv}{path}"), &p, "radio-browser.info") {
            Ok(v) => return Ok(parse_stations(&v)),
            Err(e) => err = e,
        }
    }
    Err(err)
}

// ---------- SomaFM ----------

/// SomaFM `channels.json`: one stream per channel, best MP3/AAC playlist.
pub fn parse_somafm(v: &serde_json::Value) -> Vec<Stream> {
    let s = |x: &serde_json::Value| x.as_str().unwrap_or("").trim().to_string();
    let rank = |q: &str| match q {
        "highest" => 3,
        "high" => 2,
        "low" => 1,
        _ => 0,
    };
    v["channels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let pl = c["playlists"].as_array()?;
            let best = pl
                .iter()
                .filter(|p| p["url"].as_str().is_some())
                .max_by_key(|p| (s(&p["format"]) == "mp3") as u8 * 10 + rank(&s(&p["quality"])))?;
            Some(Stream {
                name: s(&c["title"]),
                url: s(&best["url"]),
                logo: Some(s(&c["largeimage"]))
                    .filter(|l| !l.is_empty())
                    .unwrap_or_else(|| s(&c["image"])),
                group: format!(
                    "{} — {}",
                    s(&c["genre"]).replace('|', ", "),
                    s(&c["description"])
                ),
                codec: s(&best["format"]).to_uppercase(),
                homepage: format!("https://somafm.com/{}/", s(&c["id"])),
                source: "SomaFM".into(),
                ..Default::default()
            })
        })
        .collect()
}

fn somafm() -> Result<Vec<Stream>, String> {
    let v = cached_json(
        "somafm.json",
        "https://api.somafm.com/channels.json",
        "SomaFM",
    )?;
    Ok(parse_somafm(&v))
}

// ---------- iptv-org ----------

const TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Fetch `url` into the cache as `name` (reused for a day; stale copy on errors).
fn cached(name: &str, url: &str) -> Result<String, String> {
    let p = crate::store::cache_dir().join("iptv").join(name);
    let fresh = std::fs::metadata(&p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e < TTL);
    if fresh {
        if let Ok(t) = std::fs::read_to_string(&p) {
            return Ok(t);
        }
    }
    match web::get_bytes(url, 64 * 1024 * 1024) {
        Ok(Some(b)) => {
            let t = String::from_utf8_lossy(&b).into_owned();
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(&p, &t);
            Ok(t)
        }
        r => std::fs::read_to_string(&p).map_err(|_| match r {
            Err(e) => e,
            _ => format!("Not found: {url}"),
        }),
    }
}

fn cached_json(name: &str, url: &str, what: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(&cached(name, url)?).map_err(|e| format!("{what}: {e}"))
}

/// Countries (`code`, `name`, `flag`) or categories (`id`, `name`) of the iptv-org API.
pub fn parse_iptv_index(v: &serde_json::Value, countries: bool) -> Vec<Stream> {
    let mut out: Vec<Stream> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let name = e["name"].as_str()?.trim();
            let id = e[if countries { "code" } else { "id" }]
                .as_str()?
                .to_lowercase();
            let (dir, label) = if countries {
                (
                    "countries",
                    format!("{} {}", e["flag"].as_str().unwrap_or(""), id.to_uppercase()),
                )
            } else {
                ("categories", String::new())
            };
            Some(pick(name, label.trim(), &format!("{IPTV}/{dir}/{id}.m3u")))
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn iptv_index(countries: bool) -> Result<Vec<Stream>, String> {
    let f = if countries {
        "countries.json"
    } else {
        "categories.json"
    };
    let v = cached_json(f, &format!("{IPTV_API}/{f}"), "iptv-org")?;
    let v = parse_iptv_index(&v, countries);
    if v.is_empty() {
        return Err("iptv-org returned no entries".into());
    }
    Ok(v)
}

/// Channels of an iptv-org playlist URL (cached).
fn iptv_playlist(url: &str) -> Result<Vec<Stream>, String> {
    let name = url.rsplit('/').take(2).collect::<Vec<_>>().join("-");
    let mut v = parse_m3u(&cached(&name, url)?);
    for s in &mut v {
        s.source = "iptv-org".into();
    }
    Ok(v)
}

/// Channels of the whole iptv-org index whose name matches `q`.
fn iptv_search(q: &str) -> Result<Vec<Stream>, String> {
    let all = iptv_playlist(&format!("{IPTV}/index.m3u"))?;
    Ok(filter(&all, q))
}

/// Entries whose name (or group) contains every word of `q`, name matches first.
pub fn filter(all: &[Stream], q: &str) -> Vec<Stream> {
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return all.to_vec();
    }
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for s in all {
        let n = s.name.to_lowercase();
        if words.iter().all(|w| n.contains(w)) {
            a.push(s.clone());
        } else {
            let g = format!("{n} {}", s.group.to_lowercase());
            if words.iter().all(|w| g.contains(w)) {
                b.push(s.clone());
            }
        }
    }
    a.extend(b);
    a
}

// ---------- M3U ----------

fn attr(line: &str, key: &str) -> String {
    let pat = format!("{key}=\"");
    line.find(&pat)
        .and_then(|i| {
            let rest = &line[i + pat.len()..];
            rest.find('"').map(|j| rest[..j].to_string())
        })
        .unwrap_or_default()
}

/// Parse an M3U / extended M3U / IPTV playlist. Relative entries are skipped.
pub fn parse_m3u(text: &str) -> Vec<Stream> {
    let mut out = Vec::new();
    let mut pending: Option<Stream> = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            // Name = text after the last comma outside quotes.
            let mut inq = false;
            let mut cut = None;
            for (i, c) in info.char_indices() {
                match c {
                    '"' => inq = !inq,
                    ',' if !inq => cut = Some(i),
                    _ => {}
                }
            }
            let name = cut.map(|i| info[i + 1..].trim()).unwrap_or("").to_string();
            let name = if name.is_empty() {
                attr(info, "tvg-name")
            } else {
                name
            };
            pending = Some(Stream {
                name: display_name(&name),
                logo: attr(info, "tvg-logo"),
                group: attr(info, "group-title"),
                country: attr(info, "tvg-country"),
                source: "M3U".into(),
                ..Default::default()
            });
        } else if line.starts_with('#') {
            continue;
        } else if line.contains("://") {
            let mut s = pending.take().unwrap_or_else(|| Stream {
                source: "M3U".into(),
                ..Default::default()
            });
            s.url = line.to_string();
            if s.name.is_empty() {
                s.name = display_name(line.rsplit('/').find(|p| !p.is_empty()).unwrap_or(line));
            }
            out.push(s);
        } else {
            pending = None;
        }
    }
    out
}

/// Load a playlist from a URL or a local path (`~/` expanded).
pub fn load_m3u(src: &str) -> Result<Vec<Stream>, String> {
    let src = src.trim();
    let text = if src.starts_with("http://") || src.starts_with("https://") {
        let b = web::get_bytes(src, 32 * 1024 * 1024)?.ok_or("Playlist not found (404)")?;
        String::from_utf8_lossy(&b).into_owned()
    } else {
        let p = match src.strip_prefix("~/") {
            Some(r) => crate::store::home().join(r),
            None => PathBuf::from(src),
        };
        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?
    };
    let v = parse_m3u(&text);
    if v.is_empty() {
        return Err("No stream URLs found in that playlist".into());
    }
    Ok(v)
}

// ---------- saved / recent ----------

fn dir() -> PathBuf {
    crate::store::data_dir().join("streams")
}

fn load_list(name: &str) -> Vec<Stream> {
    std::fs::read_to_string(dir().join(name))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_list(name: &str, v: &[Stream]) {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    let tmp = d.join(format!(".{name}.tmp"));
    if let Ok(t) = serde_json::to_string_pretty(v) {
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, d.join(name));
        }
    }
}

/// Put `s` first in `list` (dedup by URL), keep at most `max`.
pub fn push_front(list: &mut Vec<Stream>, s: &Stream, max: usize) {
    list.retain(|o| o.url != s.url);
    list.insert(0, s.clone());
    list.truncate(max);
}

/// Stream info of what is playing now, from mpv properties.
pub fn now_info(get: impl Fn(&str) -> Option<String>) -> String {
    let mut p = Vec::new();
    let title = get("metadata/by-key/icy-title")
        .or_else(|| get("media-title"))
        .unwrap_or_default();
    if !title.is_empty() {
        p.push(format!("Now playing: {title}"));
    }
    if let Some(n) = get("metadata/by-key/icy-name").filter(|s| !s.is_empty()) {
        p.push(format!("Station: {n}"));
    }
    let codec = get("audio-codec-name").unwrap_or_default();
    let br = get("audio-bitrate")
        .and_then(|b| b.parse::<f64>().ok())
        .filter(|b| *b > 0.0)
        .map(|b| format!(" {:.0} kbps", b / 1000.0))
        .unwrap_or_default();
    if !codec.is_empty() {
        p.push(format!("Audio: {codec}{br}"));
    }
    if let Some(v) = get("video-codec").filter(|s| !s.is_empty()) {
        p.push(format!("Video: {v}"));
    }
    if let Some(c) = get("demuxer-cache-duration").and_then(|c| c.parse::<f64>().ok()) {
        p.push(format!("Buffer: {c:.1} s"));
    }
    if p.is_empty() {
        "Nothing is playing".into()
    } else {
        p.join("\n")
    }
}

// ---------- plugin ----------

#[derive(Default)]
pub struct Streams {
    list: Vec<Stream>,
    /// Source index the current list came from.
    mode: usize,
    rx: Option<Receiver<Result<Vec<Stream>, String>>>,
    /// Status shown when the results arrive instead of the count (fallback searches).
    note: Option<String>,
    /// Source shown in the dropdown last tick (placeholder/button follow it).
    last_source: String,
    /// Station picked in the browser, for the now-playing card.
    playing: Option<Stream>,
    np_url: String,
    logo_rx: Option<Receiver<(String, Option<PathBuf>)>>,
    poll: u32,
    /// Unfiltered list of live-filter sources; `list` is its filtered view.
    all: Vec<Stream>,
    filtered_by: String,
    /// Row logos (by logo URL) and their background loader.
    logos: std::collections::HashMap<String, slint::Image>,
    row_logo_rx: Option<Receiver<(String, Option<PathBuf>)>>,
    logos_dirty: bool,
}

impl Streams {
    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app.set_md_status(status.into());
        cx.app.set_md_busy(busy);
    }
    fn show(&mut self, cx: &Ctx) {
        self.show_rows(cx);
        self.load_row_logos();
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
                let p = fetch_logo(&l);
                if tx.send((l, p)).is_err() {
                    break;
                }
            }
        });
    }
    fn show_rows(&self, cx: &Ctx) {
        let logos: Vec<slint::Image> = self
            .list
            .iter()
            .map(|s| self.logos.get(&s.logo).cloned().unwrap_or_default())
            .collect();
        cx.app.set_md_logos(ModelRc::new(VecModel::from(logos)));
        let items: Vec<Item> = self
            .list
            .iter()
            .map(|s| Item {
                key: display_name(&s.name).into(),
                label: s.label().into(),
                selected: false,
            })
            .collect();
        cx.app.set_md_results(ModelRc::new(VecModel::from(items)));
        Self::secondary(cx, self.mode, false);
    }
    /// Secondary button: Remove (saved/recent), Choose file… (playlist source without a
    /// selection) or Save.
    fn secondary(cx: &Ctx, mode: usize, selected: bool) {
        let t = match mode {
            SAVED | RECENT => "Remove",
            M3U if !selected => "Choose file…",
            _ => "Save",
        };
        cx.app.set_md_secondary(t.into());
    }
    fn mode(cx: &Ctx) -> usize {
        let s = cx.app.get_md_source();
        SOURCES.iter().position(|x| *x == s.as_str()).unwrap_or(0)
    }
    fn local(&mut self, cx: &Ctx, mode: usize) {
        self.mode = mode;
        self.list = load_list(if mode == SAVED {
            "saved.json"
        } else {
            "recent.json"
        });
        self.show(cx);
        cx.app.set_md_detail("".into());
        let what = if mode == SAVED { "saved" } else { "recent" };
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
}

/// Station of `url` in the saved and recent lists.
fn known_station(url: &str) -> Option<Stream> {
    load_list("saved.json")
        .into_iter()
        .chain(load_list("recent.json"))
        .find(|s| s.url == url)
}

/// Download a station logo into the cache (once); `None` when it cannot be fetched.
fn fetch_logo(url: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    let dir = crate::store::cache_dir().join("stream-logos");
    let p = dir.join(format!("{:016x}", h.finish()));
    if p.is_file() {
        return Some(p);
    }
    let b = web::get_bytes(url, 2 * 1024 * 1024).ok()??;
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&p, b).ok()?;
    Some(p)
}

impl Streams {
    /// Fill the now-playing card while an audio-only network stream plays.
    fn now_playing(&mut self, cx: &Ctx) {
        let a = cx.app;
        let path = cx.player.get("path").unwrap_or_default();
        let show = a.get_has_media() && a.get_viz_audio() && path.contains("://");
        if !show {
            if a.get_np_show() {
                a.set_np_show(false);
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
            a.set_np_has_logo(false);
            self.logo_rx = None;
            let name = st
                .as_ref()
                .map(|s| display_name(&s.name))
                .unwrap_or_default();
            a.set_np_station(name.into());
            if let Some(logo) = st.map(|s| s.logo).filter(|l| l.starts_with("http")) {
                let (tx, rx) = channel();
                self.logo_rx = Some(rx);
                let u = path.clone();
                std::thread::spawn(move || {
                    let _ = tx.send((u, fetch_logo(&logo)));
                });
            }
        }
        let g = |k: &str| cx.player.get(k).filter(|v| !v.trim().is_empty());
        if a.get_np_station().is_empty() {
            let n = g("metadata/by-key/icy-name")
                .or_else(|| g("media-title"))
                .unwrap_or_else(|| display_name(path.rsplit('/').next().unwrap_or(&path)));
            a.set_np_station(n.into());
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
        a.set_np_track(track.into());
        a.set_np_info(info.join(" · ").into());
        if !a.get_np_show() {
            a.set_np_show(true);
        }
    }
}

impl Plugin for Streams {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Network Streams",
            "Browse Internet radio (radio-browser.info), import M3U/IPTV playlists, keep saved and recent streams, and see stream info (now-playing title, codec, bitrate, buffer).",
            "streams",
        )
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
    }
    fn disable(&mut self, cx: &Ctx) {
        self.rx = None;
        cx.app.set_np_show(false);
        if cx.app.get_md_owner() == ID {
            cx.app.set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        let idx = || args.first().and_then(|a| a.parse::<usize>().ok());
        match name {
            "open" => {
                let a = cx.app;
                a.set_settings_open(false);
                a.set_md_owner(ID.into());
                a.set_md_title("Network Streams".into());
                a.set_md_placeholder(placeholder(Self::mode(cx)).into());
                self.last_source.clear(); // the next tick sets placeholder and button
                a.set_md_search_label("Go".into());
                a.set_md_primary("Play".into());
                let src: Vec<SharedString> = SOURCES.iter().map(|s| (*s).into()).collect();
                a.set_md_sources(ModelRc::new(VecModel::from(src)));
                if !SOURCES.contains(&a.get_md_source().as_str()) {
                    a.set_md_source(SOURCES[0].into());
                }
                a.set_md_open(true);
                self.local(cx, SAVED);
                if self.list.is_empty() {
                    Self::ui(
                        cx,
                        "Pick a source, type and press Go. Saved streams show here.",
                        false,
                    );
                }
            }
            "search" => {
                let mode = Self::mode(cx);
                if mode >= SAVED {
                    self.local(cx, mode);
                    return;
                }
                let q = cx.app.get_md_query().trim().to_string();
                if live_filter(mode) {
                    // Go = (re)load the picker; typing filters it.
                    self.all.clear();
                    self.filtered_by = q.clone();
                    self.mode = mode;
                    self.list.clear();
                    self.show(cx);
                    cx.app.set_md_detail("".into());
                    Self::ui(cx, "Loading…", true);
                    let (tx, rx) = channel();
                    self.rx = Some(rx);
                    std::thread::spawn(move || {
                        let _ = tx.send(match mode {
                            TV_COUNTRY => iptv_index(true),
                            TV_CATEGORY => iptv_index(false),
                            GENRES => genre_list(),
                            _ => somafm(),
                        });
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
                }
                self.mode = mode;
                self.list.clear();
                self.show(cx);
                cx.app.set_md_detail("".into());
                Self::ui(
                    cx,
                    if tv_search {
                        "Searching iptv-org channels…"
                    } else if mode == M3U {
                        "Loading playlist…"
                    } else {
                        "Searching radio-browser.info…"
                    },
                    true,
                );
                let (tx, rx) = channel();
                self.rx = Some(rx);
                std::thread::spawn(move || {
                    let r = match mode {
                        M3U if tv_search => iptv_search(&q),
                        M3U => load_m3u(&q),
                        0 => radio_any(&q),
                        _ => radio_search(mode, &q),
                    };
                    let _ = tx.send(r);
                });
            }
            "select" => {
                if let Some(s) = idx().and_then(|i| self.list.get(i)) {
                    Self::secondary(cx, self.mode, true);
                    let mut d = s.detail();
                    if cx.player.get("path").as_deref() == Some(s.url.as_str()) {
                        d += &format!("\n\n— Playing —\n{}", now_info(|p| cx.player.get(p)));
                    }
                    cx.app.set_md_detail(d.into());
                }
            }
            "primary" => {
                let Some(s) = idx().and_then(|i| self.list.get(i)).cloned() else {
                    Self::ui(cx, "Select a stream first", false);
                    return;
                };
                if s.source == PICK {
                    // Open the picked country / category / genre.
                    self.all.clear();
                    self.list.clear();
                    self.show(cx);
                    cx.app.set_md_query("".into());
                    self.filtered_by.clear();
                    cx.app.set_md_detail("".into());
                    Self::ui(cx, &format!("Loading {}…", s.name), true);
                    self.note = Some(format!(
                        "{}: {{n}} streams. Type to filter, Go to go back.",
                        s.name
                    ));
                    let (tx, rx) = channel();
                    self.rx = Some(rx);
                    let key = s.url.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(if key.starts_with("http") {
                            iptv_playlist(&key)
                        } else {
                            genre_stations(&key)
                        });
                    });
                    return;
                }
                crate::loading::set_name(&s.url, &display_name(&s.name));
                cx.player.cmd("loadfile", &[&s.url]);
                self.playing = Some(s.clone());
                if s.source == "radio-browser.info" {
                    radio_click(&s.url);
                }
                let mut r = load_list("recent.json");
                push_front(&mut r, &s, MAX_RECENT);
                save_list("recent.json", &r);
                cx.toast(&format!("Playing {}", s.name));
                Self::ui(cx, &format!("Playing {}", s.name), false);
            }
            "secondary" if cx.app.get_md_secondary().as_str() == "Choose file…" => {
                let Some(f) = rfd::FileDialog::new()
                    .set_title("Import a playlist")
                    .add_filter("Playlists", &["m3u", "m3u8"])
                    .pick_file()
                else {
                    return;
                };
                cx.app.set_md_query(f.to_string_lossy().as_ref().into());
                self.command(cx, "search", &[]);
            }
            "secondary" => {
                let Some(i) = idx().filter(|i| *i < self.list.len()) else {
                    Self::ui(cx, "Select a stream first", false);
                    return;
                };
                if self.mode >= SAVED {
                    let file = if self.mode == SAVED {
                        "saved.json"
                    } else {
                        "recent.json"
                    };
                    let s = self.list.remove(i);
                    save_list(file, &self.list);
                    self.show(cx);
                    cx.app.set_md_detail("".into());
                    Self::ui(cx, &format!("Removed {}", s.name), false);
                } else {
                    let s = self.list[i].clone();
                    if s.source == PICK {
                        Self::ui(cx, "Open it first, then save a stream", false);
                        return;
                    }
                    let mut v = load_list("saved.json");
                    v.retain(|o| o.url != s.url);
                    v.push(s.clone());
                    save_list("saved.json", &v);
                    Self::ui(cx, &format!("Saved {}", s.name), false);
                }
            }
            "info" => cx.toast(&now_info(|p| cx.player.get(p)).replace('\n', " · ")),
            "close" => self.rx = None,
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        self.poll = self.poll.wrapping_add(1);
        if cx.app.get_md_open() && cx.app.get_md_owner() == ID {
            let src = cx.app.get_md_source();
            if src.as_str() != self.last_source {
                self.last_source = src.to_string();
                let m = Self::mode(cx);
                cx.app.set_md_placeholder(placeholder(m).into());
                Self::secondary(cx, m, false);
            }
        }
        if live_filter(self.mode) && !self.all.is_empty() && cx.app.get_md_open() {
            let q = cx.app.get_md_query().to_string();
            if q != self.filtered_by {
                self.filtered_by = q.clone();
                self.list = filter(&self.all, &q);
                self.show(cx);
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
                let img = p
                    .and_then(|p| slint::Image::load_from_path(&p).ok())
                    .unwrap_or_default();
                self.logos.insert(url, img);
                got = true;
            }
            self.logos_dirty |= got;
        }
        if self.logos_dirty && self.poll.is_multiple_of(10) {
            self.logos_dirty = false;
            self.show_rows(cx);
        }
        if self.poll.is_multiple_of(5) {
            self.now_playing(cx);
        }
        if let Some(rx) = &self.logo_rx {
            if let Ok((url, path)) = rx.try_recv() {
                self.logo_rx = None;
                if url == self.np_url {
                    match path.and_then(|p| slint::Image::load_from_path(&p).ok()) {
                        Some(img) => {
                            cx.app.set_np_logo(img);
                            cx.app.set_np_has_logo(true);
                        }
                        None => cx.app.set_np_has_logo(false),
                    }
                }
            }
        }
        let Some(rx) = &self.rx else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.rx = None;
        match r {
            Ok(v) => {
                let n = v.len();
                if live_filter(self.mode) {
                    self.all = v.clone();
                    self.filtered_by = cx.app.get_md_query().to_string();
                    self.list = filter(&self.all, &self.filtered_by);
                } else {
                    self.list = v;
                }
                self.show(cx);
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
            Err(e) => Self::ui(cx, &e, false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(live_filter(TV_COUNTRY) && !live_filter(M3U) && SOURCES[SAVED] == "Saved");
    }

    #[test]
    fn somafm_json() {
        let v = serde_json::json!({"channels":[{"id":"groovesalad","title":"Groove Salad","genre":"ambient|electronica","description":"d","image":"http://i/s.png","largeimage":"",
            "playlists":[{"url":"http://p/gs.pls","format":"aac","quality":"highest"},{"url":"http://p/gs130.pls","format":"mp3","quality":"high"}]}]});
        let s = parse_somafm(&v);
        assert_eq!(s[0].url, "http://p/gs130.pls");
        assert_eq!(s[0].logo, "http://i/s.png");
        assert!(s[0].group.starts_with("ambient, electronica"));
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
    }

    /// Live radio-browser.info search (no key). `--ignored`.
    #[test]
    #[ignore]
    fn live_radio_browser() {
        assert!(!radio_any("folk").unwrap().is_empty());
        for (m, q) in [(0, "jazz"), (1, ""), (2, "UA"), (3, "rock")] {
            let v = radio_search(m, q).unwrap();
            assert!(!v.is_empty(), "{m} {q}");
            eprintln!("{m} {q}: {} e.g. {:?}", v.len(), v[0].name);
        }
    }
}
