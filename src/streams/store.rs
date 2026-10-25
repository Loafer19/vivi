//! Streams hub storage (core, #86): favorites, recently played streams and the URL bar
//! history, all under `$XDG_DATA_HOME/vivi/streams/`:
//!
//! * `favorites.json` — starred streams (any source: radio, TV, a pasted URL, a plugin);
//! * `history.json` — recently played streams, newest first ([`MAX_RECENT`]);
//! * `urls.json` — what was typed or pasted into the URL bar, newest first ([`MAX_URLS`]).
//!
//! The first time one of them is missing it is migrated from what Vivi 0.13 kept
//! ([`migrate_in`]): Network Streams' `saved.json` (favorites) and `recent.json`, plus the
//! stream / web entries of the global Recent list and its URLs for the URL history. The
//! old files are left alone for one release (going back to 0.13 still finds them).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 50;
pub const MAX_URLS: usize = 20;
pub const FAVORITES: &str = "favorites.json";
pub const HISTORY: &str = "history.json";
pub const URLS: &str = "urls.json";
/// Network Streams (0.13) files the migration reads.
pub const LEGACY_SAVED: &str = "saved.json";
pub const LEGACY_RECENT: &str = "recent.json";

/// One stream: a radio station, a TV channel, a pasted URL or a plugin's track. The
/// Network Streams browser fills the directory fields; streams from elsewhere have only
/// `name`, `url` and maybe `logo`.
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
    /// "radio" | "tv" | "music" | "web" | "" (unknown; see [`kind_of`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// Plugin that played or starred it ("network-streams", "youtube-music", ""=URL bar).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub service: String,
}

/// What a stream is, for its card badge: the stored `kind`, else a guess from its source
/// and URL.
pub fn kind_of(s: &Stream) -> &'static str {
    match s.kind.as_str() {
        "radio" => return "radio",
        "tv" => return "tv",
        "music" => return "music",
        "web" => return "web",
        _ => {}
    }
    match s.source.as_str() {
        "radio-browser.info" | "SomaFM" => return "radio",
        "iptv-org" => return "tv",
        _ => {}
    }
    if !s.tvg_id.is_empty() {
        return "tv";
    }
    if s.service == "free-music" || s.service == "youtube-music" {
        return "music";
    }
    if crate::store::RecentKind::infer(&s.url) == crate::store::RecentKind::Web {
        return "web";
    }
    if !s.codec.is_empty() || !s.stationuuid.is_empty() {
        return "radio";
    }
    ""
}

pub fn dir() -> PathBuf {
    crate::store::data_dir().join("streams")
}

pub fn load_in(dir: &Path, name: &str) -> Vec<Stream> {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_in(dir: &Path, name: &str, v: &impl Serialize) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(t) = serde_json::to_string_pretty(v) {
        let _ = crate::store::write_atomic(&dir.join(name), &t);
    }
}

/// Put `s` first (dedup by URL), keeping what an older entry knew (logo, group, …) when
/// `s` does not say, and at most `max` entries.
pub fn merge_front(list: &mut Vec<Stream>, s: &Stream, max: usize) {
    let mut s = s.clone();
    if let Some(i) = list.iter().position(|o| o.url == s.url) {
        let old = list.remove(i);
        fill_from(&mut s, &old);
    }
    list.insert(0, s);
    list.truncate(max);
}

/// Empty fields of `s` taken from `old` (same stream, learned earlier). A name that is
/// only the URL counts as empty.
fn fill_from(s: &mut Stream, old: &Stream) {
    macro_rules! keep {
        ($($f:ident),*) => { $( if s.$f.is_empty() { s.$f = old.$f.clone(); } )* };
    }
    if s.name.is_empty() || s.name == s.url {
        s.name = old.name.clone();
    }
    keep!(
        logo,
        group,
        country,
        codec,
        homepage,
        source,
        tvg_id,
        epg,
        stationuuid,
        kind,
        service
    );
    if s.bitrate == 0 {
        s.bitrate = old.bitrate;
    }
}

/// Add `url` on top of the URL history (trimmed, no duplicates, at most `max`).
pub fn push_url_to(list: &mut Vec<String>, url: &str, max: usize) {
    let u = url.trim();
    if u.is_empty() {
        return;
    }
    list.retain(|o| o != u);
    list.insert(0, u.to_string());
    list.truncate(max);
}

