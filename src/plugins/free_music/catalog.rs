//! Free Music catalog model: tracks, licences, the search query and how results from
//! several sources are filtered, scored and merged.

use super::*;

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
    pub(crate) fn key(&self) -> String {
        crate::plugins::protocol::join(&[norm(&self.artist), norm(&self.title)])
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

pub(crate) fn norm(s: &str) -> String {
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
    pub(crate) fn uses(&self, src: &'static str) -> bool {
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

pub(crate) fn s(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(a) => a.first().map(s).unwrap_or_default(),
        _ => String::new(),
    }
}

pub(crate) fn strings(v: &serde_json::Value) -> Vec<String> {
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
pub(crate) fn scrub(e: String, secret: &str) -> String {
    if secret.is_empty() {
        e
    } else {
        e.replace(secret, "…")
    }
}

// ---------- Internet Archive ----------
