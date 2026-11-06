//! Free Music (#70): search and browse Creative Commons / public-domain music on
//! Jamendo (API v3, free `client_id` from <https://devportal.jamendo.com>) and the
//! Internet Archive (advancedsearch + metadata APIs, no key; its `freemusicarchive`
//! collection stands in for the Free Music Archive, whose API is gone). Both sources
//! are searched in parallel and merged with source badges; filters (genre, mood,
//! licence, length), sort, infinite scroll, album / artist pages and "play similar".
//! Internet Archive / FMA downloads land in a library folder with tags, cover, licence
//! and attribution (in the tags and in a `<file>.licence.txt`), through a built-in queue
//! or the Offline Downloads plugin; downloaded tracks are marked and play from the file.
//! Jamendo tracks stream only: its API terms forbid apps that offer offline access, so
//! their Download button is *Open on Jamendo* (download on the site; #85). Copies saved
//! by older versions still play from the file. The licence is shown on every row, in the
//! detail pane and on the now-playing card.

use super::auto_tagger::{self, Tags};
use super::web;
use super::{contrib, Ctx, Manifest, Plugin, SettingSpec};
use slint::ComponentHandle;
use slint::Model;
use slint::{ModelRc, SharedString, VecModel};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
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
const NO_KEY_NOTE: &str = "Jamendo is off (no client ID in the settings).";
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

mod api;
mod catalog;
pub use api::*;
pub use catalog::*;
mod plugin;

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
    /// Large cover of the detail pane: URL, cached file.
    Detail(String, Option<PathBuf>),
    /// Track id, title, result.
    Saved(String, String, Result<PathBuf, String>),
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
    dl_active: Option<(Track, Arc<DlProgress>)>,
    /// Tracks downloaded this session, for Retry in the Downloads panel.
    dl_tracks: HashMap<String, Track>,
    dl_done: usize,
    dl_last: String,
    note: String,
    /// The dialog's row model, updated in place (a new model rebuilds every row).
    model: Option<std::rc::Rc<VecModel<crate::FmRow>>>,
    /// Track id → downloaded already (file in the download folder), checked once.
    saved: HashMap<String, bool>,
    /// Source dropdown value last searched (a change searches again).
    last_source: String,
    /// Cover URL wanted in the detail pane.
    detail_url: String,
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
            dl_tracks: HashMap::new(),
            dl_done: 0,
            dl_last: String::new(),
            note: String::new(),
            model: None,
            saved: HashMap::new(),
            last_source: String::new(),
            detail_url: String::new(),
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

/// Tell the host what a track's stream URL is (title, artist, album, cover), so it plays
/// as a song: the music now-playing card, MPRIS and the shared now playing, not a radio
/// station card named after the file.
fn describe(t: &Track) {
    crate::plugins::host::media_meta(ID, track_meta(t));
}