/// Whether the URL bar text is something to open rather than words to search for.
pub fn looks_like_url(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return false;
    }
    if let Some((scheme, rest)) = t.split_once("://") {
        return !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            && !rest.is_empty();
    }
    // "www.example.com/x", "example.com/live.m3u8": a dotted host and a known TLD shape.
    let host = t.split('/').next().unwrap_or("");
    t.starts_with("www.")
        || (host.contains('.')
            && host
                .rsplit('.')
                .next()
                .is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
            && t.contains('/'))
}

/// `www.example.com/x` → `https://www.example.com/x`; URLs with a scheme stay.
pub fn normalize_url(t: &str) -> String {
    let t = t.trim();
    if t.contains("://") {
        t.to_string()
    } else {
        format!("https://{t}")
    }
}

/// What [`migrate_in`] did (counts of entries written; `None` = the file existed).
#[derive(Debug, Default, PartialEq)]
pub struct Migrated {
    pub favorites: Option<usize>,
    pub recent: Option<usize>,
    pub urls: Option<usize>,
}

/// Create the hub files in `dir` that do not exist yet from the 0.13 data: Network
/// Streams' `saved.json` / `recent.json` in the same folder and `legacy` (the global
/// Recent list, newest first). Idempotent: an existing file (even an empty list) is
/// never touched, so it runs once per file.
pub fn migrate_in(dir: &Path, legacy: &[crate::store::RecentEntry]) -> Migrated {
    use crate::store::RecentKind as K;
    let mut m = Migrated::default();
    if !dir.join(FAVORITES).exists() {
        let mut v: Vec<Stream> = Vec::new();
        for s in load_in(dir, LEGACY_SAVED) {
            if !s.url.is_empty() && !v.iter().any(|o| o.url == s.url) {
                v.push(tagged(s));
            }
        }
        m.favorites = Some(v.len());
        save_in(dir, FAVORITES, &v);
    }
    let streamish = |e: &&crate::store::RecentEntry| {
        e.target.contains("://")
            && !e.target.starts_with("file://")
            && matches!(e.kind, K::Stream | K::Web | K::Music)
    };
    if !dir.join(HISTORY).exists() {
        let mut v: Vec<Stream> = load_in(dir, LEGACY_RECENT)
            .into_iter()
            .filter(|s| !s.url.is_empty())
            .map(tagged)
            .collect();
        v.dedup_by(|a, b| a.url == b.url);
        // Global Recent entries Network Streams did not know, after its own (their
        // relative order is not known; Network Streams' list is the richer one).
        for e in legacy.iter().filter(streamish) {
            if v.iter().any(|s| s.url == e.target) {
                continue;
            }
            v.push(Stream {
                name: e.title.clone(),
                url: e.target.clone(),
                logo: e.image.clone(),
                group: e.sub.clone(),
                kind: match e.kind {
                    K::Web => "web".into(),
                    K::Music => "music".into(),
                    _ => String::new(),
                },
                service: if e.kind == K::Music {
                    "free-music".into()
                } else {
                    String::new()
                },
                ..Default::default()
            });
        }
        v.truncate(MAX_RECENT);
        m.recent = Some(v.len());
        save_in(dir, HISTORY, &v);
    }
    if !dir.join(URLS).exists() {
        // What the Open Network Stream dialog offered as its history.
        let v: Vec<String> = legacy
            .iter()
            .filter(|e| e.target.contains("://") && !e.target.starts_with("file://"))
            .filter(|e| matches!(e.kind, K::Stream | K::Web))
            .map(|e| e.target.clone())
            .take(MAX_URLS)
            .collect();
        m.urls = Some(v.len());
        save_in(dir, URLS, &v);
    }
    m
}

/// Network Streams entries were all its own.
fn tagged(mut s: Stream) -> Stream {
    if s.service.is_empty() && !s.source.is_empty() {
        s.service = "network-streams".into();
    }
    s
}

// ---------- the live files (`dir()`) ----------

