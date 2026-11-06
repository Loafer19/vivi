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
use crate::streams::store as hub_store;
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
mod hub;
mod logos;
mod plugin;
pub use logos::*;

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
/// Source dropdown of the hub's TV tab.
const HUB_TV_SOURCES: [&str; 3] = ["TV channels", "M3U / IPTV playlist", "My playlists"];

/// The stream record is core since the Streams hub (#86): favorites and Recent hold
/// streams from every source.
pub use crate::streams::Stream;

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
    /// Streams hub tab shown ("radio" | "tv"; "" = the 0.13 browser, unused now).
    hub: String,
    /// Radio tab filters (indexes into the `HUB_*` lists, 0 = any).
    r_country: usize,
    r_genre: usize,
    r_lang: usize,
    r_bitrate: usize,
    r_sort: usize,
    /// TV tab filters and the program guide switch.
    t_country: usize,
    t_category: usize,
    guide_off: bool,
    /// Group chip to apply when the playlist that is loading arrives (TV country +
    /// category).
    chip_after: Option<String>,
}

impl NetworkStreams {
    fn ui(cx: &Ctx, status: &str, busy: bool) {
        cx.app
            .global::<crate::MusicDialogState>()
            .set_status(status.into());
        cx.app.global::<crate::MusicDialogState>().set_busy(busy);
    }
    fn reload_favs(&mut self) {
        self.favs = hub_store::favorites().into_iter().map(|s| s.url).collect();
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
        if self.guide_off {
            return None;
        }
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
    /// Source mode of the dropdown (the hub tabs have their own short lists).
    fn cur_mode(&self, cx: &Ctx) -> usize {
        let s = cx.app.global::<crate::MusicDialogState>().get_source();
        match self.hub.as_str() {
            "radio" => 0,
            "tv" => match s.as_str() {
                x if x == HUB_TV_SOURCES[1] => M3U,
                x if x == HUB_TV_SOURCES[2] => MY_PL,
                _ => TV_COUNTRY,
            },
            _ => SOURCES.iter().position(|x| *x == s.as_str()).unwrap_or(0),
        }
    }
    /// `s` as the hub stores it: ours, with its kind.
    fn tagged(&self, s: &Stream) -> Stream {
        let mut s = s.clone();
        if s.service.is_empty() {
            s.service = ID.into();
        }
        if s.kind.is_empty() {
            s.kind = match hub_store::kind_of(&s) {
                "" if self.hub == "tv"
                    || [M3U, TV_COUNTRY, TV_CATEGORY, MY_PL].contains(&self.mode) =>
                {
                    "tv"
                }
                "" => "radio",
                k => k,
            }
            .into();
        }
        s
    }
    fn local(&mut self, cx: &Ctx, mode: usize) {
        self.mode = mode;
        self.reload_favs();
        self.chip.clear();
        self.loaded_src.clear();
        self.all = if mode == SAVED {
            hub_store::favorites()
        } else {
            hub_store::recent()
        };
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
        if self.guide_off {
            return;
        }
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
        // Recent first, so the now-playing card knows the station when it loads.
        hub_store::push_recent(&self.tagged(s));
        crate::streams::refresh_later();
        cx.player.cmd("loadfile", &[&s.url]);
        if s.source == "radio-browser.info" {
            radio_click(&s.stationuuid);
        }
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

/// Station of `url` in the hub's favorites and recent lists.
fn known_station(url: &str) -> Option<Stream> {
    hub_store::favorites()
        .into_iter()
        .chain(hub_store::recent())
        .find(|s| s.url == url)
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
        s += "No network stream is playing. Pick a radio station or TV channel in Library → Streams.";
    }
    let n = |k: usize, one: &str| format!("{k} {one}{}", if k == 1 { "" } else { "s" });
    s += &format!(
        "\n\n{} · {}",
        n(favorites, "favorite"),
        n(recent, "recent stream")
    );
    s
}

#[cfg(test)]
mod tests;
