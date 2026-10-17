//! Free Music (#70): search and browse Creative Commons / public-domain music on
//! Jamendo (API v3, free `client_id` from <https://devportal.jamendo.com>) and the
//! Internet Archive (advancedsearch + metadata APIs, no key; its `freemusicarchive`
//! collection stands in for the Free Music Archive, whose API is gone). Both sources
//! are searched in parallel and merged with source badges; filters (genre, mood,
//! licence, length), sort, infinite scroll, album / artist pages and "play similar".
//! Downloads land in a library folder with tags, cover, licence and attribution (in the
//! tags and in a `<file>.licence.txt`), through a built-in queue or the Offline
//! Downloads plugin.

use super::tagger::{self, Tags};
use super::web;
use super::{Ctx, Manifest, Plugin, SettingSpec};
use slint::ComponentHandle;
use slint::{ModelRc, SharedString, VecModel};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const ID: &str = "free-music";
pub const JAMENDO: &str = "Jamendo";
pub const IA: &str = "Internet Archive";
pub const FMA: &str = "Free Music Archive";
/// Source dropdown; FMA is searched through its Internet Archive mirror.
pub const SOURCES: [&str; 4] = ["All sources", JAMENDO, IA, FMA];
const NO_KEY: &str = "Jamendo needs a client ID (free at devportal.jamendo.com → Settings → Extensions → Free Music). Showing the Internet Archive only.";
pub const PAGE: usize = 24;

/// Browse chips: three lists, then genres (label, Jamendo tag / IA subject).
pub const CHIPS: [(&str, &str); 14] = [
    ("Popular", ""),
    ("New", ""),
    ("Featured", ""),
    ("Electronic", "electronic"),
    ("Rock", "rock"),
    ("Pop", "pop"),
    ("Hip-hop", "hiphop"),
    ("Jazz", "jazz"),
    ("Classical", "classical"),
    ("Ambient", "ambient"),
    ("Lo-fi", "lofi"),
    ("Folk", "folk"),
    ("Metal", "metal"),
    ("Soundtrack", "soundtrack"),
];
pub const MOODS: [&str; 8] = [
    "Any mood",
    "Relaxing",
    "Happy",
    "Energetic",
    "Sad",
    "Dark",
    "Romantic",
    "Epic",
];
pub const LICENCES: [&str; 6] = [
    "Any licence",
    "Commercial use OK",
    "Remix allowed",
    "CC BY",
    "CC BY-SA",
    "CC0 / public domain",
];
pub const LENGTHS: [&str; 4] = ["Any length", "Under 3 min", "3–6 min", "Over 6 min"];
pub const SORTS: [&str; 5] = ["Best match", "Popular", "Newest", "Shortest", "Longest"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    /// Source-specific id: Jamendo track id, IA `identifier/file`.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub artist_id: String,
    pub album: String,
    /// Jamendo album id, IA identifier.
    pub album_id: String,
    pub date: String,
    /// Seconds; 0 = unknown.
    pub duration: u32,
    pub genres: Vec<String>,
    /// Moods / other tags.
    pub tags: Vec<String>,
    pub license_url: String,
    pub stream: String,
    /// Empty when the source does not allow downloads.
    pub download: String,
    /// List thumbnail.
    pub cover: String,
    /// Full-size cover for the download (falls back to `cover`).
    pub cover_full: String,
    pub page: String,
    /// Higher = more popular (source-specific scale).
    pub popularity: u64,
    pub source: &'static str,
}

impl Track {
    pub fn license(&self) -> String {
        license_name(&self.license_url)
    }
    pub fn length(&self) -> String {
        fmt_len(self.duration)
    }
    /// "“Title” by Artist (page) via Source, licensed under CC BY 4.0 (url)."
    pub fn attribution(&self) -> String {
        format!(
            "“{}” by {} ({}) via {}, licensed under {} ({}).",
            self.title,
            if self.artist.is_empty() {
                "unknown artist"
            } else {
                &self.artist
            },
            self.page,
            self.source,
            self.license(),
            self.license_url
        )
    }
    fn key(&self) -> String {
        format!("{}\u{1f}{}", norm(&self.artist), norm(&self.title))
    }
}

pub fn fmt_len(s: u32) -> String {
    if s == 0 {
        String::new()
    } else if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------- licences ----------

/// Short licence name from a Creative Commons URL.
pub fn license_name(url: &str) -> String {
    let u = url.to_lowercase();
    let parts: Vec<&str> = u
        .split('/')
        .filter(|s| !s.is_empty() && !s.contains(':') && !s.contains("creativecommons"))
        .collect();
    match parts.as_slice() {
        ["licenses", kind, ver, ..] => format!("CC {} {ver}", kind.to_uppercase()),
        ["licenses", kind] => format!("CC {}", kind.to_uppercase()),
        ["publicdomain", "zero", ver, ..] => format!("CC0 {ver}"),
        ["publicdomain", "mark", ..] => "Public Domain Mark".into(),
        ["publicdomain", ..] => "Public domain".into(),
        _ if url.is_empty() => "Unknown licence".into(),
        _ => url.to_string(),
    }
}

/// What a licence URL allows. `None` when it is not a Creative Commons / PD URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lic {
    pub pd: bool,
    pub nc: bool,
    pub nd: bool,
    pub sa: bool,
}

pub fn lic(url: &str) -> Option<Lic> {
    let u = url.to_lowercase();
    if u.contains("/publicdomain/") {
        return Some(Lic {
            pd: true,
            nc: false,
            nd: false,
            sa: false,
        });
    }
    let kind = u.split("/licenses/").nth(1)?.split('/').next()?.to_string();
    if !kind.starts_with("by") && kind != "sa" {
        return None;
    }
    Some(Lic {
        pd: false,
        nc: kind.contains("nc"),
        nd: kind.contains("nd"),
        sa: kind.contains("sa"),
    })
}

/// Commercial use allowed (known CC licence without NC, or public domain).
pub fn commercial_ok(url: &str) -> bool {
    lic(url).is_some_and(|l| !l.nc)
}

/// Does `url` pass licence filter `i` (index into [`LICENCES`])?
pub fn lic_match(url: &str, i: usize) -> bool {
    let l = lic(url);
    match i {
        1 => l.is_some_and(|l| !l.nc),
        2 => l.is_some_and(|l| !l.nd),
        3 => l.is_some_and(|l| !l.pd && !l.nc && !l.nd && !l.sa),
        4 => l.is_some_and(|l| l.sa && !l.nc && !l.nd),
        5 => l.is_some_and(|l| l.pd),
        _ => true,
    }
}

/// One line on what the licence allows (detail pane).
pub fn lic_summary(url: &str) -> String {
    match lic(url) {
        None => "Licence unknown: check the source page before reusing.".into(),
        Some(l) if l.pd => "No rights reserved: use it for anything.".into(),
        Some(l) => {
            let mut v = vec!["Credit the artist"];
            v.push(if l.nc {
                "non-commercial use only"
            } else {
                "commercial use OK"
            });
            v.push(if l.nd {
                "no remixes / edits"
            } else if l.sa {
                "remixes must use the same licence"
            } else {
                "remixes OK"
            });
            v.join(" · ")
        }
    }
}

// ---------- query ----------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub text: String,
    /// Index into [`SOURCES`].
    pub source: usize,
    /// Index into [`CHIPS`] (browse list or genre); `None` = plain search.
    pub chip: Option<usize>,
    /// Index into [`MOODS`].
    pub mood: usize,
    pub lic: usize,
    pub len: usize,
    pub sort: usize,
    /// 0-based page.
    pub page: usize,
    /// Album page: (source, album id).
    pub album: Option<(&'static str, String)>,
    /// Artist page: (source, artist id or name).
    pub artist: Option<(&'static str, String)>,
}

impl Query {
    pub fn genre(&self) -> &'static str {
        self.chip.map(|c| CHIPS[c].1).unwrap_or("")
    }
    pub fn mood(&self) -> String {
        if self.mood == 0 {
            String::new()
        } else {
            MOODS[self.mood].to_lowercase()
        }
    }
    fn uses(&self, src: &'static str) -> bool {
        if let Some((s, _)) = self.album.as_ref().or(self.artist.as_ref()) {
            return *s == src || (src == IA && *s == FMA);
        }
        match self.source {
            0 => true,
            i => SOURCES[i] == src || (src == IA && SOURCES[i] == FMA),
        }
    }
    /// Seconds range of the length filter.
    pub fn len_range(&self) -> (u32, u32) {
        match self.len {
            1 => (0, 179),
            2 => (180, 360),
            3 => (361, 7200),
            _ => (0, 0),
        }
    }
}

/// Local filters that the APIs do not apply reliably (licence, length, downloads).
pub fn keep(t: &Track, q: &Query) -> bool {
    let (lo, hi) = q.len_range();
    // Jamendo's tag search is fuzzy: a genre chip keeps only tracks tagged with it.
    let g = q.genre();
    if !g.is_empty() && t.source == JAMENDO && !t.genres.is_empty() {
        let hit = |x: &String| x.contains(g) || (g == "hiphop" && x == "rap");
        if !t.genres.iter().chain(&t.tags).any(hit) {
            return false;
        }
    }
    lic_match(&t.license_url, q.lic)
        && (hi == 0 || t.duration == 0 || (lo..=hi).contains(&t.duration))
        && !(q.source == 3 && t.source != FMA)
}

/// Relevance of a track to `query`: whole-query and word hits in title and artist
/// count most, album and genre / tags less. 0 = no word matches.
pub fn score(t: &Track, query: &str) -> u32 {
    let q = norm(query);
    let (ti, ar, al) = (norm(&t.title), norm(&t.artist), norm(&t.album));
    let tags = t
        .genres
        .iter()
        .chain(&t.tags)
        .map(|g| g.to_lowercase())
        .collect::<Vec<_>>();
    let mut sc = 0;
    if !q.is_empty() && (ti.contains(&q) || ar.contains(&q)) {
        sc += 10;
    }
    if !q.is_empty() && (ti == q || ar == q) {
        sc += 6;
    }
    for w in q.split_whitespace().filter(|w| w.len() > 1) {
        let word = |s: &str| s.split_whitespace().any(|x| x == w);
        sc += 3 * ti.contains(w) as u32 + 3 * ar.contains(w) as u32 + al.contains(w) as u32;
        sc += word(&ti) as u32 + word(&ar) as u32;
        sc += 2 * tags.iter().any(|g| g == w) as u32;
    }
    sc
}

