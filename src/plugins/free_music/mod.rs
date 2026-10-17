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

mod api;
mod catalog;
pub use api::*;
pub use catalog::*;

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