thread_local! {
    static MIGRATED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run the migration once per process before the first read.
fn ready() -> PathBuf {
    let d = dir();
    if !MIGRATED.with(|m| m.replace(true)) {
        let r = migrate_in(&d, &crate::store::recent_entries());
        if r != Migrated::default() {
            eprintln!(
                "vivi: streams hub: migrated {} favorites, {} recent streams, {} URLs",
                r.favorites.unwrap_or(0),
                r.recent.unwrap_or(0),
                r.urls.unwrap_or(0)
            );
        }
    }
    d
}

pub fn favorites() -> Vec<Stream> {
    load_in(&ready(), FAVORITES)
}

pub fn save_favorites(v: &[Stream]) {
    save_in(&ready(), FAVORITES, &v);
}

pub fn is_favorite(url: &str) -> bool {
    favorites().iter().any(|s| s.url == url)
}

/// Star or unstar `s` (by URL). Returns whether it is a favorite now. New favorites go
/// on top.
pub fn toggle_favorite(s: &Stream) -> bool {
    let mut v = favorites();
    let was = v.iter().any(|o| o.url == s.url);
    if was {
        v.retain(|o| o.url != s.url);
    } else {
        // What Recent knew about it (logo, kind) fills the gaps.
        let mut s = s.clone();
        if let Some(old) = recent().into_iter().find(|o| o.url == s.url) {
            fill_from(&mut s, &old);
        }
        v.insert(0, s);
    }
    save_favorites(&v);
    !was
}

pub fn recent() -> Vec<Stream> {
    load_in(&ready(), HISTORY)
}

pub fn save_recent(v: &[Stream]) {
    save_in(&ready(), HISTORY, &v);
}

/// A stream was played: on top of Recent, merged with what was known about it.
pub fn push_recent(s: &Stream) {
    if s.url.is_empty() {
        return;
    }
    let mut v = recent();
    merge_front(&mut v, s, MAX_RECENT);
    save_recent(&v);
}

/// Rename the Recent entry of `url` when it only had a placeholder name (mpv learned the
/// title after it loaded).
pub fn name_recent(url: &str, name: &str) {
    let mut v = recent();
    let Some(s) = v.iter_mut().find(|s| s.url == url) else {
        return;
    };
    if (s.name.is_empty() || s.name == s.url) && !name.trim().is_empty() {
        s.name = name.trim().to_string();
        save_recent(&v);
    }
}

pub fn remove_recent(url: &str) {
    let mut v = recent();
    v.retain(|s| s.url != url);
    save_recent(&v);
}

pub fn urls() -> Vec<String> {
    std::fs::read_to_string(ready().join(URLS))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn push_url(url: &str) {
    let mut v = urls();
    push_url_to(&mut v, url, MAX_URLS);
    save_in(&ready(), URLS, &v);
}

pub fn clear_urls() {
    save_in(&ready(), URLS, &Vec::<String>::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{RecentEntry, RecentKind};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vivi-hub-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn st(name: &str, url: &str) -> Stream {
        Stream {
            name: name.into(),
            url: url.into(),
            ..Default::default()
        }
    }

    #[test]
    fn url_history_is_trimmed_deduped_and_capped() {
        let mut v = Vec::new();
        push_url_to(&mut v, "  http://a/1 ", 3);
        push_url_to(&mut v, "", 3);
        push_url_to(&mut v, "http://b/2", 3);
        push_url_to(&mut v, "http://a/1", 3);
        assert_eq!(v, ["http://a/1", "http://b/2"]);
        push_url_to(&mut v, "http://c", 3);
        push_url_to(&mut v, "http://d", 3);
        assert_eq!(v, ["http://d", "http://c", "http://a/1"]);
    }

    #[test]
    fn url_bar_tells_links_from_searches() {
        for u in [
            "https://x.example/live.m3u8",
            "rtmp://host/app",
            "www.example.com",
            "radio.example.org/stream.mp3",
            "srt://1.2.3.4:9000",
        ] {
            assert!(looks_like_url(u), "{u}");
        }
        for q in ["jazz", "bbc radio", "", "lofi beats", "news.", "v1.2"] {
            assert!(!looks_like_url(q), "{q}");
        }
        assert_eq!(
            normalize_url(" www.example.com/a "),
            "https://www.example.com/a"
        );
        assert_eq!(normalize_url("udp://@:1234"), "udp://@:1234");
    }

    #[test]
    fn recent_merges_what_it_knew() {
        let mut v = vec![
            Stream {
                logo: "http://l/1.png".into(),
                codec: "MP3".into(),
                bitrate: 128,
                ..st("Jazz FM", "http://j")
            },
            st("B", "http://b"),
        ];
        merge_front(&mut v, &st("http://j", "http://j"), 10);
        assert_eq!(v[0].name, "Jazz FM");
        assert_eq!(v[0].logo, "http://l/1.png");
        assert_eq!(v[0].bitrate, 128);
        assert_eq!(v.len(), 2);
        merge_front(&mut v, &st("C", "http://c"), 2);
        assert_eq!(
            v.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            ["http://c", "http://j"]
        );
    }

    #[test]
    fn kinds_for_badges() {
        assert_eq!(
            kind_of(&Stream {
                source: "radio-browser.info".into(),
                ..st("a", "http://a")
            }),
            "radio"
        );
        assert_eq!(
            kind_of(&Stream {
                tvg_id: "bbc.uk".into(),
                ..st("a", "http://a")
            }),
            "tv"
        );
        assert_eq!(
            kind_of(&st("v", "https://www.youtube.com/watch?v=x")),
            "web"
        );
        assert_eq!(
            kind_of(&Stream {
                service: "youtube-music".into(),
                ..st("s", "https://music.youtube.com/watch?v=x")
            }),
            "music"
        );
        assert_eq!(kind_of(&st("x", "http://host/live")), "");
    }

    #[test]
    fn migration_takes_network_streams_and_recent_once() {
        let d = tmp("migrate");
        let saved = vec![
            Stream {
                source: "radio-browser.info".into(),
                logo: "http://l".into(),
                ..st("Jazz", "http://jazz")
            },
            st("Mine", "http://mine.m3u8"),
            st("Jazz again", "http://jazz"),
        ];
        let recent = vec![Stream {
            source: "iptv-org".into(),
            tvg_id: "x.tv".into(),
            ..st("News", "http://news")
        }];
        save_in(&d, LEGACY_SAVED, &saved);
        save_in(&d, LEGACY_RECENT, &recent);
        let legacy = vec![
            RecentEntry {
                kind: RecentKind::Stream,
                title: "News HD".into(),
                ..RecentEntry::new("http://news")
            },
            RecentEntry {
                kind: RecentKind::Web,
                title: "A video".into(),
                image: "http://t.jpg".into(),
                ..RecentEntry::new("https://youtu.be/abc")
            },
            RecentEntry::new("/home/u/Videos/film.mkv"),
            RecentEntry {
                kind: RecentKind::Music,
                title: "Song".into(),
                ..RecentEntry::new("https://cdn.example/song.mp3")
            },
        ];
        let m = migrate_in(&d, &legacy);
        assert_eq!(
            m,
            Migrated {
                favorites: Some(2),
                recent: Some(3),
                urls: Some(2)
            }
        );
        let f = load_in(&d, FAVORITES);
        assert_eq!(
            f.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["Jazz", "Mine"]
        );
        assert_eq!(f[0].service, "network-streams");
        assert_eq!(f[0].logo, "http://l");
        assert_eq!(
            f[1].service, "",
            "streams without a directory source stay untagged"
        );
        let r = load_in(&d, HISTORY);
        assert_eq!(
            r.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            [
                "http://news",
                "https://youtu.be/abc",
                "https://cdn.example/song.mp3"
            ]
        );
        assert_eq!(
            r[0].tvg_id, "x.tv",
            "Network Streams' entry wins over the global one"
        );
        assert_eq!(r[1].kind, "web");
        assert_eq!(r[1].logo, "http://t.jpg");
        assert_eq!(r[2].service, "free-music");
        let u: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(d.join(URLS)).unwrap()).unwrap();
        assert_eq!(u, ["http://news", "https://youtu.be/abc"]);
        // The old files stay for one release.
        assert!(d.join(LEGACY_SAVED).exists() && d.join(LEGACY_RECENT).exists());
        // Second run: nothing to do, even after the user emptied a list.
        save_in(&d, FAVORITES, &Vec::<Stream>::new());
        assert_eq!(migrate_in(&d, &legacy), Migrated::default());
        assert!(load_in(&d, FAVORITES).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn migration_without_old_data_writes_empty_files() {
        let d = tmp("fresh");
        let m = migrate_in(&d, &[]);
        assert_eq!(
            m,
            Migrated {
                favorites: Some(0),
                recent: Some(0),
                urls: Some(0)
            }
        );
        assert!(d.join(FAVORITES).exists() && d.join(HISTORY).exists() && d.join(URLS).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_saved_files_still_parse() {
        // 0.13 saved.json entry (no kind / service).
        let t = r#"[{"name":"Radio X","url":"http://x","logo":"","group":"rock","country":"UK","codec":"MP3","bitrate":128,"homepage":"","source":"radio-browser.info","stationuuid":"u1"}]"#;
        let v: Vec<Stream> = serde_json::from_str(t).unwrap();
        assert_eq!(v[0].stationuuid, "u1");
        assert_eq!(kind_of(&v[0]), "radio");
        // New fields are skipped when empty, so 0.13 reads the new files too.
        let back = serde_json::to_string(&v[0]).unwrap();
        assert!(!back.contains("\"kind\"") && !back.contains("\"service\""));
    }
}