/// Merge per-source lists (each in its API order) into one list: deduplicated by
/// artist + title, filtered by [`keep`], sorted by `q.sort`. Best match = [`score`],
/// ties keep the API order and alternate sources.
pub fn merge(lists: &[Vec<Track>], q: &Query) -> Vec<Track> {
    let mut seen = HashSet::new();
    let mut v: Vec<(usize, Track)> = Vec::new();
    for l in lists {
        for (i, t) in l.iter().enumerate() {
            if keep(t, q) && seen.insert(t.key()) {
                v.push((i, t.clone()));
            }
        }
    }
    let text = q.text.trim();
    match q.sort {
        2 => v.sort_by(|a, b| b.1.date.cmp(&a.1.date).then(a.0.cmp(&b.0))),
        3 => v.sort_by_key(|(i, t)| {
            (
                if t.duration == 0 {
                    u32::MAX
                } else {
                    t.duration
                },
                *i,
            )
        }),
        4 => v.sort_by_key(|(i, t)| (std::cmp::Reverse(t.duration), *i)),
        0 if !text.is_empty() && q.album.is_none() => {
            let mut sc: Vec<(u32, usize, Track)> = v
                .into_iter()
                .map(|(i, t)| (score(&t, text), i, t))
                .collect();
            // Drop non-matching tracks once there are enough matching ones.
            if sc.iter().filter(|x| x.0 > 0).count() >= 5 {
                sc.retain(|x| x.0 > 0);
            }
            sc.sort_by_key(|x| (std::cmp::Reverse(x.0), x.1));
            return spread(sc.into_iter().map(|x| x.2).collect());
        }
        _ => v.sort_by_key(|(i, _)| *i),
    }
    let v: Vec<Track> = v.into_iter().map(|x| x.1).collect();
    if q.sort <= 1 && q.album.is_none() && q.artist.is_none() {
        spread(v)
    } else {
        v
    }
}

/// Keep the order but avoid runs of one artist: no more than two in a row; the extra
/// ones wait for the next free place (or go to the end).
pub fn spread(v: Vec<Track>) -> Vec<Track> {
    let mut out: Vec<Track> = Vec::with_capacity(v.len());
    let mut wait: VecDeque<Track> = VecDeque::new();
    let fits = |out: &[Track], t: &Track| {
        let a = norm(&t.artist);
        a.is_empty() || out.len() < 2 || out[out.len() - 2..].iter().any(|x| norm(&x.artist) != a)
    };
    for t in v {
        if let Some(i) = wait.iter().position(|w| fits(&out, w)) {
            let w = wait.remove(i).expect("index from position");
            out.push(w);
        }
        if fits(&out, &t) {
            out.push(t);
        } else {
            wait.push_back(t);
        }
    }
    out.extend(wait);
    out
}