pub(crate) fn track_meta(t: &Track) -> crate::plugins::host::Meta {
    crate::plugins::host::Meta {
        url: t.stream.clone(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album.clone(),
        cover: t.cover.clone(),
        // The card's info line names the source and the licence while the song plays.
        info: format!("{} · {}", t.source, t.license()),
        duration: f64::from(t.duration),
        kind: "music".into(),
        service: t.source.into(),
        ..Default::default()
    }
}

/// Cached list thumbnail (`free-music-covers/<hash>.jpg|png`); `None` when missing.
/// Cover already in the cache (no download).
pub(crate) fn cached_cover(url: &str) -> Option<PathBuf> {
    super::network_streams::cached_with_ext(
        &crate::store::cache_dir().join("free-music-covers"),
        url,
        &["jpg", "png"],
    )
}

fn fetch_cover(url: &str) -> Option<PathBuf> {
    let dir = crate::store::cache_dir().join("free-music-covers");
    let base = dir.join(super::network_streams::hash_name(url));
    if let Some(p) = super::network_streams::cached_with_ext(&dir, url, &["jpg", "png"]) {
        return Some(p);
    }
    if super::network_streams::hashed_file(&dir, url, |h| format!("{h}.none")).is_file() {
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

thread_local! {
    /// Covers decoded by the image cache (by cover URL), drained in `tick`.
    static READY: std::cell::RefCell<Vec<(String, slint::Image)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Detail-pane covers decoded by the image cache (by cover URL), drained in `tick`.
    static DETAIL: std::cell::RefCell<Vec<(String, slint::Image)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The Internet Archive's "no image" thumbnail is a wide banner; album art is ~square.
fn is_cover(img: &slint::Image) -> bool {
    let s = img.size();
    s.width > 0 && s.height > 0 && s.width < 2 * s.height
}

/// Whether Vivi downloads the track itself: the artist allows it and it is not from
/// Jamendo, whose API terms forbid apps that offer offline access (#85).
fn downloadable(t: &Track) -> bool {
    !t.download.is_empty() && t.source != JAMENDO
}

/// Why a Jamendo track has no in-app download.
const JAMENDO_DL: &str = "Jamendo tracks stream here; download them on jamendo.com (Open on Jamendo). Jamendo's API terms do not allow in-app downloads.";

/// Label of the action that opens the track's page on its source.
fn page_label(t: &Track) -> String {
    match t.source {
        JAMENDO => "Open on Jamendo".into(),
        _ => "Open on Internet Archive".into(),
    }
}

fn same_model(shown: &ModelRc<crate::FmRow>, m: &std::rc::Rc<VecModel<crate::FmRow>>) -> bool {
    shown
        .as_any()
        .downcast_ref::<VecModel<crate::FmRow>>()
        .map(|v| v as *const _)
        == Some(std::rc::Rc::as_ptr(m))
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
            self.detail_url.clear();
            cx.app
                .global::<crate::MusicDialogState>()
                .set_has_detail_cover(false);
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
                // Short: it follows the result count on one status line.
                self.note = NO_KEY_NOTE.into();
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
            let pct = p.pm.load(Ordering::Relaxed) / 10;
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

    /// Downloaded already: the file is in the download folder (checked once per track).
    fn is_saved(&mut self, cx: &Ctx, t: &Track) -> bool {
        if t.download.is_empty() {
            return false;
        }
        if let Some(s) = self.saved.get(&t.id) {
            return *s;
        }
        let s = target(t, &Self::folder(cx)).is_file();
        self.saved.insert(t.id.clone(), s);
        s
    }

    fn show(&mut self, cx: &Ctx) {
        let sel = self.sel.clone();
        let tracks = std::mem::take(&mut self.tracks);
        let saved: Vec<bool> = tracks.iter().map(|t| self.is_saved(cx, t)).collect();
        self.tracks = tracks;
        let rows: Vec<crate::FmRow> = self
            .tracks
            .iter()
            .zip(saved)
            .map(|(t, saved)| {
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
                    dl: downloadable(t),
                    saved,
                    selected: sel.as_ref() == Some(&t.id),
                    bar: -1.0,
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
        let st = cx.app.global::<crate::MusicDialogState>();
        // The highlight follows the selected track when rows are re-sorted or replaced.
        let idx = sel
            .as_ref()
            .and_then(|id| self.tracks.iter().position(|t| &t.id == id));
        st.set_selected(idx.map_or(-1, |i| i as i32));
        match &self.model {
            // Same model still shown: change only the rows that differ.
            Some(m) if same_model(&st.get_tracks(), m) => {
                for (i, row) in rows.into_iter().enumerate() {
                    if i < m.row_count() {
                        if m.row_data(i).as_ref() != Some(&row) {
                            m.set_row_data(i, row);
                        }
                    } else {
                        m.push(row);
                    }
                }
                while m.row_count() > self.tracks.len() {
                    m.remove(m.row_count() - 1);
                }
            }
            _ => {
                let m = std::rc::Rc::new(VecModel::from(rows));
                st.set_tracks(ModelRc::from(m.clone()));
                self.model = Some(m);
            }
        }
        st.set_results(ModelRc::new(VecModel::from(items)));
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
            // A direct link to the track's page (Jamendo's API terms ask for one).
            acts.push(page_label(t).into());
        }
        cx.app
            .global::<crate::MusicDialogState>()
            .set_actions(ModelRc::new(VecModel::from(acts)));
        cx.app.global::<crate::MusicDialogState>().set_secondary(
            match self.selected() {
                Some(t) if t.source == JAMENDO => "Open on Jamendo",
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
        d += &if t.source == JAMENDO {
            format!("\n\n{JAMENDO_DL}")
        } else if t.download.is_empty() {
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
        self.want_detail_cover(cx, t);
        self.buttons(cx);
    }

    /// Large cover above the detail text: the full-size cover (cached like the row covers),
    /// the row thumbnail when that fails.
    fn want_detail_cover(&mut self, cx: &Ctx, t: &Track) {
        let url = [&t.cover_full, &t.cover]
            .into_iter()
            .find(|u| u.starts_with("http"))
            .cloned()
            .unwrap_or_default();
        let st = cx.app.global::<crate::MusicDialogState>();
        if url == self.detail_url && st.get_detail_cover_owner() == ID {
            return;
        }
        st.set_has_detail_cover(false);
        self.detail_url = url.clone();
        if url.is_empty() {
            return;
        }
        if let Some(p) = cached_cover(&url) {
            self.detail_ready(cx, &url, &p);
            return;
        }
        let (tx, small) = (self.tx.clone(), t.cover.clone());
        std::thread::spawn(move || {
            let p =
                fetch_cover(&url).or_else(|| (small != url).then(|| fetch_cover(&small)).flatten());
            let _ = tx.send(Msg::Detail(url, p));
        });
    }

    fn detail_ready(&mut self, cx: &Ctx, url: &str, p: &Path) {
        let key = url.to_string();
        let img = crate::images::get(p, crate::images::TILE, move |i| {
            DETAIL.with(|r| r.borrow_mut().push((key, i)))
        });
        if let Some(i) = img {
            self.show_detail_cover(cx, url, i);
        }
    }

    fn show_detail_cover(&mut self, cx: &Ctx, url: &str, img: slint::Image) {
        // Same rule as the rows: the Archive's wide "audio" banner is no cover.
        if url != self.detail_url || !is_cover(&img) {
            return;
        }
        let st = cx.app.global::<crate::MusicDialogState>();
        st.set_detail_cover(img);
        st.set_detail_cover_owner(ID.into());
        st.set_has_detail_cover(true);
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
                grid: false,
                info: false,
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
        a.global::<crate::MusicDialogState>()
            .set_has_detail_cover(false);
        // Its page (Streams hub → Music, or its sidebar icon) (#86).
        crate::streams::open_dialog(a);
        self.last_source = SOURCES[self.q.source].into();
        // Downloads may have been moved or deleted since the last time.
        self.saved.clear();
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
        self.last_source = src.to_string();
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
        self.sync_query_box(cx);
        self.filters(cx);
        self.run(cx);
    }

    fn play(&mut self, cx: &Ctx, t: &Track, queue: bool) {
        // Downloaded already: play the file (offline, with its tags and cover).
        let local = target(t, &Self::folder(cx));
        if !t.download.is_empty() && local.is_file() {
            let p = local.to_string_lossy().into_owned();
            if queue {
                cx.player.enqueue(&p);
                cx.toast(&format!(
                    "Added {} — {} to the queue (downloaded copy)",
                    t.artist, t.title
                ));
            } else {
                cx.player.cmd("loadfile", &[&p]);
                cx.toast(&format!(
                    "Playing {} — {} (downloaded copy)",
                    t.artist, t.title
                ));
            }
            return;
        }
        describe(t);
        crate::loading::set_name(&t.stream, &format!("{} — {}", t.artist, t.title));
        crate::store::recent_hint(crate::store::RecentEntry {
            target: t.stream.clone(),
            kind: crate::store::RecentKind::Music,
            title: t.title.clone(),
            sub: t.artist.clone(),
            image: t.cover.clone(),
        });
        if queue {
            cx.player.enqueue(&t.stream);
            cx.toast(&format!("Added {} — {} to the queue", t.artist, t.title));
        } else {
            cx.player.cmd("loadfile", &[&t.stream]);
            cx.toast(&format!("Playing {} — {}", t.artist, t.title));
        }
    }

    /// Show the current page's search text in the query box (Similar, album and artist
    /// pages, and Back from them), so it never shows a search that is not running.
    fn sync_query_box(&mut self, cx: &Ctx) {
        // A half-typed edit belongs to the page that was left.
        self.typed = None;
        cx.app
            .global::<crate::MusicDialogState>()
            .set_query(query_box_text(&self.q).into());
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
                describe(t);
                crate::loading::set_name(&t.stream, &format!("{} — {}", t.artist, t.title));
                crate::store::recent_hint(crate::store::RecentEntry {
                    target: t.stream.clone(),
                    kind: crate::store::RecentKind::Music,
                    title: t.title.clone(),
                    sub: t.artist.clone(),
                    image: t.cover.clone(),
                });
                cx.player.enqueue(&t.stream);
                n += 1;
            }
        }
        if n > 0 {
            self.note = format!("Queued {} similar tracks.", seen.len() - 1);
        }
    }

    fn download(&mut self, cx: &Ctx, t: Track) {
        if t.source == JAMENDO {
            Self::ui(cx, JAMENDO_DL, false);
            return;
        }
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
        crate::downloads::report(
            ID,
            &t.id,
            crate::downloads::Report {
                title: Some(format!("{} — {}", t.artist, t.title)),
                state: Some("queued".into()),
                artwork: cached_cover(&t.cover).map(|p| p.to_string_lossy().into_owned()),
                error: Some(String::new()),
                done: Some(0),
                can_pause: Some(false),
                ..Default::default()
            },
        );
        self.dl_tracks.insert(t.id.clone(), t.clone());
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
        let p = Arc::new(DlProgress::default());
        let (tx, dir, t2, p2) = (self.tx.clone(), Self::folder(cx), t.clone(), p.clone());
        crate::downloads::report(
            ID,
            &t.id,
            crate::downloads::Report {
                state: Some("active".into()),
                path: Some(target(&t, &dir).to_string_lossy().into_owned()),
                ..Default::default()
            },
        );
        std::thread::spawn(move || {
            let r = download(&t2, &dir, &p2);
            let _ = tx.send(Msg::Saved(t2.id.clone(), t2.title.clone(), r));
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

/// Text the search box shows for `q`: an album or artist page lists that album or
/// artist, not a text search, so the box is empty there (the title names the page).
pub(crate) fn query_box_text(q: &Query) -> String {
    if q.album.is_some() || q.artist.is_some() {
        String::new()
    } else {
        q.text.clone()
    }
}

#[cfg(test)]
mod tests;
