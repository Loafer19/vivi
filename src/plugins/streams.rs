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
pub const SOURCES: [&str; 7] = [
    "Radio: search",
    "Radio: top voted",
    "Radio: by country",
    "Radio: by genre",
    "M3U / IPTV playlist",
    "Saved",
    "Recent",
];
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
        let mut s = format!("{}\n\n{}\n", self.name, self.url);
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
                name,
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
                s.name = line
                    .rsplit('/')
                    .find(|p| !p.is_empty())
                    .unwrap_or(line)
                    .into();
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
}

impl Streams {
    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app.set_md_status(status.into());
        cx.app.set_md_busy(busy);
    }
    fn show(&self, cx: &Ctx) {
        let items: Vec<Item> = self
            .list
            .iter()
            .map(|s| Item {
                key: s.name.clone().into(),
                label: s.label().into(),
                selected: false,
            })
            .collect();
        cx.app.set_md_results(ModelRc::new(VecModel::from(items)));
        cx.app
            .set_md_secondary(if self.mode >= 5 { "Remove" } else { "Save" }.into());
    }
    fn mode(cx: &Ctx) -> usize {
        let s = cx.app.get_md_source();
        SOURCES.iter().position(|x| *x == s.as_str()).unwrap_or(0)
    }
    fn local(&mut self, cx: &Ctx, mode: usize) {
        self.mode = mode;
        self.list = load_list(if mode == 5 {
            "saved.json"
        } else {
            "recent.json"
        });
        self.show(cx);
        cx.app.set_md_detail("".into());
        let what = if mode == 5 { "saved" } else { "recent" };
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
                a.set_md_placeholder(
                    "Station name · country (or code) · genre · playlist file or URL".into(),
                );
                a.set_md_search_label("Go".into());
                a.set_md_primary("Play".into());
                let src: Vec<SharedString> = SOURCES.iter().map(|s| (*s).into()).collect();
                a.set_md_sources(ModelRc::new(VecModel::from(src)));
                if !SOURCES.contains(&a.get_md_source().as_str()) {
                    a.set_md_source(SOURCES[0].into());
                }
                a.set_md_open(true);
                self.local(cx, 5);
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
                if mode >= 5 {
                    self.local(cx, mode);
                    return;
                }
                let q = cx.app.get_md_query().trim().to_string();
                if q.is_empty() && mode != 1 {
                    Self::ui(cx, "Type something first", false);
                    return;
                }
                self.mode = mode;
                self.list.clear();
                self.show(cx);
                cx.app.set_md_detail("".into());
                Self::ui(
                    cx,
                    if mode == 4 {
                        "Loading playlist…"
                    } else {
                        "Searching radio-browser.info…"
                    },
                    true,
                );
                let (tx, rx) = channel();
                self.rx = Some(rx);
                std::thread::spawn(move || {
                    let r = if mode == 4 {
                        load_m3u(&q)
                    } else {
                        radio_search(mode, &q)
                    };
                    let _ = tx.send(r);
                });
            }
            "select" => {
                if let Some(s) = idx().and_then(|i| self.list.get(i)) {
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
                cx.player.cmd("loadfile", &[&s.url]);
                if s.source == "radio-browser.info" {
                    radio_click(&s.url);
                }
                let mut r = load_list("recent.json");
                push_front(&mut r, &s, MAX_RECENT);
                save_list("recent.json", &r);
                cx.toast(&format!("Playing {}", s.name));
                Self::ui(cx, &format!("Playing {}", s.name), false);
            }
            "secondary" => {
                let Some(i) = idx().filter(|i| *i < self.list.len()) else {
                    Self::ui(cx, "Select a stream first", false);
                    return;
                };
                if self.mode >= 5 {
                    let file = if self.mode == 5 {
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
        let Some(rx) = &self.rx else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.rx = None;
        match r {
            Ok(v) => {
                let n = v.len();
                self.list = v;
                self.show(cx);
                Self::ui(
                    cx,
                    &if n == 0 {
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

    /// Live radio-browser.info search (no key). `--ignored`.
    #[test]
    #[ignore]
    fn live_radio_browser() {
        for (m, q) in [(0, "jazz"), (1, ""), (2, "UA"), (3, "rock")] {
            let v = radio_search(m, q).unwrap();
            assert!(!v.is_empty(), "{m} {q}");
            eprintln!("{m} {q}: {} e.g. {:?}", v.len(), v[0].name);
        }
    }
}