fn s(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(a) => a.first().map(s).unwrap_or_default(),
        _ => String::new(),
    }
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Array(a) => a.iter().map(s).filter(|x| !x.is_empty()).collect(),
        serde_json::Value::String(x) => x
            .split([';', ','])
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// HTML entities Jamendo leaves in names ("That&#039;s").
pub fn unescape(s: &str) -> String {
    s.replace("&#039;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Remove a secret (the client id) from text that may be shown or logged.
fn scrub(e: String, secret: &str) -> String {
    if secret.is_empty() {
        e
    } else {
        e.replace(secret, "…")
    }
}

// ---------- Internet Archive ----------

const IA_ROWS: usize = 12;
const IA_TRACKS_PER_ITEM: usize = 8;
const IA_ALBUM_TRACKS: usize = 60;
/// Longest track kept (seconds); longer files are mixes, lectures or shows.
const IA_MAX_LEN: f64 = 20.0 * 60.0;
const IA_TIMEOUT: u64 = 12;

fn ia_clean(text: &str) -> String {
    let q: String = text
        .chars()
        .map(|c| {
            if "():\"\\[]{}^~*?!+-&|/".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    q.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Advanced-search query: music collections only, no spoken word, licence filter.
pub fn ia_query(q: &Query) -> String {
    let mut parts = Vec::new();
    let t = ia_clean(&q.text);
    if let Some((_, a)) = &q.artist {
        parts.push(format!("creator:(\"{}\")", ia_clean(a)));
    } else if !t.is_empty() {
        parts.push(format!(
            "(title:({t}) OR creator:({t}) OR subject:({t}) OR description:({t}))"
        ));
    }
    let g = q.genre();
    if !g.is_empty() {
        let alt = match g {
            "hiphop" => "hiphop OR \"hip hop\" OR rap",
            "lofi" => "lofi OR \"lo-fi\" OR chillhop",
            "soundtrack" => "soundtrack OR filmscore OR score",
            g => g,
        };
        // FMA items have no subjects; their genre is often only in the description.
        parts.push(format!("(subject:({alt}) OR description:({alt}))"));
    }
    let m = q.mood();
    if !m.is_empty() {
        parts.push(format!("(subject:({m}) OR description:({m}))"));
    }
    parts.push("mediatype:audio".into());
    parts.push(
        match q.lic {
            1 => "(licenseurl:*publicdomain* OR (licenseurl:*creativecommons* AND NOT licenseurl:*nc*))",
            2 => "(licenseurl:*publicdomain* OR (licenseurl:*creativecommons* AND NOT licenseurl:*nd*))",
            3 => "licenseurl:*licenses/by/*",
            4 => "licenseurl:*licenses/by-sa/*",
            5 => "licenseurl:*publicdomain*",
            _ => "(licenseurl:*creativecommons* OR licenseurl:*publicdomain*)",
        }
        .into(),
    );
    parts.push(if q.source == 3 {
        "collection:(freemusicarchive)".into()
    } else if t.is_empty() && q.artist.is_none() {
        // Browsing: curated netlabel / FMA collections only; the community upload
        // pool (opensource_audio) is dominated by game rips and mislabelled uploads.
        "collection:(netlabels OR freemusicarchive OR audio_music)".into()
    } else {
        "collection:(netlabels OR opensource_audio OR audio_music OR freemusicarchive OR 78rpm OR etree OR jamendo-albums)".into()
    });
    parts.push("NOT collection:(podcasts OR librivoxaudio OR oldtimeradio OR audio_bookspoetry OR radioprograms OR audio_religion OR audio_podcast OR audio_tech OR audio_news OR audio_foreign)".into());
    parts.push("NOT subject:(podcast OR lecture OR lectures OR sermon OR audiobook OR interview OR talk OR speech OR \"radio show\" OR episode OR philosophy OR quran OR koran OR bhajan OR kirtan OR \"sound effects\" OR sfx)".into());
    if q.chip == Some(2) {
        parts.push("collection:(netlabels)".into());
    }
    parts.join(" AND ")
}

/// Sort parameter of an IA search (empty = IA relevance).
pub fn ia_sort(q: &Query) -> &'static str {
    match (q.sort, q.chip) {
        (2, _) | (0, Some(1)) => "publicdate desc",
        (1, _) | (0, Some(0) | Some(2)) => "downloads desc",
        _ if q.text.trim().is_empty() => "downloads desc",
        _ => "",
    }
}

/// Item ids of a search page and whether more pages exist.
pub fn ia_ids(v: &serde_json::Value, page: usize) -> (Vec<(String, u64)>, bool) {
    let ids = v["response"]["docs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some((
                d["identifier"].as_str()?.to_string(),
                d["downloads"].as_u64().unwrap_or(0),
            ))
        })
        .collect();
    let found = v["response"]["numFound"].as_u64().unwrap_or(0) as usize;
    (ids, found > (page + 1) * IA_ROWS)
}

fn ia_search_ids(q: &Query) -> Result<(Vec<(String, u64)>, bool), String> {
    let query = ia_query(q);
    let rows = IA_ROWS.to_string();
    let page = (q.page + 1).to_string();
    let mut p = vec![
        ("q", query.as_str()),
        ("fl[]", "identifier"),
        ("fl[]", "downloads"),
        ("rows", rows.as_str()),
        ("page", page.as_str()),
        ("output", "json"),
    ];
    let sort = ia_sort(q);
    if !sort.is_empty() {
        p.push(("sort[]", sort));
    }
    let v = web::get_json_timeout("https://archive.org/advancedsearch.php", &p, IA, 20)?;
    Ok(ia_ids(&v, q.page))
}

fn ia_item(id: &str, max: usize, downloads: u64) -> Vec<Track> {
    web::get_json_timeout(
        &format!("https://archive.org/metadata/{}", urlencode(id)),
        &[],
        IA,
        IA_TIMEOUT,
    )
    .map(|m| {
        let mut v = ia_tracks(&m, max);
        for t in &mut v {
            t.popularity = downloads;
        }
        v
    })
    .unwrap_or_default()
}

/// Track length in seconds from an IA `length` field ("245.3" or "4:05").
fn ia_len(v: &serde_json::Value) -> Option<f64> {
    let l = s(v);
    if l.contains(':') {
        Some(
            l.split(':')
                .fold(0.0, |a, p| a * 60.0 + p.parse::<f64>().unwrap_or(0.0)),
        )
    } else {
        l.parse().ok()
    }
}

/// Drop upload noise from a title: "Song (ID: 396093)", "Song [www.site.com]".
pub fn clean_title(t: &str) -> String {
    let mut t = t.trim().to_string();
    for (open, close) in [('(', ')'), ('[', ']')] {
        if let (Some(a), true) = (t.rfind(open), t.ends_with(close)) {
            let inner = t[a + 1..t.len() - 1].to_lowercase();
            if a > 0
                && (inner.starts_with("id:") || inner.contains("www.") || inner.contains("http"))
            {
                t = t[..a].trim_end().to_string();
            }
        }
    }
    t
}

/// Title of a file name stem: drops a leading track number, separators and a trailing
/// "by <artist>" ("03_-_My_Song", "deep-and-dirty-by-kevin-macleod").
fn tidy_stem(stem: &str, artist: &str) -> String {
    let mut t = stem.replace('_', " ");
    if !t.contains(' ') {
        t = t.replace('-', " ");
    }
    let a = norm(artist);
    if !a.is_empty() {
        let low = t.to_lowercase();
        if let Some(i) = low.rfind(&format!(" by {a}")) {
            if i > 0 && low[i..].trim_end().len() == a.len() + 4 {
                t.truncate(i);
            }
        }
    }
    let t = t.as_str();
    let t = t.trim_start_matches(|c: char| c.is_ascii_digit());
    let t = t.trim_start_matches(|c: char| " -.".contains(c));
    if t.is_empty() {
        stem.to_string()
    } else {
        t.trim().to_string()
    }
}

/// MP3 tracks of an Internet Archive item (`/metadata/<id>` JSON), at most `max`.
pub fn ia_tracks(m: &serde_json::Value, max: usize) -> Vec<Track> {
    let md = &m["metadata"];
    let id = s(&md["identifier"]);
    let lic = s(&md["licenseurl"]);
    if id.is_empty() || lic.is_empty() {
        return Vec::new();
    }
    let files: Vec<&serde_json::Value> = m["files"].as_array().into_iter().flatten().collect();
    let fmt = |f: &serde_json::Value| s(&f["format"]);
    // Prefer VBR MP3 derivatives, else any MP3.
    let short = |f: &&&serde_json::Value| ia_len(&f["length"]).is_none_or(|l| l <= IA_MAX_LEN);
    let mut mp3: Vec<&&serde_json::Value> = files
        .iter()
        .filter(|f| fmt(f) == "VBR MP3")
        .filter(short)
        .collect();
    if mp3.is_empty() {
        mp3 = files
            .iter()
            .filter(|f| fmt(f).contains("MP3"))
            .filter(short)
            .collect();
    }
    mp3.sort_by_key(|f| {
        s(&f["track"])
            .split('/')
            .next()
            .and_then(|n| n.trim().parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    let fma = strings(&md["collection"])
        .iter()
        .any(|c| c == "freemusicarchive");
    let item_artist = s(&md["creator"]);
    let subjects: Vec<String> = strings(&md["subject"])
        .into_iter()
        .flat_map(|x| {
            x.split([';', ','])
                .map(|p| p.trim().to_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|x| !x.is_empty() && x.len() < 30)
        .take(8)
        .collect();
    let dl = |name: &str| {
        format!(
            "https://archive.org/download/{}/{}",
            urlencode(&id),
            name.split('/').map(urlencode).collect::<Vec<_>>().join("/")
        )
    };
    // Item artwork: a JPEG/PNG original (cover.jpg, folder.jpg, …), else the thumbnail.
    let thumb = format!("https://archive.org/services/img/{}", urlencode(&id));
    let art = files
        .iter()
        .filter(|f| {
            let f = fmt(f);
            f == "JPEG" || f == "PNG" || f == "Item Image"
        })
        .max_by_key(|f| {
            let n = s(&f["name"]).to_lowercase();
            (n.contains("cover") || n.contains("folder") || n.contains("front")) as u8
        })
        .map(|f| dl(&s(&f["name"])))
        .unwrap_or_else(|| thumb.clone());
    mp3.into_iter()
        .take(max)
        .map(|f| {
            let name = s(&f["name"]);
            let url = dl(&name);
            let stem = Path::new(&name)
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            let title = s(&f["title"]);
            let artist = s(&f["artist"]);
            let artist = if artist.is_empty() {
                item_artist.clone()
            } else {
                artist
            };
            let mut genres = strings(&f["genre"]);
            genres.retain(|g| !g.eq_ignore_ascii_case("other"));
            let title = if title.is_empty() {
                tidy_stem(&stem, &artist)
            } else {
                title
            };
            Track {
                id: format!("{id}/{name}"),
                title: clean_title(&title),
                artist_id: artist.clone(),
                artist,
                album: {
                    let a = s(&f["album"]);
                    if a.is_empty() {
                        s(&md["title"])
                    } else {
                        a
                    }
                },
                album_id: id.clone(),
                date: s(&md["date"]).chars().take(10).collect(),
                duration: ia_len(&f["length"]).unwrap_or(0.0).round() as u32,
                genres: genres.into_iter().map(|g| g.to_lowercase()).collect(),
                tags: subjects.clone(),
                license_url: lic.clone(),
                stream: url.clone(),
                download: url,
                cover: thumb.clone(),
                cover_full: art.clone(),
                page: format!("https://archive.org/details/{}", urlencode(&id)),
                popularity: 0,
                source: if fma { FMA } else { IA },
            }
        })
        .collect()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

// ---------- Jamendo ----------

/// How a Jamendo text search asks: full-text, track-name prefix, exact artist name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JMode {
    Search,
    Name,
    Artist,
    Browse,
}

/// Query parameters of a Jamendo `/tracks` request (without the client id).
pub fn jamendo_params(q: &Query, mode: JMode) -> Vec<(&'static str, String)> {
    let mut p: Vec<(&'static str, String)> = vec![
        ("format", "json".into()),
        ("limit", PAGE.to_string()),
        ("offset", (q.page * PAGE).to_string()),
        ("include", "musicinfo".into()),
        ("audioformat", "mp32".into()),
        ("audiodlformat", "mp32".into()),
        ("imagesize", "300".into()),
    ];
    let text = q.text.trim().to_string();
    if let Some((_, a)) = &q.album {
        // Jamendo lists album tracks in album order by default ("position" is no order).
        p.push(("album_id", a.clone()));
        return p;
    }
    if let Some((_, a)) = &q.artist {
        p.push(("artist_id", a.clone()));
    }
    match mode {
        JMode::Search => p.push(("search", text)),
        JMode::Name => p.push(("namesearch", text)),
        JMode::Artist => p.push(("artist_name", text)),
        JMode::Browse => {}
    }
    // Genre in `fuzzytags`, mood in `vartags` (both together: jazz AND relaxing).
    if !q.genre().is_empty() {
        p.push(("fuzzytags", q.genre().to_string()));
    }
    if !q.mood().is_empty() {
        p.push(("vartags", q.mood()));
    }
    if q.chip == Some(2) {
        p.push(("featured", "1".into()));
    }
    match q.lic {
        1 => p.push(("ccnc", "false".into())),
        2 => p.push(("ccnd", "false".into())),
        3 => {
            p.push(("ccnc", "false".into()));
            p.push(("ccnd", "false".into()));
            p.push(("ccsa", "false".into()));
        }
        4 => {
            p.push(("ccnc", "false".into()));
            p.push(("ccnd", "false".into()));
            p.push(("ccsa", "true".into()));
        }
        _ => {}
    }
    let (lo, hi) = q.len_range();
    if hi > 0 {
        p.push(("durationbetween", format!("{lo}_{hi}")));
    }
    let order = match (q.sort, q.chip) {
        (1, _) => "popularity_total",
        (2, _) | (0, Some(1)) => "releasedate_desc",
        (3, _) => "duration",
        (4, _) => "duration_desc",
        _ if mode == JMode::Browse || q.artist.is_some() => "popularity_month",
        _ => "",
    };
    if !order.is_empty() {
        p.push(("order", order.into()));
    }
    p
}

fn jamendo_get(key: &str, params: &[(&'static str, String)]) -> Result<Vec<Track>, String> {
    let mut p: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    p.push(("client_id", key));
    // Jamendo sometimes answers an empty page for a query that has results; one retry.
    let mut last = Vec::new();
    for attempt in 0..3 {
        let v = web::get_json_timeout("https://api.jamendo.com/v3.0/tracks/", &p, JAMENDO, 15)
            .map_err(|e| scrub(e, key))?;
        if v["headers"]["status"] != "success" {
            return Err(format!(
                "Jamendo: {}",
                v["headers"]["error_message"]
                    .as_str()
                    .filter(|m| !m.is_empty())
                    .map(|m| scrub(m.to_string(), key))
                    .unwrap_or("request failed (check the client ID)".into())
            ));
        }
        last = jamendo_tracks(&v);
        if !last.is_empty() || attempt == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(250 * (attempt + 1)));
    }
    Ok(last)
}

/// One Jamendo page for `q`; text searches ask three ways in parallel and merge.
pub fn jamendo_search(key: &str, q: &Query) -> Result<(Vec<Track>, bool), String> {
    let modes: &[JMode] = if q.text.trim().is_empty() || q.album.is_some() {
        &[JMode::Browse]
    } else if q.page > 0 {
        &[JMode::Search, JMode::Name]
    } else {
        &[JMode::Search, JMode::Name, JMode::Artist]
    };
    let hs: Vec<_> = modes
        .iter()
        .map(|m| {
            let (key, p) = (key.to_string(), jamendo_params(q, *m));
            std::thread::spawn(move || jamendo_get(&key, &p))
        })
        .collect();
    let mut out: Vec<Track> = Vec::new();
    let mut seen = HashSet::new();
    let mut err = None;
    let mut more = false;
    // Interleave so that every way of asking contributes its best hits first.
    let lists: Vec<Vec<Track>> = hs
        .into_iter()
        .filter_map(
            |h| match h.join().unwrap_or_else(|_| Err("Jamendo: failed".into())) {
                Ok(v) => {
                    more |= v.len() >= PAGE;
                    Some(v)
                }
                Err(e) => {
                    err = Some(e);
                    None
                }
            },
        )
        .collect();
    for i in 0..PAGE {
        for l in &lists {
            if let Some(t) = l.get(i) {
                if seen.insert(t.id.clone()) {
                    out.push(t.clone());
                }
            }
        }
    }
    match (out.is_empty(), err) {
        (true, Some(e)) => Err(e),
        _ => Ok((out, more)),
    }
}

pub fn jamendo_tracks(v: &serde_json::Value) -> Vec<Track> {
    v["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let mi = &r["musicinfo"]["tags"];
            let image = s(&r["image"]);
            Track {
                id: s(&r["id"]),
                title: unescape(&s(&r["name"])),
                artist: unescape(&s(&r["artist_name"])),
                artist_id: s(&r["artist_id"]),
                album: unescape(&s(&r["album_name"])),
                album_id: s(&r["album_id"]),
                date: s(&r["releasedate"]),
                duration: r["duration"].as_u64().unwrap_or(0) as u32,
                genres: strings(&mi["genres"]),
                tags: strings(&mi["vartags"]),
                license_url: s(&r["license_ccurl"]),
                stream: s(&r["audio"]),
                download: if r["audiodownload_allowed"].as_bool() == Some(false) {
                    String::new()
                } else {
                    s(&r["audiodownload"])
                },
                cover_full: if image.contains("width=") {
                    image.replace("width=300", "width=600")
                } else {
                    image.clone()
                },
                cover: image,
                page: s(&r["shareurl"]),
                popularity: 0,
                source: JAMENDO,
            }
        })
        .filter(|t| !t.stream.is_empty())
        .collect()
}

// ---------- similar ----------

/// Query for tracks like `t`: its genre and first mood tag, popular first, same source.
pub fn similar_query(t: &Track) -> Query {
    let genre = t.genres.iter().chain(&t.tags).find_map(|g| {
        CHIPS
            .iter()
            .position(|c| !c.1.is_empty() && c.1 == g.as_str())
    });
    let mood = t
        .tags
        .iter()
        .find_map(|g| MOODS.iter().skip(1).position(|m| m.eq_ignore_ascii_case(g)))
        .map(|i| i + 1)
        .unwrap_or(0);
    let text = if genre.is_none() {
        t.genres
            .iter()
            .chain(&t.tags)
            .next()
            .cloned()
            .unwrap_or_else(|| t.artist.clone())
    } else {
        String::new()
    };
    Query {
        text,
        source: SOURCES.iter().position(|x| *x == t.source).unwrap_or(0),
        chip: genre,
        mood,
        sort: 1,
        ..Default::default()
    }
}

// ---------- download ----------

/// Target file of a download: `dir/<Artist>/<Artist> - <Title>.mp3`.
pub fn target(t: &Track, dir: &Path) -> PathBuf {
    let artist = if t.artist.is_empty() {
        "Unknown artist"
    } else {
        &t.artist
    };
    dir.join(web::safe_name(artist)).join(format!(
        "{} - {}.mp3",
        web::safe_name(artist),
        web::safe_name(&t.title)
    ))
}

/// Tags written to a download: licence and attribution in copyright / licence / comment.
pub fn tags_for(t: &Track) -> Tags {
    let year: String = t.date.chars().take(4).collect();
    Tags {
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        album_artist: t.artist.clone(),
        date: t.date.clone(),
        genre: t
            .genres
            .iter()
            .take(3)
            .map(|g| {
                let mut c = g.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("; "),
        copyright: format!(
            "{year} {}. Licensed under {}: {}",
            t.artist,
            t.license(),
            t.license_url
        )
        .trim()
        .to_string(),
        license: t.license_url.clone(),
        comment: t.attribution(),
        ..Default::default()
    }
}

/// Text of the `<file>.licence.txt` next to a download.
pub fn licence_text(t: &Track) -> String {
    format!(
        "{}\n\nTitle: {}\nArtist: {}\nAlbum: {}\nSource: {}\nLicence: {} — {}\n{}\n",
        t.attribution(),
        t.title,
        t.artist,
        t.album,
        t.page,
        t.license(),
        t.license_url,
        lic_summary(&t.license_url)
    )
}

/// Download a track into [`target`] with tags, cover and a licence file; `progress`
/// gets per-mille progress. Returns the saved path.
pub fn download(t: &Track, dir: &Path, progress: &AtomicU32) -> Result<PathBuf, String> {
    use std::io::{Read, Write};
    if t.download.is_empty() {
        return Err("The artist does not allow downloads of this track (preview only)".into());
    }
    let out = target(t, dir);
    std::fs::create_dir_all(out.parent().unwrap_or(dir)).map_err(|e| e.to_string())?;
    let mut r = web::agent_with_timeout(600)
        .get(&t.download)
        .call()
        .map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("download failed ({})", r.status()));
    }
    let total: u64 = r
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // lofty picks the format from the extension, so the partial file ends in .mp3.
    let tmp = out.with_extension("part.mp3");
    let res = (|| -> Result<(), String> {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut rd = r.body_mut().with_config().limit(300 * 1024 * 1024).reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0u64;
        loop {
            let n = rd.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            got += n as u64;
            if let Some(pm) = (got * 1000).checked_div(total) {
                progress.store(pm.min(999) as u32, Ordering::Relaxed);
            }
        }
        if got == 0 {
            return Err("empty download".into());
        }
        let cover_url = if t.cover_full.is_empty() {
            &t.cover
        } else {
            &t.cover_full
        };
        let cover = (!cover_url.is_empty())
            .then(|| web::get_bytes(cover_url, 10 * 1024 * 1024).ok().flatten())
            .flatten()
            .or_else(|| {
                (cover_url != &t.cover && !t.cover.is_empty())
                    .then(|| web::get_bytes(&t.cover, 10 * 1024 * 1024).ok().flatten())
                    .flatten()
            });
        tagger::write_tags(&tmp, &tags_for(t), cover.as_deref())?;
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())
    })();
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let _ = std::fs::write(out.with_extension("licence.txt"), licence_text(t));
    progress.store(1000, Ordering::Relaxed);
    Ok(out)
}

// ---------- plugin ----------

enum Msg {
    /// Tracks of one source for search `gen`; `done` = that source finished.
    Page {
        gen: u64,
        src: &'static str,
        pos: usize,
        tracks: Vec<Track>,
        more: bool,
        done: bool,
    },
    Fail {
        gen: u64,
        src: &'static str,
        err: String,
    },
    Cover(String, Option<PathBuf>),
    Saved(String, Result<PathBuf, String>),
}

/// Which way the downloads go ([`Manifest`] setting `downloader`).
const DOWNLOADERS: [&str; 2] = ["Built-in (tags, cover, licence)", "Offline Downloads queue"];
const RADIO: usize = 12;
const DEBOUNCE: Duration = Duration::from_millis(450);

pub struct FreeMusic {
    q: Query,
    gen: u64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Tracks of earlier pages (fixed order, infinite scroll appends below).
    base: Vec<Track>,
    /// Current page per source, in API order.
    /// Current page per source: (position in the API answer, tracks).
    page: HashMap<&'static str, std::collections::BTreeMap<usize, Vec<Track>>>,
    pending: HashSet<&'static str>,
    /// "Play similar" in progress: (search gen, keys already queued).
    radio: Option<(u64, HashSet<String>)>,
    more: HashMap<&'static str, bool>,
    errors: Vec<String>,
    tracks: Vec<Track>,
    sel: Option<String>,
    covers: HashMap<String, slint::Image>,
    cover_asked: HashSet<String>,
    covers_dirty: bool,
    typed: Option<Instant>,
    /// Queries to go back to from album / artist pages.
    back: Vec<Query>,
    title: String,
    dl_queue: VecDeque<Track>,
    dl_active: Option<(Track, Arc<AtomicU32>)>,
    dl_done: usize,
    dl_last: String,
    note: String,
}

impl Default for FreeMusic {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            q: Query::default(),
            gen: 0,
            tx,
            rx,
            base: Vec::new(),
            page: HashMap::new(),
            pending: HashSet::new(),
            radio: None,
            more: HashMap::new(),
            errors: Vec::new(),
            tracks: Vec::new(),
            sel: None,
            covers: HashMap::new(),
            cover_asked: HashSet::new(),
            covers_dirty: false,
            typed: None,
            back: Vec::new(),
            title: String::new(),
            dl_queue: VecDeque::new(),
            dl_active: None,
            dl_done: 0,
            dl_last: String::new(),
            note: String::new(),
        }
    }
}

/// Jamendo client id: the setting, else `JAMENDO_CLIENT_ID` from the environment, else
/// one baked in at build time (`VIVI_JAMENDO_CLIENT_ID`). Never a client secret.
fn jamendo_key(cx: &Ctx) -> String {
    let k = cx.get("jamendo-client-id").trim().to_string();
    if !k.is_empty() {
        return k;
    }
    std::env::var("JAMENDO_CLIENT_ID")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .or(option_env!("VIVI_JAMENDO_CLIENT_ID").map(String::from))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Cached list thumbnail (`free-music-covers/<hash>.jpg|png`); `None` when missing.
fn fetch_cover(url: &str) -> Option<PathBuf> {
    let dir = crate::store::cache_dir().join("free-music-covers");
    let base = dir.join(super::streams::hash_name(url));
    for ext in ["jpg", "png"] {
        let p = base.with_extension(ext);
        if p.is_file() {
            return Some(p);
        }
    }
    if base.with_extension("none").is_file() {
        return None;
    }
    std::fs::create_dir_all(&dir).ok()?;
    let b = web::get_bytes(url, 4 * 1024 * 1024).ok().flatten();
    // Slint picks the decoder from the extension.
    let ext = match b.as_deref() {
        Some([0x89, b'P', b'N', b'G', ..]) => "png",
        Some([0xFF, 0xD8, ..]) => "jpg",
        _ => {
            let _ = std::fs::write(base.with_extension("none"), b"");
            return None;
        }
    };
    let p = base.with_extension(ext);
    std::fs::write(&p, b?).ok()?;
    Some(p)
}

/// The Internet Archive's "no image" thumbnail is a wide banner; album art is ~square.
fn is_cover(img: &slint::Image) -> bool {
    let s = img.size();
    s.width > 0 && s.height > 0 && s.width < 2 * s.height
}

impl FreeMusic {
    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app
            .global::<crate::MusicDialogState>()
            .set_status(status.into());
        cx.app.global::<crate::MusicDialogState>().set_busy(busy);
    }
    fn folder(cx: &Ctx) -> PathBuf {
        let f = cx.get("folder");
        let f = f.trim();
        if f.is_empty() {
            crate::store::home().join("Music").join("Free Music")
        } else if let Some(r) = f.strip_prefix("~/") {
            crate::store::home().join(r)
        } else {
            PathBuf::from(f)
        }
    }
    fn selected(&self) -> Option<&Track> {
        let id = self.sel.as_ref()?;
        self.tracks.iter().find(|t| &t.id == id)
    }
    fn at(&self, args: &[String]) -> Option<Track> {
        let i = args.first()?.parse::<usize>().ok()?;
        self.tracks.get(i).cloned()
    }

    /// Start (or continue, for page > 0) the search for `self.q`.
    fn run(&mut self, cx: &Ctx) {
        self.gen += 1;
        self.typed = None;
        let gen = self.gen;
        if self.q.page == 0 {
            self.base.clear();
            self.tracks.clear();
            self.sel = None;
            cx.app
                .global::<crate::MusicDialogState>()
                .set_detail("".into());
            cx.app
                .global::<crate::MusicDialogState>()
                .set_copy("".into());
        } else {
            self.base = self.tracks.clone();
        }
        self.page.clear();
        self.pending.clear();
        self.errors.clear();
        self.note.clear();
        let key = jamendo_key(cx);
        let q = self.q.clone();
        if q.uses(JAMENDO) {
            if key.is_empty() {
                if q.source == 1 || q.album.is_some() || q.artist.is_some() {
                    self.show(cx);
                    Self::ui(cx, NO_KEY, false);
                    return;
                }
                self.note = NO_KEY.into();
            } else if self.more.get(JAMENDO).copied().unwrap_or(true) || q.page == 0 {
                self.pending.insert(JAMENDO);
                let (tx, q, key) = (self.tx.clone(), q.clone(), key.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(match jamendo_search(&key, &q) {
                        Ok((tracks, more)) => Msg::Page {
                            gen,
                            src: JAMENDO,
                            pos: 0,
                            tracks,
                            more,
                            done: true,
                        },
                        Err(err) => Msg::Fail {
                            gen,
                            src: JAMENDO,
                            err,
                        },
                    });
                });
            }
        }
        if q.uses(IA) && (self.more.get(IA).copied().unwrap_or(true) || q.page == 0) {
            self.pending.insert(IA);
            let (tx, q) = (self.tx.clone(), q.clone());
            std::thread::spawn(move || ia_job(gen, &q, &tx));
        }
        if q.page == 0 {
            self.more.clear();
        }
        self.show(cx);
        self.status(cx);
    }

    fn status(&self, cx: &Ctx) {
        if let Some((t, p)) = &self.dl_active {
            let pct = p.load(Ordering::Relaxed) / 10;
            let left = self.dl_queue.len();
            let s = format!(
                "Downloading “{}” {pct}%{}",
                t.title,
                if left > 0 {
                    format!(" · {left} more queued")
                } else {
                    String::new()
                }
            );
            Self::ui(cx, &s, !self.pending.is_empty());
            return;
        }
        let loading: Vec<&str> = [JAMENDO, IA]
            .into_iter()
            .filter(|s| self.pending.contains(s))
            .collect();
        let n = self.tracks.len();
        let mut s = if !loading.is_empty() {
            format!(
                "{}Searching {}…",
                if n > 0 {
                    format!("{n} tracks · ")
                } else {
                    String::new()
                },
                loading.join(" and ")
            )
        } else if n == 0 && !self.errors.is_empty() {
            self.errors.join(" · ")
        } else if n == 0 {
            "Nothing found. Try other words, another genre or fewer filters.".into()
        } else {
            let more = self.more.values().any(|m| *m);
            format!(
                "{n} tracks{}. Double-click plays; Download saves with tags and the licence.",
                if more { " (scroll for more)" } else { "" }
            )
        };
        if n > 0 && !self.errors.is_empty() && loading.is_empty() {
            s = format!("{s}  ({})", self.errors.join(" · "));
        }
        if !self.note.is_empty() && loading.is_empty() {
            s = format!("{s}  {}", self.note);
        }
        if !self.dl_last.is_empty() && loading.is_empty() {
            s = format!("{} · {s}", self.dl_last);
        }
        Self::ui(cx, &s, !loading.is_empty());
    }

    fn remerge(&mut self) {
        let order = [JAMENDO, IA];
        let lists: Vec<Vec<Track>> = order
            .iter()
            .filter_map(|s| self.page.get(s))
            .map(|m| m.values().flatten().cloned().collect())
            .collect();
        let fresh = merge(&lists, &self.q);
        let seen: HashSet<String> = self.base.iter().map(|t| t.key()).collect();
        self.tracks = self.base.clone();
        self.tracks
            .extend(fresh.into_iter().filter(|t| !seen.contains(&t.key())));
    }

    fn show(&mut self, cx: &Ctx) {
        let sel = self.sel.clone();
        let rows: Vec<crate::FmRow> = self
            .tracks
            .iter()
            .map(|t| {
                let img = self.covers.get(&t.cover).cloned();
                crate::FmRow {
                    title: t.title.clone().into(),
                    sub: [t.artist.as_str(), t.album.as_str()]
                        .iter()
                        .filter(|x| !x.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" · ")
                        .into(),
                    badge: match t.source {
                        JAMENDO => "Jamendo",
                        FMA => "FMA",
                        _ => "Archive",
                    }
                    .into(),
                    lic: t.license().replace("Unknown licence", "?").into(),
                    free: commercial_ok(&t.license_url),
                    dur: t.length().into(),
                    has_cover: img.is_some(),
                    cover: img.unwrap_or_default(),
                    dl: !t.download.is_empty(),
                    selected: sel.as_ref() == Some(&t.id),
                }
            })
            .collect();
        let items: Vec<crate::Item> = self
            .tracks
            .iter()
            .map(|t| crate::Item {
                key: t.title.clone().into(),
                label: t.artist.clone().into(),
                selected: sel.as_ref() == Some(&t.id),
            })
            .collect();
        cx.app
            .global::<crate::MusicDialogState>()
            .set_tracks(ModelRc::new(VecModel::from(rows)));
        cx.app
            .global::<crate::MusicDialogState>()
            .set_results(ModelRc::new(VecModel::from(items)));
        cx.app.global::<crate::MusicDialogState>().set_has_more(
            self.pending.is_empty() && self.more.values().any(|m| *m) && !self.tracks.is_empty(),
        );
        self.want_covers();
        self.buttons(cx);
    }

    fn want_covers(&mut self) {
        let want: Vec<String> = self
            .tracks
            .iter()
            .map(|t| t.cover.clone())
            .filter(|c| c.starts_with("http") && self.cover_asked.insert(c.clone()))
            .collect();
        if want.is_empty() {
            return;
        }
        // A few parallel fetchers per batch; results trickle in through the channel.
        let chunk = want.len().div_ceil(4).max(1);
        for part in want.chunks(chunk) {
            let (tx, part) = (self.tx.clone(), part.to_vec());
            std::thread::spawn(move || {
                for u in part {
                    let p = fetch_cover(&u);
                    if tx.send(Msg::Cover(u, p)).is_err() {
                        break;
                    }
                }
            });
        }
    }

    fn buttons(&self, cx: &Ctx) {
        let mut acts: Vec<SharedString> = Vec::new();
        if !self.back.is_empty() {
            acts.push("‹ Back".into());
        }
        if let Some(t) = self.selected() {
            acts.push("Queue".into());
            acts.push("Similar".into());
            if !t.album_id.is_empty() {
                acts.push("Album".into());
            }
            if !t.artist_id.is_empty() {
                acts.push("Artist".into());
            }
            acts.push("Copy credit".into());
            acts.push("Web page".into());
        }
        cx.app
            .global::<crate::MusicDialogState>()
            .set_actions(ModelRc::new(VecModel::from(acts)));
        cx.app.global::<crate::MusicDialogState>().set_secondary(
            match self.selected() {
                Some(t) if t.download.is_empty() => "Preview only",
                _ => "Download",
            }
            .into(),
        );
    }

    fn filters(&self, cx: &Ctx) {
        let f = |name: &str, opts: &[&str], i: usize| crate::MdFilter {
            name: name.into(),
            options: ModelRc::new(VecModel::from(
                opts.iter()
                    .map(|o| SharedString::from(*o))
                    .collect::<Vec<_>>(),
            )),
            value: opts[i.min(opts.len() - 1)].into(),
        };
        let v = vec![
            f("mood", &MOODS, self.q.mood),
            f("licence", &LICENCES, self.q.lic),
            f("length", &LENGTHS, self.q.len),
            f("sort", &SORTS, self.q.sort),
        ];
        cx.app
            .global::<crate::MusicDialogState>()
            .set_filters(ModelRc::new(VecModel::from(v)));
        let chips: Vec<SharedString> = CHIPS.iter().map(|c| c.0.into()).collect();
        cx.app
            .global::<crate::MusicDialogState>()
            .set_chips(ModelRc::new(VecModel::from(chips)));
        cx.app.global::<crate::MusicDialogState>().set_chip(
            self.q
                .chip
                .filter(|_| self.q.album.is_none() && self.q.artist.is_none())
                .map(|c| CHIPS[c].0)
                .unwrap_or("")
                .into(),
        );
        cx.app
            .global::<crate::MusicDialogState>()
            .set_title(self.title_text().into());
    }

    fn title_text(&self) -> String {
        if self.title.is_empty() {
            "Free Music".into()
        } else {
            format!("Free Music  ›  {}", self.title)
        }
    }

    fn detail(&self, t: &Track, cx: &Ctx) -> String {
        let mut d = format!("{}\n{}", t.title, t.artist);
        if !t.album.is_empty() {
            d += &format!("\nAlbum: {}", t.album);
        }
        let mut meta = Vec::new();
        if !t.date.is_empty() {
            meta.push(t.date.clone());
        }
        if t.duration > 0 {
            meta.push(t.length());
        }
        meta.push(t.source.to_string());
        d += &format!("\n{}", meta.join(" · "));
        let tags: Vec<&str> = t
            .genres
            .iter()
            .chain(&t.tags)
            .map(String::as_str)
            .take(8)
            .collect();
        if !tags.is_empty() {
            d += &format!("\nTags: {}", tags.join(", "));
        }
        d += &format!(
            "\n\nLicence: {}\n{}\n{}",
            t.license(),
            lic_summary(&t.license_url),
            t.license_url
        );
        d += &format!(
            "\n\nAttribution (Copy credit; also saved in the tags and a .licence.txt):\n{}",
            t.attribution()
        );
        d += &if t.download.is_empty() {
            "\n\nThe artist allows streaming only, no download.".to_string()
        } else {
            format!("\n\nDownloads to {}", Self::folder(cx).display())
        };
        d
    }

    fn select(&mut self, cx: &Ctx, t: &Track) {
        self.sel = Some(t.id.clone());
        cx.app
            .global::<crate::MusicDialogState>()
            .set_detail(self.detail(t, cx).into());
        cx.app
            .global::<crate::MusicDialogState>()
            .set_copy(t.attribution().into());
        self.buttons(cx);
    }

    fn open(&mut self, cx: &Ctx) {
        let a = cx.app;
        cx.app.global::<crate::Overlays>().set_settings_open(false);
        a.global::<crate::MusicDialogState>().set_owner(ID.into());
        a.global::<crate::MusicDialogState>()
            .set_caps(crate::MdCaps {
                logos: false,
                chips: true,
                rich: true,
            });
        a.global::<crate::MusicDialogState>()
            .set_placeholder("Search songs, artists, albums, genres…".into());
        a.global::<crate::MusicDialogState>()
            .set_search_label("Search".into());
        a.global::<crate::MusicDialogState>()
            .set_primary("Play".into());
        let src: Vec<SharedString> = SOURCES.iter().map(|s| (*s).into()).collect();
        a.global::<crate::MusicDialogState>()
            .set_sources(ModelRc::new(VecModel::from(src)));
        a.global::<crate::MusicDialogState>()
            .set_source(SOURCES[self.q.source].into());
        a.global::<crate::MusicDialogState>().set_detail("".into());
        a.global::<crate::Overlays>().set_md_open(true);
        self.back.clear();
        self.title.clear();
        self.q.album = None;
        self.q.artist = None;
        if self.q.text.trim().is_empty() && self.q.chip.is_none() {
            self.q.chip = Some(0);
        }
        a.global::<crate::MusicDialogState>()
            .set_query(self.q.text.clone().into());
        self.q.page = 0;
        self.filters(cx);
        self.run(cx);
    }

    /// Read the query box and the source dropdown into `self.q` and search page 0.
    fn search(&mut self, cx: &Ctx) {
        let text = cx
            .app
            .global::<crate::MusicDialogState>()
            .get_query()
            .to_string();
        let src = cx.app.global::<crate::MusicDialogState>().get_source();
        self.q.source = SOURCES.iter().position(|s| *s == src.as_str()).unwrap_or(0);
        self.q.text = text;
        self.q.page = 0;
        self.q.album = None;
        self.q.artist = None;
        self.back.clear();
        self.title.clear();
        if !self.q.text.trim().is_empty() && self.q.chip.is_some_and(|c| c < 3) {
            // A text search replaces the Popular / New / Featured lists.
            self.q.chip = None;
        }
        if self.q.text.trim().is_empty() && self.q.chip.is_none() {
            self.q.chip = Some(0);
        }
        self.filters(cx);
        self.run(cx);
    }

    fn page_view(&mut self, cx: &Ctx, title: String, q: Query) {
        self.back.push(self.q.clone());
        self.title = title;
        self.q = q;
        self.filters(cx);
        self.run(cx);
    }

    fn play(&mut self, cx: &Ctx, t: &Track, queue: bool) {
        crate::loading::set_name(&t.stream, &format!("{} — {}", t.artist, t.title));
        if queue {
            cx.player.enqueue(&t.stream);
            cx.toast(&format!("Added {} — {} to the queue", t.artist, t.title));
        } else {
            cx.player.cmd("loadfile", &[&t.stream]);
            cx.toast(&format!("Playing {} — {}", t.artist, t.title));
        }
    }

    /// "Play similar": play this track, then queue similar ones as they arrive.
    fn similar(&mut self, cx: &Ctx, t: &Track) {
        self.play(cx, t, false);
        let mut q = similar_query(t);
        if q.source == 3 {
            q.source = 2;
        }
        let title = format!("Similar to {}", t.title);
        self.page_view(cx, title, q);
        // Radio: the first results that arrive are queued after this track.
        self.radio = Some((self.gen, HashSet::from([t.key()])));
    }

    /// Queue up to [`RADIO`] tracks of the radio search as results come in.
    fn feed_radio(&mut self, cx: &Ctx) {
        let Some((gen, seen)) = &mut self.radio else {
            return;
        };
        if *gen != self.gen {
            self.radio = None;
            return;
        }
        let mut n = 0;
        for t in &self.tracks {
            if seen.len() > RADIO {
                break;
            }
            if seen.insert(t.key()) {
                crate::loading::set_name(&t.stream, &format!("{} — {}", t.artist, t.title));
                cx.player.enqueue(&t.stream);
                n += 1;
            }
        }
        if n > 0 {
            self.note = format!("Queued {} similar tracks.", seen.len() - 1);
        }
    }

    fn download(&mut self, cx: &Ctx, t: Track) {
        if t.download.is_empty() {
            Self::ui(
                cx,
                "The artist does not allow downloads of this track (preview only)",
                false,
            );
            return;
        }
        if cx.get("downloader") == DOWNLOADERS[1] {
            if let Some(script) = cx.package_script("offline-downloads") {
                cx.player
                    .cmd("script-message-to", &[&script, "enqueue", &t.download]);
                cx.toast(&format!(
                    "Sent “{}” to Offline Downloads (credit: Copy credit)",
                    t.title
                ));
                return;
            }
            self.note = "Offline Downloads is not enabled; using the built-in downloader.".into();
        }
        if target(&t, &Self::folder(cx)).is_file() {
            cx.toast(&format!("“{}” is already downloaded", t.title));
            return;
        }
        if self.dl_queue.iter().any(|x| x.id == t.id)
            || self.dl_active.as_ref().is_some_and(|(x, _)| x.id == t.id)
        {
            cx.toast("Already in the download queue");
            return;
        }
        self.dl_queue.push_back(t);
        self.next_download(cx);
        self.status(cx);
    }

    fn next_download(&mut self, cx: &Ctx) {
        if self.dl_active.is_some() {
            return;
        }
        let Some(t) = self.dl_queue.pop_front() else {
            return;
        };
        let p = Arc::new(AtomicU32::new(0));
        let (tx, dir, t2, p2) = (self.tx.clone(), Self::folder(cx), t.clone(), p.clone());
        std::thread::spawn(move || {
            let r = download(&t2, &dir, &p2);
            let _ = tx.send(Msg::Saved(t2.title.clone(), r));
        });
        self.dl_active = Some((t, p));
    }
}

/// Internet Archive search page: item ids, then item metadata in parallel; each item
/// is sent as soon as it arrives so the list fills progressively.
fn ia_job(gen: u64, q: &Query, tx: &Sender<Msg>) {
    let (ids, more) = if let Some((_, id)) = &q.album {
        (vec![(id.clone(), 0)], false)
    } else {
        match ia_search_ids(q) {
            Ok(x) => x,
            Err(err) => {
                let _ = tx.send(Msg::Fail { gen, src: IA, err });
                return;
            }
        }
    };
    let max = if q.album.is_some() {
        IA_ALBUM_TRACKS
    } else if q.text.trim().is_empty() && q.artist.is_none() {
        // Browse lists: a few tracks per album so that one album does not fill the page.
        3
    } else {
        IA_TRACKS_PER_ITEM
    };
    let hs: Vec<_> = ids
        .into_iter()
        .enumerate()
        .map(|(pos, (id, dls))| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let tracks = ia_item(&id, max, dls);
                let _ = tx.send(Msg::Page {
                    gen,
                    src: IA,
                    pos,
                    tracks,
                    more,
                    done: false,
                });
            })
        })
        .collect();
    for h in hs {
        let _ = h.join();
    }
    let _ = tx.send(Msg::Page {
        gen,
        src: IA,
        pos: usize::MAX,
        tracks: Vec::new(),
        more,
        done: true,
    });
}

impl Plugin for FreeMusic {
    fn manifest(&self) -> Manifest {
        Manifest::builtin(
            ID,
            "Free Music",
            "Search and browse Creative Commons and public-domain music from Jamendo, the Internet Archive and the Free Music Archive: genres, moods, licence filters, album and artist pages, play similar, and downloads with tags, cover and licence attribution.",
            "free-music",
        )
        .setting(SettingSpec::secret(
            "jamendo-client-id",
            "Jamendo client ID",
            "For Jamendo (the biggest catalogue; searched together with the Internet Archive once set). Get a free client ID at https://devportal.jamendo.com (sign up → My Apps → Add application). Only the public client ID is needed, never the secret. The Internet Archive needs no key.",
        ))
        .setting(SettingSpec::text(
            "folder",
            "Download folder",
            "Where downloads go (one folder per artist). Add it to the library to see them there.",
            "~/Music/Free Music",
            "",
        ))
        .setting(SettingSpec::action(
            "choose-folder",
            "Choose download folder",
            "Pick the download folder with a dialog.",
            "Choose…",
        ))
        .setting(SettingSpec::choice(
            "downloader",
            "Download with",
            "Built-in saves tags, cover, licence and attribution. Offline Downloads (plugin) adds the file to its queue instead, without tags.",
            &DOWNLOADERS,
        ))
        .setting(SettingSpec::action(
            "open",
            "Find free music",
            "Opens the search and browse dialog.",
            "Browse…",
        ))
    }
    fn disable(&mut self, cx: &Ctx) {
        self.gen += 1;
        self.pending.clear();
        if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            cx.app.global::<crate::Overlays>().set_md_open(false);
        }
    }
    fn command(&mut self, cx: &Ctx, name: &str, args: &[String]) {
        match name {
            "open" => self.open(cx),
            "choose-folder" => {
                let _ = slint::invoke_from_event_loop(|| {
                    if let Some(d) = rfd::FileDialog::new()
                        .set_title("Free Music download folder")
                        .pick_folder()
                    {
                        super::command(ID, "set-folder", &[d.to_string_lossy().into_owned()]);
                    }
                });
            }
            "set-folder" => {
                if let Some(d) = args.first().filter(|d| !d.is_empty()) {
                    cx.set("folder", d);
                    slint::Timer::single_shot(Duration::ZERO, crate::settings::refresh_ui);
                    cx.toast(&format!("Free Music downloads go to {d}"));
                }
            }
            "search" => self.search(cx),
            // `query <words…>`: fill the search box and search (scripts, screenshots).
            "query" => {
                cx.app
                    .global::<crate::MusicDialogState>()
                    .set_query(args.join(" ").into());
                self.search(cx);
            }
            // Typing: search after a short pause (see `tick`).
            "edited" => self.typed = Some(Instant::now()),
            "more" => {
                if self.pending.is_empty() && self.more.values().any(|m| *m) {
                    self.q.page += 1;
                    self.run(cx);
                }
            }
            "chip" => {
                let c = args.first().map(String::as_str).unwrap_or("");
                let i = CHIPS.iter().position(|x| x.0 == c);
                self.q.chip = if self.q.chip == i && i.is_some_and(|i| i >= 3) {
                    None
                } else {
                    i
                };
                if self.q.chip.is_some_and(|c| c < 3) {
                    // Popular / New / Featured are lists, not text searches.
                    self.q.text.clear();
                    cx.app
                        .global::<crate::MusicDialogState>()
                        .set_query("".into());
                }
                self.search(cx);
            }
            "filter" => {
                let (Some(n), true) = (args.first(), args.len() > 1) else {
                    return;
                };
                let v = &args[1..].join(" ");
                let pos = |l: &[&str]| l.iter().position(|x| x == v).unwrap_or(0);
                match n.as_str() {
                    "mood" => self.q.mood = pos(&MOODS),
                    "licence" => self.q.lic = pos(&LICENCES),
                    "length" => self.q.len = pos(&LENGTHS),
                    "sort" => self.q.sort = pos(&SORTS),
                    _ => return,
                }
                self.q.page = 0;
                self.filters(cx);
                self.run(cx);
            }
            "select" => {
                if let Some(t) = self.at(args) {
                    self.select(cx, &t);
                    self.show(cx);
                }
            }
            "primary" => match self.at(args).or_else(|| self.selected().cloned()) {
                Some(t) => self.play(cx, &t, false),
                None => Self::ui(cx, "Select a track first", false),
            },
            "secondary" => match self.at(args).or_else(|| self.selected().cloned()) {
                Some(t) => self.download(cx, t),
                None => Self::ui(cx, "Select a track first", false),
            },
            n if n.starts_with("act:") => {
                if &n[4..] == "‹ Back" {
                    if let Some(q) = self.back.pop() {
                        self.q = q;
                        if self.back.is_empty() {
                            self.title.clear();
                        }
                        self.filters(cx);
                        self.run(cx);
                    }
                    return;
                }
                let Some(t) = self.at(args).or_else(|| self.selected().cloned()) else {
                    return;
                };
                let src = SOURCES.iter().position(|s| *s == t.source).unwrap_or(0);
                match &n[4..] {
                    "Queue" => self.play(cx, &t, true),
                    "Similar" => self.similar(cx, &t),
                    "Album" => {
                        let q = Query {
                            source: src,
                            album: Some((t.source, t.album_id.clone())),
                            ..Default::default()
                        };
                        self.page_view(cx, format!("Album: {}", t.album), q);
                    }
                    "Artist" => {
                        let q = Query {
                            source: src,
                            sort: self.q.sort,
                            artist: Some((t.source, t.artist_id.clone())),
                            ..Default::default()
                        };
                        self.page_view(cx, format!("Artist: {}", t.artist), q);
                    }
                    // The dialog copied the text to the clipboard itself.
                    "Copy credit" => cx.toast("Attribution copied to the clipboard"),
                    "Web page" => crate::settings::open_url(&t.page),
                    _ => {}
                }
            }
            "close" => {
                self.gen += 1;
                self.pending.clear();
            }
            _ => {}
        }
    }
    fn tick(&mut self, cx: &Ctx) {
        if self.typed.is_some_and(|t| {
            t.elapsed() >= DEBOUNCE && cx.app.global::<crate::MusicDialogState>().get_owner() == ID
        }) {
            self.typed = None;
            let q = cx.app.global::<crate::MusicDialogState>().get_query();
            if q.trim().chars().count() >= 2 && q.trim() != self.q.text.trim() {
                self.search(cx);
            }
        }
        let mut changed = false;
        let mut status = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Page {
                    gen,
                    src,
                    pos,
                    tracks,
                    more,
                    done,
                } if gen == self.gen => {
                    if !tracks.is_empty() {
                        self.page.entry(src).or_default().insert(pos, tracks);
                    }
                    self.more.insert(src, more);
                    if done {
                        self.pending.remove(src);
                    }
                    changed = true;
                }
                Msg::Fail { gen, src, err } if gen == self.gen => {
                    eprintln!("vivi: free music: {err}");
                    self.pending.remove(src);
                    self.errors.push(err);
                    changed = true;
                }
                Msg::Cover(url, p) => {
                    if let Some(img) = p
                        .and_then(|p| slint::Image::load_from_path(&p).ok())
                        .filter(is_cover)
                    {
                        self.covers.insert(url, img);
                        self.covers_dirty = true;
                    }
                }
                Msg::Saved(title, r) => {
                    self.dl_active = None;
                    match r {
                        Ok(p) => {
                            self.dl_done += 1;
                            self.dl_last = format!("Saved {}", p.display());
                            cx.toast(&format!(
                                "Downloaded “{title}” with licence and attribution"
                            ));
                        }
                        Err(e) => {
                            self.dl_last = format!("“{title}” failed: {e}");
                            cx.toast(&self.dl_last.clone());
                        }
                    }
                    self.next_download(cx);
                    status = true;
                }
                _ => {}
            }
        }
        if changed {
            self.remerge();
            self.feed_radio(cx);
            self.covers_dirty = false;
            if cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
                self.show(cx);
            }
            status = true;
        } else if self.covers_dirty
            && cx.app.global::<crate::MusicDialogState>().get_owner() == ID
            && cx.app.global::<crate::Overlays>().get_md_open()
        {
            self.covers_dirty = false;
            self.show(cx);
        }
        if self.dl_active.is_some() {
            status = true;
        }
        if status && cx.app.global::<crate::MusicDialogState>().get_owner() == ID {
            self.status(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> serde_json::Value {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/free_music")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
    }

    fn t(ti: &str, ar: &str) -> Track {
        Track {
            id: format!("{ar}/{ti}"),
            title: ti.into(),
            artist: ar.into(),
            ..Default::default()
        }
    }

    #[test]
    fn licences() {
        assert_eq!(
            license_name("http://creativecommons.org/licenses/by-nc-sa/4.0/"),
            "CC BY-NC-SA 4.0"
        );
        assert_eq!(
            license_name("https://creativecommons.org/licenses/by/3.0/us/"),
            "CC BY 3.0"
        );
        assert_eq!(
            license_name("http://creativecommons.org/publicdomain/zero/1.0/"),
            "CC0 1.0"
        );
        assert_eq!(
            license_name("https://creativecommons.org/publicdomain/mark/1.0/"),
            "Public Domain Mark"
        );
        let by = "http://creativecommons.org/licenses/by/3.0/";
        let bysa = "http://creativecommons.org/licenses/by-sa/3.0/";
        let nc = "http://creativecommons.org/licenses/by-nc-nd/3.0/";
        let pd = "http://creativecommons.org/publicdomain/zero/1.0/";
        assert!(commercial_ok(by) && commercial_ok(pd) && !commercial_ok(nc));
        assert!(!commercial_ok("") && !commercial_ok("http://example.com/x"));
        assert!(lic_match(by, 3) && !lic_match(bysa, 3) && lic_match(bysa, 4));
        assert!(lic_match(pd, 5) && !lic_match(by, 5));
        assert!(lic_match(bysa, 2) && !lic_match(nc, 2) && !lic_match(nc, 1));
        assert!(lic_match("", 0) && !lic_match("", 1));
        assert!(lic_summary(nc).contains("non-commercial"));
        assert!(lic_summary(bysa).contains("same licence"));
    }

    #[test]
    fn parse_jamendo_fixture() {
        let v = jamendo_tracks(&fixture("jamendo_tracks.json"));
        assert_eq!(v.len(), 3);
        let a = &v[0];
        assert_eq!(a.title, "That's Life");
        assert_eq!(a.artist, "Yellow Faces");
        assert_eq!(
            (a.artist_id.as_str(), a.album_id.as_str()),
            ("7123", "104336")
        );
        assert_eq!(a.duration, 187);
        assert_eq!(a.genres, ["rock", "pop"]);
        assert_eq!(a.tags, ["happy", "energetic"]);
        assert!(a.cover.contains("width=300") && a.cover_full.contains("width=600"));
        assert_eq!(a.license(), "CC BY-SA 3.0");
        assert!(!a.download.is_empty());
        assert_eq!(v[1].download, "", "download not allowed");
        let dir = std::env::temp_dir().join("vivi-fm-nodl");
        assert!(download(&v[1], &dir, &AtomicU32::new(0)).is_err());
        // Entries without a stream are dropped.
        assert!(v.iter().all(|t| !t.stream.is_empty()));
    }

    #[test]
    fn parse_ia_fixture() {
        let v = ia_tracks(&fixture("ia_metadata.json"), 10);
        // The FLAC original, the 64 kbps duplicate and the hour-long mix are skipped.
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].title, "Opening");
        assert_eq!(v[1].title, "Night Drive");
        assert_eq!(v[0].artist, "Some Netlabel Artist");
        assert_eq!(v[0].album, "Night Sessions");
        assert_eq!(v[0].date, "2012-12-30");
        assert_eq!(v[0].duration, 245);
        assert_eq!(v[0].source, FMA);
        assert!(v[0].tags.contains(&"electronic".to_string()));
        assert_eq!(
            v[1].download,
            "https://archive.org/download/night_sessions/disc%201/02%20Night%20Drive.mp3"
        );
        assert!(v[0].cover_full.ends_with("/cover.jpg"));
        assert!(v[0].attribution().contains("CC BY 4.0"));
        let s = fixture("ia_search.json");
        let (ids, more) = ia_ids(&s, 0);
        assert_eq!(ids[0], ("night_sessions".to_string(), 1234));
        assert!(more);
        assert!(!ia_ids(&s, 10).1);
    }

    #[test]
    fn parse_ia_item_inline() {
        let m = serde_json::json!({
            "metadata": {"identifier": "my item", "creator": ["Band"], "title": "Live",
                "date": "2012-12-30T00:00:00Z",
                "licenseurl": "http://creativecommons.org/licenses/by/4.0/"},
            "files": [
                {"name": "a.flac", "format": "Flac"},
                {"name": "dir/b c.mp3", "format": "VBR MP3", "title": "Song B"},
                {"name": "c_64kb.mp3", "format": "64Kbps MP3"}
            ]
        });
        let t = ia_tracks(&m, 12);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].title, "Song B");
        assert_eq!(t[0].source, IA);
        assert_eq!(
            t[0].download,
            "https://archive.org/download/my%20item/dir/b%20c.mp3"
        );
        // No licence: nothing.
        let m = serde_json::json!({"metadata": {"identifier": "x"}, "files": []});
        assert!(ia_tracks(&m, 12).is_empty());
    }

    #[test]
    fn relevance_and_merge() {
        let q = Query {
            text: "kevin macleod".into(),
            ..Default::default()
        };
        let ia = vec![
            t("Lecture 3", "Prof"),
            t("Other", "Kevin MacLeod"),
            t("Kevin MacLeod Medley", "X"),
        ];
        let jam = vec![
            t("Kevin's Song", "Somebody"),
            t("Monkeys Spinning Monkeys", "Kevin MacLeod"),
        ];
        let v = merge(&[jam, ia], &q);
        assert_eq!(v[0].artist, "Kevin MacLeod");
        assert_eq!(v[1].artist, "Kevin MacLeod", "both exact artist hits first");
        // Fewer than five hits: non-matching ones stay, at the end.
        assert_eq!(v.last().unwrap().title, "Lecture 3");
        // Duplicates across sources collapse.
        let a = vec![t("Song", "Band")];
        let mut b = t("song!", "band");
        b.id = "other".into();
        assert_eq!(merge(&[a, vec![b]], &Query::default()).len(), 1);
        // Runs of one artist are broken up.
        let run = vec![
            t("1", "A"),
            t("2", "A"),
            t("3", "A"),
            t("4", "B"),
            t("5", "C"),
        ];
        let names: Vec<String> = spread(run).into_iter().map(|t| t.title).collect();
        assert_eq!(names, ["1", "2", "4", "3", "5"]);
        // Tag matches count.
        let mut g = t("Untitled", "Y");
        g.genres = vec!["jazz".into()];
        assert!(score(&g, "jazz") > score(&t("Untitled", "Z"), "jazz"));
    }

    #[test]
    fn filters_and_sort() {
        let mk = |ti: &str, d: u32, lic: &str, date: &str| Track {
            id: ti.into(),
            title: ti.into(),
            duration: d,
            license_url: lic.into(),
            date: date.into(),
            ..Default::default()
        };
        let by = "http://creativecommons.org/licenses/by/3.0/";
        let nc = "http://creativecommons.org/licenses/by-nc/3.0/";
        let l = vec![
            mk("a", 120, by, "2020-01-01"),
            mk("b", 400, nc, "2022-01-01"),
            mk("c", 200, by, "2021-01-01"),
        ];
        let q = |f: fn(&mut Query)| {
            let mut q = Query::default();
            f(&mut q);
            merge(std::slice::from_ref(&l), &q)
                .iter()
                .map(|t| t.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(q(|q| q.lic = 1), ["a", "c"]);
        let mut j = mk("j", 100, by, "");
        j.source = JAMENDO;
        j.genres = vec!["ambient".into()];
        let jazz = Query {
            chip: Some(7),
            ..Default::default()
        };
        assert!(!keep(&j, &jazz) && keep(&j, &Query::default()));
        j.genres.push("jazzfusion".into());
        assert!(keep(&j, &jazz));
        assert_eq!(q(|q| q.len = 1), ["a"]);
        assert_eq!(q(|q| q.len = 3), ["b"]);
        assert_eq!(q(|q| q.sort = 2), ["b", "c", "a"]);
        assert_eq!(q(|q| q.sort = 3), ["a", "c", "b"]);
        assert_eq!(q(|q| q.sort = 4), ["b", "c", "a"]);
    }

    #[test]
    fn query_building() {
        let mut q = Query {
            text: "ac/dc: (live)".into(),
            chip: Some(7),
            mood: 2,
            lic: 1,
            len: 2,
            ..Default::default()
        };
        let s = ia_query(&q);
        assert!(s.contains("title:(ac dc live)"), "{s}");
        assert!(s.contains("subject:(jazz)") && s.contains("subject:(happy)"));
        assert!(s.contains("NOT licenseurl:*nc*") && s.contains("NOT subject:(podcast"));
        assert!(
            s.contains("opensource_audio"),
            "text searches include community uploads"
        );
        assert_eq!(ia_sort(&q), "", "text search keeps IA relevance");
        let p = jamendo_params(&q, JMode::Name);
        let has = |k: &str, v: &str| p.iter().any(|(a, b)| *a == k && b == v);
        assert!(
            has("namesearch", "ac/dc: (live)")
                && has("fuzzytags", "jazz")
                && has("vartags", "happy")
        );
        assert!(has("ccnc", "false") && has("durationbetween", "180_360"));
        assert!(!p.iter().any(|(k, _)| *k == "client_id"));
        q.text.clear();
        q.chip = Some(1);
        q.page = 2;
        let p = jamendo_params(&q, JMode::Browse);
        let has = |k: &str, v: &str| p.iter().any(|(a, b)| *a == k && b == v);
        assert!(has("order", "releasedate_desc") && has("offset", "48"));
        assert_eq!(ia_sort(&q), "publicdate desc");
        assert!(
            !ia_query(&q).contains("opensource_audio"),
            "browse: curated only"
        );
        q.source = 3;
        assert!(ia_query(&q).contains("collection:(freemusicarchive)"));
        let a = Query {
            album: Some((JAMENDO, "9".into())),
            ..Default::default()
        };
        let p = jamendo_params(&a, JMode::Browse);
        assert!(p.iter().any(|(k, v)| *k == "album_id" && v == "9"));
        assert!(a.uses(JAMENDO) && !a.uses(IA));
        assert_eq!(tidy_stem("03_-_My_Song", ""), "My Song");
        assert_eq!(
            tidy_stem("deep-and-dirty-by-kevin-macleod", "Kevin MacLeod"),
            "deep and dirty"
        );
        assert_eq!(ia_len(&serde_json::json!("4:05")), Some(245.0));
        assert_eq!(fmt_len(245), "4:05");
        assert_eq!(
            clean_title("{dj-N} Club Step (ID: 396093)"),
            "{dj-N} Club Step"
        );
        assert_eq!(clean_title("Song (Live)"), "Song (Live)");
        assert_eq!(fmt_len(3725), "1:02:05");
        assert_eq!(unescape("That&#039;s"), "That's");
        assert_eq!(
            scrub("url?client_id=abc1".into(), "abc1"),
            "url?client_id=…"
        );
    }

    #[test]
    fn similar_and_tags() {
        let mut x = t("Song", "Band");
        x.source = JAMENDO;
        x.genres = vec!["jazz".into()];
        x.tags = vec!["relaxing".into()];
        x.date = "2019-05-01".into();
        x.license_url = "http://creativecommons.org/licenses/by/3.0/".into();
        let q = similar_query(&x);
        assert_eq!(q.chip.map(|c| CHIPS[c].1), Some("jazz"));
        assert_eq!(MOODS[q.mood], "Relaxing");
        assert_eq!(q.source, 1);
        let tg = tags_for(&x);
        assert_eq!(tg.genre, "Jazz");
        assert!(tg
            .copyright
            .starts_with("2019 Band. Licensed under CC BY 3.0"));
        assert!(tg.comment.contains("licensed under CC BY 3.0"));
        assert!(licence_text(&x).contains("commercial use OK"));
        assert!(target(&x, Path::new("/m")).ends_with("Band/Band - Song.mp3"));
    }

    /// Live Internet Archive search + download with tags (no key). `--ignored`.
    #[test]
    #[ignore]
    fn live_internet_archive() {
        let q = Query {
            text: "kevin macleod".into(),
            source: 2,
            ..Default::default()
        };
        let t0 = Instant::now();
        let (ids, _) = ia_search_ids(&q).unwrap();
        eprintln!("IA search {:?}: {} items", t0.elapsed(), ids.len());
        let all: Vec<Track> = ids
            .iter()
            .take(6)
            .flat_map(|(id, d)| ia_item(id, 8, *d))
            .collect();
        let v = merge(&[all], &q);
        assert!(!v.is_empty());
        for t in v.iter().take(5) {
            eprintln!(
                "  {} — {} [{}] {}",
                t.artist,
                t.title,
                t.license(),
                t.length()
            );
        }
        let dir = std::env::temp_dir().join(format!("vivi-fm-{}", std::process::id()));
        let small = v
            .iter()
            .min_by_key(|t| {
                if t.duration == 0 {
                    u32::MAX
                } else {
                    t.duration
                }
            })
            .unwrap();
        let p = download(small, &dir, &AtomicU32::new(0)).unwrap();
        let (tags, dur) = tagger::read_tags(&p).unwrap();
        assert_eq!(tags.title, small.title);
        assert!(
            tags.copyright.contains(&small.license_url),
            "{}",
            tags.copyright
        );
        assert!(tags.comment.contains("licensed under"));
        assert!(dur > 1.0);
        assert!(p.with_extension("licence.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live Jamendo search; needs `JAMENDO_CLIENT_ID` in the environment. `--ignored`.
    #[test]
    #[ignore]
    fn live_jamendo() {
        let Ok(key) = std::env::var("JAMENDO_CLIENT_ID") else {
            eprintln!("JAMENDO_CLIENT_ID not set, skipped");
            return;
        };
        for text in ["rock", "jazz", "piano", "lofi"] {
            let q = Query {
                text: text.into(),
                source: 1,
                ..Default::default()
            };
            let t0 = Instant::now();
            let (v, more) = jamendo_search(&key, &q).unwrap();
            let v = merge(&[v], &q);
            eprintln!(
                "{text}: {} tracks in {:?} (more {more})",
                v.len(),
                t0.elapsed()
            );
            assert!(!v.is_empty(), "{text}");
            assert!(v[0].stream.starts_with("http"));
        }
        let q = Query {
            chip: Some(0),
            source: 1,
            lic: 1,
            ..Default::default()
        };
        let (v, _) = jamendo_search(&key, &q).unwrap();
        assert!(!v.is_empty());
        assert!(merge(&[v], &q)
            .iter()
            .all(|t| commercial_ok(&t.license_url)));
    }
}
