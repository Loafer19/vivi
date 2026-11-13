//! Library UI controller: navigation, search, sorting, home sections, context menu, thumbnails.

use crate::config::Config;
use crate::library::{self, Media, MediaKind};
use crate::player::Player;
use crate::{images, store, AppWindow, Item, LibItem};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

thread_local! {
    /// Models that may show thumbnails; updated when the worker finishes one.
    static MODELS: RefCell<Vec<ThumbModel>> = const { RefCell::new(Vec::new()) };
    /// While the browsed list is built: only take thumbnails already in memory; the rest
    /// are decoded when their rows scroll into view ([`thumb_window`]).
    static LAZY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The rows the browsed list last reported as visible.
    static SEEN: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 48)) };
    /// The image file behind each thumbnail key (a library file's thumbnail, a cover).
    static SRCS: RefCell<std::collections::HashMap<slint::SharedString, std::path::PathBuf>> =
        RefCell::new(Default::default());
}

/// Rows kept around the visible ones (decoded ahead, not dropped while scrolling back).
const WINDOW_MARGIN: usize = 24;

/// A model that shows thumbnails plus the rows of each `thumb_key`, so a finished
/// thumbnail touches only its own rows instead of scanning every row of every model.
struct ThumbModel {
    model: Rc<VecModel<LibItem>>,
    rows: std::collections::HashMap<slint::SharedString, Vec<usize>>,
    /// Rows near the viewport (the browsed list only; `None` = every row).
    window: Option<(usize, usize)>,
}

impl ThumbModel {
    fn new(model: Rc<VecModel<LibItem>>) -> Self {
        Self {
            model,
            rows: Default::default(),
            window: None,
        }
    }

    fn set_thumb(&self, key: &str, img: &slint::Image) {
        for &i in self.rows.get(key).map(Vec::as_slice).unwrap_or_default() {
            if self.window.is_some_and(|(lo, hi)| i < lo || i >= hi) {
                continue;
            }
            if let Some(mut r) = self
                .model
                .row_data(i)
                .filter(|r| r.thumb_key.as_str() == key)
            {
                r.thumb = img.clone();
                r.has_thumb = true;
                self.model.set_row_data(i, r);
            }
        }
    }
}

/// The rows of each non-empty `thumb_key` in `v`.
fn thumb_rows(v: &[LibItem]) -> std::collections::HashMap<slint::SharedString, Vec<usize>> {
    let mut rows: std::collections::HashMap<_, Vec<usize>> = Default::default();
    for (i, r) in v.iter().enumerate() {
        if !r.thumb_key.is_empty() {
            rows.entry(r.thumb_key.clone()).or_default().push(i);
        }
    }
    rows
}

/// Fill `model` with `v`, keeping the thumbnail index in step when it is one of
/// the thumbnail models. The model is set outside the `MODELS` borrow.
///
/// Incremental (#100): rows that did not change are left alone and the tail is
/// pushed or cut, so a refresh of the same view (pin, watched, a finished scan) keeps
/// the scroll position and only repaints what changed.
fn show(model: &Rc<VecModel<LibItem>>, v: Vec<LibItem>) {
    let rows = thumb_rows(&v);
    MODELS.with(|ms| {
        if let Some(t) = ms
            .borrow_mut()
            .iter_mut()
            .find(|t| Rc::ptr_eq(&t.model, model))
        {
            t.rows = rows;
        }
    });
    let old = model.row_count();
    // A different list (navigation): replace it in one go.
    let same_list =
        old > 0 && !v.is_empty() && model.row_data(0).is_some_and(|r| r.key == v[0].key);
    if !same_list {
        model.set_vec(v);
        return;
    }
    let n = v.len();
    for (i, mut row) in v.into_iter().enumerate() {
        if i < old {
            if let Some(cur) = model.row_data(i) {
                // Keep a picture already shown for the same file.
                if cur.thumb_key == row.thumb_key && cur.has_thumb && !row.has_thumb {
                    row.thumb = cur.thumb.clone();
                    row.has_thumb = true;
                }
                if cur != row {
                    model.set_row_data(i, row);
                }
            }
        } else {
            model.push(row);
        }
    }
    for i in (n..old).rev() {
        model.remove(i);
    }
}

/// Add `v` after the rows of `model` (the next page of the browsed list).
fn append(model: &Rc<VecModel<LibItem>>, v: Vec<LibItem>) {
    let base = model.row_count();
    MODELS.with(|ms| {
        if let Some(t) = ms
            .borrow_mut()
            .iter_mut()
            .find(|t| Rc::ptr_eq(&t.model, model))
        {
            for (k, idx) in thumb_rows(&v) {
                t.rows
                    .entry(k)
                    .or_default()
                    .extend(idx.into_iter().map(|i| i + base));
            }
        }
    });
    for r in v {
        model.push(r);
    }
}

/// The section of a library navigation key (`"<section>"` or `"<section>:<arg>"`, as
/// the UI sends it, e.g. `"video:/media/Films"` or `"music:artists:Name"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav {
    Home,
    Video,
    Music,
    Streams,
    Service,
    Recent,
    Downloads,
    Bookmarks,
    Playlists,
    /// A smart playlist (`smart:<id>`) or a "Similar to" list (`similar:<path>`), #87.
    Smart,
}

impl Nav {
    /// The section of `key` and the rest after the first `:`. Unknown sections are Home.
    fn parse(key: &str) -> (Self, &str) {
        let (section, arg) = key.split_once(':').unwrap_or((key, ""));
        let s = match section {
            "video" => Self::Video,
            "music" => Self::Music,
            "streams" => Self::Streams,
            "service" => Self::Service,
            "recent" => Self::Recent,
            "downloads" => Self::Downloads,
            "bookmarks" => Self::Bookmarks,
            "playlists" => Self::Playlists,
            "smart" | "similar" => Self::Smart,
            _ => Self::Home,
        };
        (s, arg)
    }
}

/// Rows of the browsed list fetched per page (#100); more load while scrolling.
const PAGE: usize = 400;

pub struct Lib {
    /// Read connection to the library index (UI thread). Views query it per page
    /// instead of keeping every row in memory (#100).
    db: Option<rusqlite::Connection>,
    /// Indexed files, and whether any is audio (refreshed after each scan).
    total: usize,
    has_audio: bool,
    /// The rest of the browsed list: its query with the next offset.
    more: Option<library::Query>,
    /// The page is a smart playlist (the UI shows its own empty state).
    smart: bool,
    nav: String,
    search: String,
    requested: HashSet<String>,
    thumbs: Option<std::sync::mpsc::Sender<Media>>,
    models: Models,
    back: Vec<String>,
    fwd: Vec<String>,
}

fn name_of(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map_or(p.to_string(), |n| n.to_string_lossy().into_owned())
}

impl Lib {
    fn item(&mut self, m: &Media, cfg: &Config) -> LibItem {
        let pr = library::progress(m);
        let mut sub = Vec::new();
        if m.kind == MediaKind::Audio && !m.artist.is_empty() {
            sub.push(m.artist.clone());
        }
        if m.duration > 0.0 {
            sub.push(library::human_time(m.duration));
        }
        sub.push(library::human_size(m.size));
        let mut it = LibItem {
            key: m.path.clone().into(),
            thumb_key: m.path.clone().into(),
            title: m.title.clone().into(),
            subtitle: sub.join(" · ").into(),
            kind: m.kind.as_str().into(),
            progress: pr,
            pinned: cfg.library.pinned.contains(&m.path),
            watched: pr >= 0.95 || cfg.library.watched.contains(&m.path),
            ..Default::default()
        };
        if cfg.library.thumbnails {
            let tp = library::thumb_path(&m.path);
            if tp.exists() {
                // Decoded on the image worker; the row is filled when it is ready.
                if let Some(img) = thumb(&m.path, &tp) {
                    it.thumb = img;
                    it.has_thumb = true;
                }
            } else if tp.with_extension("none").exists() {
                // No picture available (e.g. audio without cover art).
            } else if let Some(tx) = &self.thumbs {
                if self.requested.insert(m.path.clone()) {
                    let _ = tx.send(m.clone());
                }
            }
        }
        it
    }

    /// A navigation/action row with its own icon (Figma "Add", "Songs", "Albums", …).
    fn action(key: &str, title: &str, icon: &str) -> LibItem {
        LibItem {
            key: key.into(),
            title: title.into(),
            kind: "action".into(),
            icon: icon.into(),
            ..Default::default()
        }
    }

    /// A Recent entry: library files keep their thumbnail and details; folders, playlists,
    /// streams and web videos get their title, a host / folder line and a type icon.
    /// The subtitle starts with the type label ("Stream · radio.example").
    fn recent_item(&mut self, e: &store::RecentEntry, cfg: &Config) -> LibItem {
        let label = e.kind.label();
        let mut it = self.recent_item_inner(e, cfg, label);
        it.badge = label.into();
        it.recent_key = e.target.clone().into();
        // Station logo / cover / thumbnail when the opener knew one and it is cached.
        if !it.has_thumb && !e.image.is_empty() {
            let img = if Path::new(&e.image).is_file() {
                Some(std::path::PathBuf::from(&e.image))
            } else {
                crate::plugins::network_streams::cached_logo(&e.image)
                    .or_else(|| crate::plugins::free_music::cached_cover(&e.image))
                    .or_else(|| crate::plugins::host::cached_cover(&e.image))
            };
            if let Some(p) = img {
                // Decoded on a worker; rows keyed by the file get it when it is ready.
                let key = p.to_string_lossy().into_owned();
                it.thumb_key = key.as_str().into();
                if let Some(i) = picture(&key) {
                    it.thumb = i;
                    it.has_thumb = true;
                }
            }
        }
        it
    }

    fn recent_item_inner(&mut self, e: &store::RecentEntry, cfg: &Config, label: &str) -> LibItem {
        use store::RecentKind as K;
        match e.kind {
            K::Video | K::Audio if Path::new(&e.target).is_file() => {
                let m = self.find(&e.target);
                let mut it = self.item(&m, cfg);
                it.subtitle = format!("{label} · {}", it.subtitle).into();
                it
            }
            K::Folder => {
                let mut it = Self::group(
                    format!("video:{}", e.target),
                    &e.display_title(),
                    format!("{label} · {}", e.display_sub()),
                    "folder",
                    cfg.library.pinned.contains(&e.target),
                );
                it.icon = "folder".into();
                it
            }
            K::Playlist => {
                let mut it = Self::group(
                    e.target.clone(),
                    &e.display_title(),
                    format!("{label} · {}", e.display_sub()),
                    "playlist",
                    false,
                );
                it.icon = "playlist".into();
                it
            }
            _ => {
                let kind = if matches!(e.kind, K::Music | K::Audio) {
                    "audio"
                } else {
                    "video"
                };
                let mut it = Self::group(
                    e.target.clone(),
                    &e.display_title(),
                    format!("{label} · {}", e.display_sub()),
                    kind,
                    false,
                );
                it.icon = match e.kind {
                    K::Music | K::Audio => "songs",
                    K::Web => "web",
                    _ => "streaming",
                }
                .into();
                it
            }
        }
    }

    fn group(key: String, title: &str, sub: String, kind: &str, pinned: bool) -> LibItem {
        LibItem {
            key: key.into(),
            title: title.into(),
            subtitle: sub.into(),
            kind: kind.into(),
            pinned,
            ..Default::default()
        }
    }

    /// Use the playlist thumbnail chosen in the New Playlist dialog.
    fn cover(it: &mut LibItem, playlist: &std::path::Path) {
        if let Some(c) = store::playlist_cover(playlist) {
            let key = c.to_string_lossy().into_owned();
            it.thumb_key = key.as_str().into();
            if let Some(img) = thumb(&key, &c) {
                it.thumb = img;
                it.has_thumb = true;
            }
        }
    }

    /// Run `q` on the index (nothing without one).
    fn rows(&self, q: &library::Query, cfg: &Config) -> Vec<Media> {
        let Some(db) = &self.db else {
            return Vec::new();
        };
        library::query(db, q, &|m| watched(m, cfg))
    }

    fn count(&self, q: &library::Query) -> usize {
        self.db.as_ref().map_or(0, |db| library::count(db, q))
    }

    /// Re-read the totals after a scan (the per-folder counts for Settings too).
    fn reload(&mut self) {
        let Some(db) = &self.db else { return };
        self.total = library::count(db, &library::Query::default());
        self.has_audio = library::count(
            db,
            &library::Query {
                kind: Some(MediaKind::Audio),
                ..Default::default()
            },
        ) > 0;
        DIRS.with(|x| *x.borrow_mut() = Some(library::dir_counts(db, &Default::default())));
    }

    /// The browsed list's first page of `q`; the rest loads as it scrolls ([`Lib::more`]).
    fn page(&mut self, mut q: library::Query, cfg: &Config, out: &mut Vec<LibItem>) {
        q.offset = 0;
        q.limit = PAGE;
        let rows = self.rows(&q, cfg);
        if rows.len() == PAGE {
            q.offset = PAGE;
            self.more = Some(q);
        }
        for m in &rows {
            out.push(self.item(m, cfg));
        }
    }

    /// The next page of the browsed list, if there is one.
    fn next_page(&mut self, cfg: &Config) -> Vec<LibItem> {
        let Some(mut q) = self.more.take() else {
            return Vec::new();
        };
        let rows = self.rows(&q, cfg);
        if rows.len() == q.limit {
            q.offset += q.limit;
            self.more = Some(q);
        }
        rows.iter().map(|m| self.item(m, cfg)).collect()
    }

    /// Rebuild the current view. Returns breadcrumb (key, label) pairs.
    fn build(&mut self, cfg: &Config) -> (Vec<LibItem>, Vec<(String, String)>) {
        let nav = self.nav.clone();
        let (section, arg) = Nav::parse(&nav);
        let sort = cfg.library.sort.as_str();
        self.more = None;
        self.smart = section == Nav::Smart;
        let base = |kind: Option<MediaKind>| library::Query {
            kind,
            search: self.search.clone(),
            sort: sort.into(),
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut crumbs = Vec::new();
        match section {
            Nav::Video => {
                crumbs.push(("video".into(), "Videos".into()));
                let videos = base(Some(MediaKind::Video));
                if !self.search.is_empty() {
                    self.page(videos, cfg, &mut out);
                } else if arg.is_empty() {
                    // Roots. Skip the "Add" row when there are no folders with
                    // videos so the shared empty state can show (#102).
                    let groups: Vec<(&String, usize)> = cfg
                        .library
                        .folders
                        .iter()
                        .map(|r| {
                            let under = library::Query {
                                under: Some(r.clone()),
                                ..videos.clone()
                            };
                            (r, self.count(&under))
                        })
                        .filter(|(_, n)| *n > 0)
                        .collect();
                    if !groups.is_empty() {
                        out.push(Self::action("action:add-folder", "Add", "add"));
                        for (r, n) in groups {
                            let pin = cfg.library.pinned.contains(r);
                            out.push(Self::group(
                                format!("video:{r}"),
                                &name_of(r),
                                format!("{n} videos"),
                                "folder",
                                pin,
                            ));
                        }
                    }
                } else {
                    let root = cfg
                        .library
                        .folders
                        .iter()
                        .find(|r| arg.starts_with(r.as_str()))
                        .cloned()
                        .unwrap_or_default();
                    let mut acc = root.clone();
                    crumbs.push((format!("video:{root}"), name_of(&root)));
                    for part in arg[root.len()..].split('/').filter(|s| !s.is_empty()) {
                        acc = format!("{acc}/{part}");
                        crumbs.push((format!("video:{acc}"), part.to_string()));
                    }
                    let dirs = self
                        .db
                        .as_ref()
                        .map(|db| library::subdirs(db, &videos, arg))
                        .unwrap_or_default();
                    for (d, n) in dirs {
                        let full = format!("{arg}/{d}");
                        let pin = cfg.library.pinned.contains(&full);
                        out.push(Self::group(
                            format!("video:{full}"),
                            &d,
                            format!("{n} videos"),
                            "folder",
                            pin,
                        ));
                    }
                    let files = library::Query {
                        dir: Some(arg.into()),
                        ..videos
                    };
                    self.page(files, cfg, &mut out);
                }
            }
            Nav::Music if arg.is_empty() && self.search.is_empty() => {
                crumbs.push(("music".into(), "Music".into()));
                // Songs/Albums/… only once there is audio; otherwise EmptyState (#102).
                if self.has_audio {
                    out.push(Self::action("action:add-folder", "Add", "add"));
                    out.push(Self::action("music:songs", "Songs", "songs"));
                    out.push(Self::action("music:albums", "Albums", "album"));
                    out.push(Self::action("music:artists", "Artists", "artist"));
                    out.push(Self::action("music:genres", "Genres", "genre"));
                }
            }
            // The Streams hub and service pages (src/streams): drawn by the hub, no items.
            Nav::Streams => crumbs.push(("streams".into(), "Streams".into())),
            Nav::Service => {
                crumbs.push(("streams".into(), "Streams".into()));
                crumbs.push((nav.clone(), crate::streams::service_name(arg)));
            }
            Nav::Recent => {
                crumbs.push(("recent".into(), "Recent".into()));
                for e in store::recent_entries() {
                    out.push(self.recent_item(&e, cfg));
                }
            }
            Nav::Music => {
                crumbs.push(("music".into(), "Music".into()));
                let songs = base(Some(MediaKind::Audio));
                let (tab, val) = arg.split_once(':').unwrap_or((arg, ""));
                let tab = if tab.is_empty() { "songs" } else { tab };
                let field = match tab {
                    "albums" => Some(library::Field::Album),
                    "artists" => Some(library::Field::Artist),
                    "songs" => None,
                    _ => Some(library::Field::Genre),
                };
                let label = match tab {
                    "albums" => "Albums",
                    "artists" => "Artists",
                    "genres" => "Genres",
                    _ => "Songs",
                };
                crumbs.push((format!("music:{tab}"), label.into()));
                match field {
                    Some(f) if val.is_empty() => {
                        let groups = self
                            .db
                            .as_ref()
                            .map(|db| library::groups(db, &songs, f))
                            .unwrap_or_default();
                        let firsts: Vec<String> = groups.iter().map(|g| g.2.clone()).collect();
                        let covers = self
                            .db
                            .as_ref()
                            .map(|db| library::by_paths(db, &firsts))
                            .unwrap_or_default();
                        for (g, n, first) in groups {
                            let mut it = Self::group(
                                format!("music:{tab}:{g}"),
                                &g,
                                format!("{n} songs"),
                                "group",
                                false,
                            );
                            // Use the first song's cover as the group thumbnail.
                            if let Some(m) = covers.get(&first) {
                                let t = self.item(m, cfg);
                                it.thumb = t.thumb;
                                it.has_thumb = t.has_thumb;
                                it.thumb_key = t.thumb_key;
                            }
                            out.push(it);
                        }
                    }
                    f => {
                        let mut q = songs;
                        if let Some(f) = f {
                            q.field = Some((f, val.into()));
                            crumbs.push((nav.clone(), val.into()));
                        }
                        if tab == "albums" && sort == "name" {
                            q.sort = "track".into();
                        }
                        self.page(q, cfg, &mut out);
                    }
                }
            }
            // The shared Downloads panel (ui/downloads.slint) draws its own list.
            Nav::Downloads => {
                crumbs.push(("downloads".into(), "Downloads".into()));
            }
            // Every saved bookmark (src/bookmarks.rs), by file then time.
            Nav::Bookmarks => {
                crumbs.push(("playlists".into(), "Playlists".into()));
                crumbs.push(("bookmarks".into(), "Bookmarks".into()));
                let dir = crate::bookmarks::thumbs_dir();
                for (path, title, m) in crate::bookmarks::list_all() {
                    let file = if title.is_empty() {
                        store::display_name(&path)
                    } else {
                        title
                    };
                    let mut sub = format!("{file} · {}", crate::bookmarks::short_time(m.time));
                    if !m.note.is_empty() {
                        sub = format!("{sub} · {}", m.note);
                    }
                    let mut it = Self::group(
                        crate::bookmarks::lib_key(&path, m.time),
                        &m.label,
                        sub,
                        "bookmark",
                        false,
                    );
                    it.icon = "bookmark".into();
                    // Only once the frame is saved (a miss would stick in the image cache);
                    // the page renders again when it is (bookmarks::changed).
                    if !m.thumb.is_empty() && dir.join(&m.thumb).is_file() {
                        let f = dir.join(&m.thumb).to_string_lossy().into_owned();
                        it.thumb_key = f.as_str().into();
                        if let Some(img) = picture(&f) {
                            it.thumb = img;
                            it.has_thumb = true;
                        }
                    }
                    out.push(it);
                }
            }
            Nav::Smart => {
                crumbs.push(("playlists".into(), "Playlists".into()));
                if let Some(l) = smart_list(&nav) {
                    crumbs.push((nav.clone(), l.name.clone()));
                    let rows = self
                        .db
                        .as_ref()
                        .map(|db| crate::smart::evaluate(db, &l, crate::smart::now()))
                        .unwrap_or_default();
                    for m in &rows {
                        out.push(self.item(m, cfg));
                    }
                }
            }
            Nav::Playlists => {
                crumbs.push(("playlists".into(), "Playlists".into()));
                let marks = crate::bookmarks::list_all().len();
                let lists = store::playlists();
                let smart = crate::smart::load();
                if !lists.is_empty() || marks > 0 || !smart.is_empty() {
                    out.push(Self::action(
                        "action:create-playlist",
                        "Create playlist",
                        "add",
                    ));
                }
                if marks > 0 {
                    let mut it = Self::action("bookmarks", "Bookmarks", "bookmark");
                    it.subtitle = format!("{marks} saved").into();
                    out.push(it);
                }
                // Smart playlists (#87): rebuilt from the play history and tags.
                let now = crate::smart::now();
                for l in &smart {
                    let n = self
                        .db
                        .as_ref()
                        .map_or(0, |db| crate::smart::evaluate(db, l, now).len());
                    let mut it = Self::group(
                        format!("smart:{}", l.id),
                        &l.name,
                        format!("{n} items · {}", crate::smart::describe(l)),
                        "playlist",
                        false,
                    );
                    it.icon = "playlist".into();
                    it.badge = "Smart".into();
                    out.push(it);
                }
                let names: Vec<(String, String)> = crate::plugins::manifests()
                    .into_iter()
                    .map(|m| (m.id, m.name))
                    .collect();
                for (name, path) in lists {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    let n = text
                        .lines()
                        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
                        .count();
                    // Kept in sync by an extension: its name as the badge and in the line.
                    let owner = store::parse_playlist_source(&text).map(|s| {
                        names
                            .iter()
                            .find(|(id, _)| *id == s.plugin)
                            .map_or(s.plugin.clone(), |(_, n)| n.clone())
                    });
                    let sub = match &owner {
                        Some(o) => format!("{n} items · {o}"),
                        None => format!("{n} items"),
                    };
                    let mut it = Self::group(
                        path.to_string_lossy().into_owned(),
                        &name,
                        sub,
                        "playlist",
                        false,
                    );
                    if let Some(o) = owner {
                        it.badge = o.into();
                    }
                    Self::cover(&mut it, &path);
                    out.push(it);
                }
            }
            _ => {
                crumbs.push(("home".into(), "Home".into()));
                if !self.search.is_empty() {
                    let mut q = base(None);
                    q.limit = 200;
                    for m in self.rows(&q, cfg) {
                        out.push(self.item(&m, cfg));
                    }
                } else {
                    // Recently added
                    let q = library::Query {
                        sort: "newest".into(),
                        limit: 12,
                        ..Default::default()
                    };
                    for m in self.rows(&q, cfg) {
                        out.push(self.item(&m, cfg));
                    }
                }
            }
        }
        (out, crumbs)
    }

    fn find(&self, path: &str) -> Media {
        self.db
            .as_ref()
            .and_then(|db| library::by_path(db, path))
            .unwrap_or_else(|| Media {
                title: Path::new(path)
                    .file_stem()
                    .map_or(path.into(), |s| s.to_string_lossy().into_owned()),
                kind: library::kind_of(Path::new(path)).unwrap_or_default(),
                size: std::fs::metadata(path).map_or(0, |m| m.len() as i64),
                path: path.into(),
                ..Default::default()
            })
    }

    /// Files played lately, newest first: the Recent list, then the play history (#87).
    fn history_paths(&self) -> Vec<String> {
        let mut v = store::recent();
        if let Some(db) = &self.db {
            let seen: HashSet<String> = v.iter().cloned().collect();
            v.extend(
                crate::smart::recent_paths(db, 500)
                    .into_iter()
                    .filter(|p| !seen.contains(p)),
            );
        }
        v
    }

    /// The Home sections (built here, shown by [`HomeRows::show`] after the library
    /// borrow ends).
    fn home(&mut self, cfg: &Config) -> HomeRows {
        // "Continue Watching": started, unfinished videos among the files played lately
        // (the resume positions live in mpv's watch-later files, not in the index).
        let mut cont: Vec<Media> = Vec::new();
        for p in self.history_paths() {
            if cont.len() >= 12 {
                break;
            }
            if library::kind_of(Path::new(&p)) == Some(MediaKind::Video)
                && !cfg.library.watched.contains(&p)
                && store::resume_position(&p).is_some()
                && Path::new(&p).exists()
            {
                let m = self.find(&p);
                let pr = library::progress(&m);
                if pr > 0.0 && pr < 0.95 {
                    cont.push(m);
                }
            }
        }
        let cont: Vec<LibItem> = cont.iter().take(12).map(|m| self.item(m, cfg)).collect();
        // Same entries as the Recent view: files that still exist, folders, playlists,
        // streams, web videos and Free Music tracks, each with its type label.
        let recent: Vec<LibItem> = store::recent_entries()
            .iter()
            .filter(|e| store::url_host(&e.target).is_some() || Path::new(&e.target).exists())
            .take(12)
            .map(|e| self.recent_item(e, cfg))
            .collect();
        let pinned: Vec<LibItem> = cfg
            .library
            .pinned
            .iter()
            .map(|p| {
                if Path::new(p).is_dir() {
                    let mut g = Self::group(
                        format!("video:{p}"),
                        &name_of(p),
                        "Folder".into(),
                        "folder",
                        true,
                    );
                    g.key = format!("video:{p}").into();
                    g
                } else {
                    let m = self.find(p);
                    self.item(&m, cfg)
                }
            })
            .collect();
        // "Listen Again": recently played songs.
        let listen: Vec<LibItem> = store::recent()
            .iter()
            .filter(|p| {
                library::kind_of(Path::new(p)) == Some(MediaKind::Audio) && Path::new(p).exists()
            })
            .take(12)
            .map(|p| {
                let m = self.find(p);
                self.item(&m, cfg)
            })
            .collect();
        // "Frequently Visited Folders": favourite folders, then folders of recent files.
        let mut dirs: Vec<String> = cfg
            .library
            .pinned
            .iter()
            .filter(|p| Path::new(p).is_dir())
            .cloned()
            .collect();
        for p in store::recent() {
            if let Some(d) = Path::new(&p).parent().filter(|d| d.is_dir()) {
                let d = d.to_string_lossy().into_owned();
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
        let folders: Vec<LibItem> = dirs
            .iter()
            .take(12)
            .map(|d| {
                Self::group(
                    format!("video:{d}"),
                    &name_of(d),
                    d.clone(),
                    "folder",
                    cfg.library.pinned.contains(d),
                )
            })
            .collect();
        let lists: Vec<LibItem> = store::playlists()
            .into_iter()
            .take(12)
            .map(|(n, p)| {
                let mut it = Self::group(
                    p.to_string_lossy().into_owned(),
                    &n,
                    String::new(),
                    "playlist",
                    false,
                );
                Self::cover(&mut it, &p);
                it
            })
            .collect();
        // "Frequently Listened Artists": artists by number of recent plays, then library size.
        let mut count: std::collections::HashMap<String, usize> = self
            .db
            .as_ref()
            .map(|db| library::artist_counts(db).into_iter().collect())
            .unwrap_or_default();
        let played = store::recent();
        let known = self
            .db
            .as_ref()
            .map(|db| library::by_paths(db, &played))
            .unwrap_or_default();
        for p in &played {
            if let Some(m) = known.get(p).filter(|m| !m.artist.is_empty()) {
                *count.entry(m.artist.clone()).or_default() += 1000;
            }
        }
        let mut ranked: Vec<(String, usize)> = count.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let artists: Vec<LibItem> = ranked
            .into_iter()
            .take(12)
            .map(|(a, _)| {
                let mut it = Self::group(
                    format!("music:artists:{a}"),
                    &a,
                    String::new(),
                    "group",
                    false,
                );
                it.icon = "artist".into();
                it
            })
            .collect();
        HomeRows {
            listen,
            folders,
            lists,
            artists,
            cont,
            recent,
            pinned,
        }
    }
}

/// A smart playlist key (`smart:<id>` or `similar:<path>`).
fn is_smart(key: &str) -> bool {
    key.starts_with("smart:") || key.starts_with("similar:")
}

/// The smart list behind a key.
fn smart_list(key: &str) -> Option<crate::smart::SmartList> {
    if let Some(p) = key.strip_prefix("similar:") {
        return Some(crate::smart::similar(p));
    }
    let id = key.strip_prefix("smart:")?;
    crate::smart::load().into_iter().find(|l| l.id == id)
}

/// Play, shuffle, queue, save as .m3u or delete a smart playlist.
fn smart_action(a: &AppWindow, p: &Rc<Player>, lib: &Rc<RefCell<Lib>>, key: &str, action: &str) {
    let Some(list) = smart_list(key) else { return };
    let note = |t: String| a.global::<crate::Notices>().set_resume_text(t.into());
    if action == "delete" {
        let mut all = crate::smart::load();
        all.retain(|l| l.id != list.id);
        match crate::smart::save(&all) {
            Ok(()) => note(format!("Deleted “{}”", list.name)),
            Err(e) => eprintln!("vivi: smart playlists: {e}"),
        }
        return;
    }
    let files: Vec<String> = match lib.try_borrow() {
        Ok(l) => {
            l.db.as_ref()
                .map(|db| crate::smart::evaluate(db, &list, crate::smart::now()))
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.path)
                .collect()
        }
        Err(_) => return,
    };
    if files.is_empty() {
        note(format!("“{}” is empty", list.name));
        return;
    }
    match action {
        "save-m3u" => match store::save_playlist(&list.name, &files) {
            Ok(_) => note(format!("Saved “{}” to Playlists", list.name)),
            Err(e) => eprintln!("vivi: save playlist: {e}"),
        },
        "enqueue" => {
            let start = !a.global::<crate::Playback>().get_has_media();
            for (i, f) in files.iter().enumerate() {
                if i == 0 && start {
                    p.open(f);
                } else {
                    p.enqueue(f);
                }
            }
            note(format!("Added {} items to the queue", files.len()));
        }
        // Play / Shuffle: start the list and queue the rest of it.
        _ => {
            p.open(&files[0]);
            for f in &files[1..] {
                p.enqueue(f);
            }
            if action == "shuffle" {
                let p = p.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(400), move || {
                    p.run("playlist-shuffle")
                });
            }
            if a.global::<crate::Library>().get_full()
                || !a.global::<crate::Playback>().get_has_media()
            {
                a.global::<crate::Overlays>().set_library_open(false);
            }
            a.global::<crate::Library>().set_full(false);
        }
    }
}

/// Watched: finished (95 %) or marked watched.
fn watched(m: &Media, cfg: &Config) -> bool {
    library::progress(m) >= 0.95 || cfg.library.watched.contains(&m.path)
}

/// The Home sections of one render.
struct HomeRows {
    listen: Vec<LibItem>,
    folders: Vec<LibItem>,
    lists: Vec<LibItem>,
    artists: Vec<LibItem>,
    cont: Vec<LibItem>,
    recent: Vec<LibItem>,
    pinned: Vec<LibItem>,
}

/// The library's models, cloned out so they are filled after the `Lib` borrow ends
/// (filling a model runs Slint bindings, which may call back into the library, #101).
#[derive(Clone)]
struct Models {
    items: Rc<VecModel<LibItem>>,
    cont: Rc<VecModel<LibItem>>,
    recent: Rc<VecModel<LibItem>>,
    pinned: Rc<VecModel<LibItem>>,
    listen: Rc<VecModel<LibItem>>,
    folders: Rc<VecModel<LibItem>>,
    lists: Rc<VecModel<LibItem>>,
    artists: Rc<VecModel<LibItem>>,
}

impl HomeRows {
    fn show(self, m: &Models) {
        show(&m.listen, self.listen);
        show(&m.folders, self.folders);
        show(&m.lists, self.lists);
        show(&m.artists, self.artists);
        show(&m.cont, self.cont);
        show(&m.recent, self.recent);
        show(&m.pinned, self.pinned);
    }
}

use crate::util::strings;

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: crate::app::state::AppState, log: bool) {
    let items = Rc::new(VecModel::default());
    let (cont, recent, pinned) = (
        Rc::new(VecModel::default()),
        Rc::new(VecModel::default()),
        Rc::new(VecModel::default()),
    );
    let (listen, folders, lists, artists) = (
        Rc::new(VecModel::default()),
        Rc::new(VecModel::default()),
        Rc::new(VecModel::default()),
        Rc::new(VecModel::default()),
    );
    MODELS.with(|m| {
        m.borrow_mut()
            .extend([&items, &cont, &recent, &pinned, &listen].map(|m| ThumbModel::new(m.clone())))
    });
    app.global::<crate::Library>()
        .set_listen(ModelRc::from(listen.clone()));
    app.global::<crate::Library>()
        .set_folders(ModelRc::from(folders.clone()));
    app.global::<crate::Library>()
        .set_lists(ModelRc::from(lists.clone()));
    app.global::<crate::Library>()
        .set_artists(ModelRc::from(artists.clone()));
    app.global::<crate::Library>()
        .set_items(ModelRc::from(items.clone()));
    app.global::<crate::Library>()
        .set_continue(ModelRc::from(cont.clone()));
    app.global::<crate::Library>()
        .set_recent(ModelRc::from(recent.clone()));
    app.global::<crate::Library>()
        .set_pinned(ModelRc::from(pinned.clone()));
    app.global::<crate::Library>()
        .set_ytdlp(library::ytdlp_available());

    let thumbs = (cfg.config().library.thumbnails && library::ffmpeg_available()).then(|| {
        library::thumb_worker(move |path, file| {
            let _ = slint::invoke_from_event_loop(move || {
                // Decode only when a row near the view shows it; the rest load on scroll.
                SRCS.with(|m| m.borrow_mut().insert(path.as_str().into(), file.clone()));
                if !wanted(&path) {
                    return;
                }
                let key = path.clone();
                if let Some(img) = images::get(&file, images::TILE, move |i| set_thumb(&key, &i)) {
                    set_thumb(&path, &img);
                }
            });
        })
    });

    // Station logos cached by older versions (no file extension, never loadable).
    {
        let w = app.as_weak();
        std::thread::spawn(move || {
            if crate::plugins::network_streams::migrate_logo_cache() > 0 {
                let _ = w.upgrade_in_event_loop(|a| a.global::<crate::Library>().invoke_refresh());
            }
            // Keep the image caches on disk under their budget, oldest files first.
            images::prune_disk(images::DISK_BUDGET);
        });
    }

    let db = library::db_path();
    let mut l = Lib {
        db: library::open_db(&db)
            .map_err(|e| eprintln!("vivi: library index: {e}"))
            .ok(),
        total: 0,
        has_audio: false,
        more: None,
        smart: false,
        nav: "home".into(),
        search: String::new(),
        requested: HashSet::new(),
        thumbs,
        models: Models {
            items,
            cont,
            recent,
            pinned,
            listen,
            folders,
            lists,
            artists,
        },
        back: Vec::new(),
        fwd: Vec::new(),
    };
    l.reload();
    let lib = Rc::new(RefCell::new(l));

    let render: Rc<dyn Fn()> = {
        let w = app.as_weak();
        let lib = lib.clone();
        let cfg = cfg.clone();
        Rc::new(move || {
            let Some(a) = w.upgrade() else { return };
            let c = cfg.config();
            // Build under the library borrow, fill the models after it ends: setting a
            // model runs Slint bindings that may call back in here (#101).
            let Ok(mut l) = lib.try_borrow_mut() else {
                eprintln!("vivi: library: nested render skipped");
                return;
            };
            LAZY.set(true);
            let (v, mut crumbs) = l.build(&c);
            LAZY.set(false);
            let home = l.home(&c);
            let models = l.models.clone();
            let (nav, total, back, fwd, smart) = (
                l.nav.clone(),
                l.total,
                !l.back.is_empty(),
                !l.fwd.is_empty(),
                l.smart,
            );
            drop(l);
            // Figma breadcrumb: long paths keep the last two levels behind "...".
            if crumbs.len() > 3 {
                let keep = crumbs.split_off(crumbs.len() - 2);
                let up = crumbs.pop().map(|c| c.0).unwrap_or_default();
                crumbs = std::iter::once((up, "...".to_string()))
                    .chain(keep)
                    .collect();
            }
            show(&models.items, v);
            let (lo, hi) = SEEN.get();
            thumb_window(lo, hi);
            home.show(&models);
            let g = a.global::<crate::Library>();
            g.set_nav(nav.clone().into());
            g.set_smart(smart);
            g.set_crumb_keys(strings(crumbs.iter().map(|c| c.0.clone())));
            g.set_crumb_labels(strings(crumbs.into_iter().map(|c| c.1)));
            g.set_sort(c.library.sort.clone().into());
            g.set_grid(c.library.view != "list");
            g.set_count(total as i32);
            g.set_can_back(back);
            g.set_can_forward(fwd);
            a.global::<crate::Panel>().invoke_lib_search("".into());
            crate::streams::nav_changed(&a, &nav);
        })
    };

    // Background scan; re-render when done. `quiet` (the startup scan): no "scanning"
    // indicator, and Home is only rebuilt when the index actually changed.
    let rescan_with: Rc<dyn Fn(bool)> = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        Rc::new(move |quiet| {
            let roots = cfg.config().library.folders.clone();
            let hidden = cfg.config().stub(crate::config::HIDDEN_FILES);
            if let Some(a) = w.upgrade().filter(|_| !quiet) {
                a.global::<crate::Library>().set_scanning(true);
            }
            let w = w.clone();
            let db = db.clone();
            std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let res = library::open_db(&db).and_then(|mut c| {
                    library::scan_changes(&mut c, &roots, hidden)
                        .map(|(n, changed)| (n, changed, !quiet || changed > 0))
                });
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(a) = w.upgrade() else { return };
                    a.global::<crate::Library>().set_scanning(false);
                    match res {
                        Ok((n, changed, reload)) => {
                            if log {
                                eprintln!(
                                    "library scan: {n} files, {changed} changed in {:?}",
                                    t.elapsed()
                                );
                            }
                            if reload {
                                RELOAD.set(true);
                                a.global::<crate::Library>().invoke_refresh();
                            }
                        }
                        Err(e) => eprintln!("vivi: library scan failed: {e}"),
                    }
                });
            });
        })
    };
    let rescan: Rc<dyn Fn()> = {
        let r = rescan_with.clone();
        Rc::new(move || r(false))
    };

    {
        let lib = lib.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_refresh(move || {
            if RELOAD.take() {
                if let Ok(mut l) = lib.try_borrow_mut() {
                    l.reload();
                }
                crate::settings::refresh_ui();
            }
            render();
        });
    }
    {
        let lib = lib.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_navigate(move |key| {
            {
                let mut l = lib.borrow_mut();
                if l.nav != key.as_str() {
                    let prev = std::mem::replace(&mut l.nav, key.to_string());
                    l.back.push(prev);
                    l.fwd.clear();
                }
                l.search.clear();
            }
            render();
        });
    }
    {
        let lib = lib.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_history(move |forward| {
            {
                let mut l = lib.borrow_mut();
                let next = if forward { l.fwd.pop() } else { l.back.pop() };
                let Some(next) = next else { return };
                let prev = std::mem::replace(&mut l.nav, next);
                if forward {
                    l.back.push(prev);
                } else {
                    l.fwd.push(prev);
                }
                l.search.clear();
            }
            render();
        });
    }
    {
        let lib = lib.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_search(move |q| {
            lib.borrow_mut().search = q.to_string();
            render();
        });
    }
    {
        let cfg = cfg.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_set_sort(move |s| {
            cfg.update(move |c| c.library.sort = s.to_string());
            let _ = cfg.save();
            render();
        });
    }
    {
        let cfg = cfg.clone();
        let render = render.clone();
        {
            // Near the end of the browsed list: append its next page (#100).
            let (lib, cfg) = (lib.clone(), cfg.clone());
            app.global::<crate::Library>()
                .on_thumb_window(move |lo, hi| {
                    let (lo, hi) = (lo.max(0) as usize, hi.max(0) as usize);
                    let more = lib.try_borrow_mut().ok().and_then(|mut l| {
                        let n = l.models.items.row_count();
                        (l.more.is_some() && hi + WINDOW_MARGIN * 2 >= n)
                            .then(|| (l.next_page(&cfg.config()), l.models.items.clone()))
                    });
                    if let Some((rows, model)) = more.filter(|(r, _)| !r.is_empty()) {
                        append(&model, rows);
                    }
                    thumb_window(lo, hi);
                });
        }
        app.global::<crate::Library>().on_set_grid(move |g| {
            cfg.update(move |c| c.library.view = if g { "grid" } else { "list" }.into());
            let _ = cfg.save();
            render();
        });
    }
    {
        let p = player.clone();
        let w = app.as_weak();
        let lib = lib.clone();
        app.global::<crate::Library>().on_open(move |key, kind| {
            let Some(a) = w.upgrade() else { return };
            match kind.as_str() {
                "action" => match key.as_str() {
                    "action:add-folder" => a.global::<crate::Library>().invoke_add_folder(),
                    "action:network" => crate::streams::open_page(&a, "streams:url"),
                    "action:create-playlist" => {
                        a.global::<crate::Library>().invoke_create_playlist()
                    }
                    k => a.global::<crate::Library>().invoke_navigate(k.into()),
                },
                "folder" | "group" => a.global::<crate::Library>().invoke_navigate(key),
                "playlist" if is_smart(&key) => a.global::<crate::Library>().invoke_navigate(key),
                "playlist" => a.global::<crate::Panel>().invoke_playlist_load(key),
                "bookmark" => {
                    if let Some((path, t)) = crate::bookmarks::parse_lib_key(&key) {
                        crate::bookmarks::open_at(&p, &path, t);
                        if a.global::<crate::Library>().get_full()
                            || !a.global::<crate::Playback>().get_has_media()
                        {
                            a.global::<crate::Overlays>().set_library_open(false);
                        }
                        a.global::<crate::Library>().set_full(false);
                    }
                }
                _ => {
                    // Play the item and queue the rest of the current list after it.
                    p.open(&key);
                    let items = lib.borrow().models.items.clone();
                    let rest: Vec<String> = items
                        .iter()
                        .skip_while(|r| r.key != key)
                        .skip(1)
                        .filter(|r| r.kind == "audio" && kind == "audio")
                        .map(|r| r.key.to_string())
                        .collect();
                    for r in rest {
                        p.enqueue(&r);
                    }
                    // The docked sidebar stays next to the video; the full view closes.
                    if a.global::<crate::Library>().get_full()
                        || !a.global::<crate::Playback>().get_has_media()
                    {
                        a.global::<crate::Overlays>().set_library_open(false);
                    }
                    a.global::<crate::Library>().set_full(false);
                }
            }
        });
    }
    {
        let p = player.clone();
        let w = app.as_weak();
        let cfg = cfg.clone();
        let render = render.clone();
        let lib = lib.clone();
        app.global::<crate::Library>()
            .on_menu(move |key, kind, action| {
                let Some(a) = w.upgrade() else { return };
                let path = key.strip_prefix("video:").unwrap_or(&key).to_string();
                if is_smart(&key) {
                    smart_action(&a, &p, &lib, &key, &action);
                    render();
                    return;
                }
                match action.as_str() {
                    "similar" => a
                        .global::<crate::Library>()
                        .invoke_navigate(format!("similar:{path}").into()),
                    "play" => a.global::<crate::Library>().invoke_open(key, kind),
                    // remove_at re-renders this page (bookmarks::changed).
                    "delete-bookmark" => {
                        if let Some((path, t)) = crate::bookmarks::parse_lib_key(&key) {
                            crate::bookmarks::remove_at(&path, t);
                        }
                    }
                    "enqueue" => {
                        if a.global::<crate::Playback>().get_has_media() {
                            p.enqueue(&path);
                        } else {
                            p.open(&path);
                        }
                        a.global::<crate::Notices>().set_resume_text(
                            format!("Added “{}” to the queue", name_of(&path)).into(),
                        );
                    }
                    "pin" | "unpin" => {
                        let pin = action == "pin";
                        cfg.update(move |c| {
                            c.library.pinned.retain(|x| x != &path);
                            if pin {
                                c.library.pinned.push(path);
                            }
                        });
                        let _ = cfg.save();
                        render();
                    }
                    "forget" => {
                        store::forget_position(&path);
                        render();
                    }
                    "clear-recent" | "refresh-recent" => {
                        if action == "clear-recent" {
                            store::clear_recent();
                        }
                        a.global::<crate::Library>()
                            .set_recent_files(crate::app::session::recent_items());
                        render();
                    }
                    "forget-recent" => {
                        // The hover × passes the stored target; the context menu passes the
                        // library key ("video:<folder>" for folders).
                        store::remove_recent(&key);
                        store::remove_recent(&path);
                        a.global::<crate::Library>()
                            .set_recent_files(crate::app::session::recent_items());
                        render();
                    }
                    "watched" => {
                        let is = cfg.config().library.watched.contains(&path)
                            || lib
                                .try_borrow()
                                .is_ok_and(|l| library::progress(&l.find(&path)) >= 0.95);
                        let p2 = path.clone();
                        cfg.update(move |c| {
                            c.library.watched.retain(|x| x != &p2);
                            if !is {
                                c.library.watched.push(p2);
                            }
                        });
                        if is {
                            store::forget_position(&path);
                        }
                        let _ = cfg.save();
                        render();
                    }
                    "reveal" => reveal(&path),
                    "source" => {
                        if let Some(d) = Path::new(&path).parent() {
                            a.global::<crate::Library>()
                                .invoke_navigate(format!("video:{}", d.to_string_lossy()).into());
                        }
                    }
                    "sub" | "audio" => {
                        let sub = action == "sub";
                        let mut dlg = rfd::FileDialog::new().set_title(if sub {
                            "Add subtitle tracks"
                        } else {
                            "Add audio tracks"
                        });
                        if let Some(d) = Path::new(&path).parent() {
                            dlg = dlg.set_directory(d);
                        }
                        if let Some(files) = dlg.pick_files() {
                            let files: Vec<String> = files
                                .iter()
                                .map(|f| f.to_string_lossy().into_owned())
                                .collect();
                            if p.get("path").as_deref() != Some(path.as_str()) {
                                p.open(&path);
                            }
                            PENDING.with(|q| {
                                q.borrow_mut().extend(files.into_iter().map(|f| (sub, f)))
                            });
                            flush_pending(&p);
                        }
                    }
                    "shuffle" => {
                        a.global::<crate::Library>().invoke_open(key, kind);
                        let p = p.clone();
                        slint::Timer::single_shot(
                            std::time::Duration::from_millis(400),
                            move || p.run("playlist-shuffle"),
                        );
                    }
                    "export" => {
                        let name = Path::new(&path).file_name().map(|n| n.to_owned());
                        let mut dlg = rfd::FileDialog::new().set_title("Export playlist");
                        if let Some(n) = name {
                            dlg = dlg.set_file_name(n.to_string_lossy());
                        }
                        if let Some(to) = dlg.save_file() {
                            match std::fs::copy(&path, &to) {
                                Ok(_) => a.global::<crate::Notices>().set_resume_text(
                                    format!("Exported to {}", to.display()).into(),
                                ),
                                Err(e) => eprintln!("vivi: export playlist: {e}"),
                            }
                        }
                    }
                    // Playlists an extension keeps in sync (`#VIVI-SOURCE`).
                    "playlist-sync" | "playlist-open" => {
                        let Some(src) = store::playlist_source(Path::new(&path)) else {
                            return;
                        };
                        let ok = crate::plugins::run(
                            &src.plugin,
                            &action,
                            std::slice::from_ref(&src.source),
                        );
                        if !ok && action == "playlist-open" && !src.url.is_empty() {
                            crate::settings::open_url(&src.url);
                        } else if !ok {
                            crate::plugins::toast(
                                &a,
                                "The extension that made this playlist is off",
                            );
                        } else if action == "playlist-sync" {
                            a.global::<crate::Notices>()
                                .set_resume_text(format!("Syncing “{}”…", name_of(&path)).into());
                        }
                    }
                    "delete" => {
                        a.global::<crate::Panel>().invoke_playlist_delete(key);
                        render();
                    }
                    "addpl" | "newpl" => {}
                    x if x.starts_with("addpl:") => {
                        let pl = &x["addpl:".len()..];
                        use std::io::Write;
                        let res = std::fs::OpenOptions::new()
                            .append(true)
                            .open(pl)
                            .and_then(|mut f| writeln!(f, "{path}"));
                        match res {
                            Ok(()) => a.global::<crate::Notices>().set_resume_text(
                                format!("Added to “{}”", store::display_name(pl)).into(),
                            ),
                            Err(e) => eprintln!("vivi: add to playlist: {e}"),
                        }
                        render();
                    }
                    "add-file" => {
                        let d = rfd::AsyncFileDialog::new()
                            .set_title("Add files to the queue")
                            .set_directory(&path);
                        let (p, w) = (p.clone(), a.as_weak());
                        crate::filepick::spawn(
                            async move {
                                d.pick_files()
                                    .await
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|f| f.path().to_path_buf())
                                    .collect()
                            },
                            move |files| {
                                let Some(a) = w.upgrade() else { return };
                                for (i, f) in files.iter().enumerate() {
                                    let f = f.to_string_lossy();
                                    if i == 0 && !a.global::<crate::Playback>().get_has_media() {
                                        p.open(&f);
                                    } else {
                                        p.enqueue(&f);
                                    }
                                }
                            },
                        );
                    }
                    "add-folder" => a.global::<crate::Library>().invoke_add_folder(),
                    "edit-subs" | "edit-audio" => {
                        a.global::<crate::Settings>()
                            .set_category(crate::settings::category(if action == "edit-subs" {
                                "Subtitles"
                            } else {
                                "Audio"
                            }));
                        a.global::<crate::Overlays>().set_settings_open(true);
                        a.global::<crate::Settings>().invoke_refresh(
                            a.global::<crate::Settings>().get_category(),
                            "".into(),
                            false,
                        );
                    }
                    "resync" => a.global::<crate::Library>().invoke_rescan(),
                    "info" => {
                        let (title, rows) = info(&lib.borrow(), &key, &kind);
                        a.global::<crate::Dialogs>().set_info_title(title.into());
                        a.global::<crate::Dialogs>()
                            .set_info_keys(strings(rows.iter().map(|r| r.0.to_string())));
                        a.global::<crate::Dialogs>()
                            .set_info_values(strings(rows.into_iter().map(|r| r.1)));
                        a.global::<crate::Overlays>().set_info_open(true);
                    }
                    _ => {}
                }
            });
    }
    {
        let p = player.clone();
        let w = app.as_weak();
        app.global::<crate::Playback>().on_open_url(move |url| {
            let url = url.trim().to_string();
            if url.is_empty() {
                return;
            }
            if log {
                eprintln!("open url {url}");
            }
            p.open(&url);
            if let Some(a) = w.upgrade() {
                a.global::<crate::Overlays>().set_network_open(false);
                a.global::<crate::Overlays>().set_library_open(false);
            }
        });
    }
    {
        // Compact library in the side panel.
        let w = app.as_weak();
        let lib = lib.clone();
        app.global::<crate::Panel>().on_lib_search(move |q| {
            let Some(a) = w.upgrade() else { return };
            // Newest first; a few pages' worth, the panel shows 60 after de-duplication.
            let v: Vec<Media> = match lib.try_borrow() {
                Ok(l) => l.rows(
                    &library::Query {
                        search: q.to_string(),
                        sort: "newest".into(),
                        limit: 240,
                        ..Default::default()
                    },
                    &Config::default(),
                ),
                Err(_) => return,
            };
            // One row per file: the same file reached twice (symlinked or overlapping
            // library folders) or copied into two folders (same title and size).
            let mut seen_path = HashSet::new();
            let mut seen_file = HashSet::new();
            let items: Vec<Item> = v
                .iter()
                .filter(|m| {
                    let real = std::fs::canonicalize(&m.path)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| m.path.clone());
                    seen_path.insert(real) && seen_file.insert((m.title.to_lowercase(), m.size))
                })
                .take(60)
                .map(|m| Item {
                    key: m.path.clone().into(),
                    label: m.title.clone().into(),
                    selected: false,
                })
                .collect();
            a.global::<crate::Panel>()
                .set_lib(ModelRc::new(VecModel::from(items)));
        });
    }

    {
        let rescan = rescan.clone();
        app.global::<crate::Library>().on_rescan(move || rescan());
    }
    {
        let cfg = cfg.clone();
        let rescan = rescan.clone();
        app.global::<crate::Library>().on_add_folder(move || {
            let Some(d) = rfd::FileDialog::new()
                .set_title("Add a folder to the library")
                .pick_folder()
            else {
                return;
            };
            let d = d.to_string_lossy().replace('\\', "/");
            cfg.update(move |c| {
                if !c.library.folders.contains(&d) {
                    c.library.folders.push(d);
                }
            });
            let _ = cfg.save();
            rescan();
        });
    }
    {
        let w = app.as_weak();
        app.global::<crate::Library>().on_create_playlist(move || {
            // Figma "New Playlist" dialog (src/dialogs.rs saves it).
            if let Some(a) = w.upgrade() {
                a.global::<crate::Overlays>().set_new_playlist_open(true);
            }
        });
    }
    // Home is built once from the stored index; the folders are re-checked a few seconds
    // later (stat only, tags of changed files) and Home is rebuilt only if something changed.
    render();
    slint::Timer::single_shot(std::time::Duration::from_secs(3), move || rescan_with(true));
}

thread_local! {
    /// Subtitle/audio files to attach once the chosen file has loaded: (is_sub, path).
    static PENDING: RefCell<Vec<(bool, String)>> = const { RefCell::new(Vec::new()) };
}

/// Attach pending external tracks if a file is loaded (called again on FileLoaded).
pub fn flush_pending(p: &Player) {
    if p.get("duration").is_none() {
        return;
    }
    for (sub, f) in PENDING.with(|q| std::mem::take(&mut *q.borrow_mut())) {
        p.cmd(if sub { "sub-add" } else { "audio-add" }, &[&f, "select"]);
    }
}

/// Show a file (or folder) in the system file manager.
pub(crate) fn reveal(path: &str) {
    let p = Path::new(path);
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg("-R").arg(p).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer")
        .arg(format!("/select,{}", p.display()))
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let r = {
        // org.freedesktop.FileManager1 selects the file; xdg-open on the folder otherwise.
        let uri = format!("file://{}", p.display());
        std::process::Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{uri}"),
                "string:",
            ])
            .status()
            .ok()
            .filter(|s| s.success())
            .map(|_| ())
            .ok_or(())
            .or_else(|_| {
                let dir = if p.is_dir() {
                    p
                } else {
                    p.parent().unwrap_or(p)
                };
                std::process::Command::new("xdg-open")
                    .arg(dir)
                    .spawn()
                    .map(|_| ())
            })
    };
    if let Err(e) = r {
        eprintln!("vivi: show in file manager: {e:?}");
    }
}

/// "Properties & Metadata" rows for a library item.
fn info(l: &Lib, key: &str, kind: &str) -> (String, Vec<(&'static str, String)>) {
    let path = key.strip_prefix("video:").unwrap_or(key);
    match kind {
        "folder" => {
            let (n, size, dur) = l.db.as_ref().map_or((0, 0, 0.0), |db| {
                library::totals(
                    db,
                    &library::Query {
                        under: Some(path.into()),
                        ..Default::default()
                    },
                )
            });
            (
                name_of(path),
                vec![
                    ("Location", path.to_string()),
                    ("Media files", n.to_string()),
                    ("Total size", library::human_size(size)),
                    ("Total length", library::human_time(dur)),
                ],
            )
        }
        "playlist" => {
            let n = std::fs::read_to_string(path).map_or(0, |s| {
                s.lines()
                    .filter(|l| !l.starts_with('#') && !l.is_empty())
                    .count()
            });
            (
                store::display_name(path),
                vec![("File", path.to_string()), ("Items", n.to_string())],
            )
        }
        "group" => {
            let parts: Vec<&str> = key.splitn(3, ':').collect();
            let (tab, val) = (
                parts.get(1).copied().unwrap_or(""),
                parts.get(2).copied().unwrap_or(""),
            );
            let field = match tab {
                "albums" => library::Field::Album,
                "artists" => library::Field::Artist,
                _ => library::Field::Genre,
            };
            let n = l.count(&library::Query {
                kind: Some(MediaKind::Audio),
                field: Some((field, val.into())),
                ..Default::default()
            });
            (val.to_string(), vec![("Songs", n.to_string())])
        }
        _ => {
            let m = l.find(path);
            let mut v = vec![
                ("Location", m.path.clone()),
                ("Type", m.kind.as_str().to_string()),
            ];
            if m.duration > 0.0 {
                v.push(("Length", library::human_time(m.duration)));
            }
            if m.size > 0 {
                v.push(("Size", library::human_size(m.size)));
            }
            for (k, val) in [
                ("Artist", &m.artist),
                ("Album", &m.album),
                ("Genre", &m.genre),
            ] {
                if !val.is_empty() {
                    v.push((k, val.clone()));
                }
            }
            if m.track > 0 {
                v.push(("Track", m.track.to_string()));
            }
            let pr = library::progress(&m);
            if pr > 0.0 {
                v.push(("Watched", format!("{:.0}%", pr * 100.0)));
            }
            (m.title, v)
        }
    }
}

thread_local! {
    /// A scan changed the index: re-read the totals on the next refresh.
    static RELOAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Indexed files per directory (`None` until the index is read), for the folder
    /// list on Settings → Library.
    static DIRS: RefCell<Option<std::collections::BTreeMap<String, usize>>> =
        const { RefCell::new(None) };
}

/// Indexed files in library folder `folder` and its sub-folders (`None` before the
/// index is read).
pub fn folder_count(folder: &str) -> Option<usize> {
    let f = folder.trim_end_matches(['/', '\\']);
    DIRS.with(|d| {
        d.borrow().as_ref().map(|d| {
            d.range(f.to_string()..)
                .take_while(|(k, _)| k.starts_with(f))
                .filter(|(k, _)| k.len() == f.len() || k[f.len()..].starts_with(['/', '\\']))
                .map(|(_, n)| n)
                .sum()
        })
    })
}

/// Show `img` on every library row whose `thumb_key` is `key`.
fn set_thumb(key: &str, img: &slint::Image) {
    MODELS.with(|ms| {
        for m in ms.borrow().iter() {
            m.set_thumb(key, img);
        }
    });
}

/// The picture for `path` if it is in the image cache; otherwise it is decoded on the
/// image worker and the rows keyed by `path` are filled when it is ready.
fn picture(path: &str) -> Option<slint::Image> {
    thumb(path, Path::new(path))
}

/// The thumbnail of the rows keyed `key`, from the image file `src`: from memory when
/// it is there, else decoded on the worker (the rows are filled when it is ready). While
/// the browsed list is built ([`LAZY`]) nothing is queued: rows get their picture once
/// they scroll into view.
fn thumb(key: &str, src: &Path) -> Option<slint::Image> {
    SRCS.with(|m| m.borrow_mut().insert(key.into(), src.to_path_buf()));
    if LAZY.get() {
        return images::peek(src, images::TILE);
    }
    let k = key.to_string();
    images::get(src, images::TILE, move |i| set_thumb(&k, &i))
}

/// Whether a row near the view (or on Home) shows thumbnail `key`.
fn wanted(key: &str) -> bool {
    MODELS.with(|ms| {
        ms.borrow().iter().any(|t| {
            t.rows.get(key).is_some_and(|rows| {
                rows.iter()
                    .any(|&i| t.window.is_none_or(|(lo, hi)| i >= lo && i < hi))
            })
        })
    })
}

/// The browsed list shows rows `lo..hi`: decode their thumbnails and drop the pictures
/// of rows far from the view, so memory follows what is on screen, not the folder size.
pub fn thumb_window(lo: usize, hi: usize) {
    SEEN.set((lo, hi));
    let model = MODELS.with(|ms| {
        let mut ms = ms.borrow_mut();
        let t = ms.first_mut()?;
        let n = t.model.row_count();
        let w = (
            lo.saturating_sub(WINDOW_MARGIN),
            (hi + WINDOW_MARGIN).min(n),
        );
        t.window = Some(w);
        Some((t.model.clone(), w))
    });
    let Some((model, (lo, hi))) = model else {
        return;
    };
    for i in 0..model.row_count() {
        let Some(mut r) = model.row_data(i) else {
            continue;
        };
        let inside = i >= lo && i < hi;
        if inside && !r.has_thumb && !r.thumb_key.is_empty() {
            let src = SRCS.with(|m| m.borrow().get(&r.thumb_key).cloned());
            if let Some(src) = src.filter(|p| p.is_file()) {
                if let Some(img) = thumb(r.thumb_key.as_str(), &src) {
                    r.thumb = img;
                    r.has_thumb = true;
                    model.set_row_data(i, r);
                }
            }
        } else if !inside && r.has_thumb {
            r.thumb = slint::Image::default();
            r.has_thumb = false;
            model.set_row_data(i, r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_counts_include_sub_folders() {
        assert_eq!(folder_count("/m"), None);
        let d: std::collections::BTreeMap<String, usize> =
            [("/m", 1), ("/m/x", 1), ("/m2", 1), ("/mx", 1)]
                .into_iter()
                .map(|(k, n)| (k.to_string(), n))
                .collect();
        DIRS.with(|x| *x.borrow_mut() = Some(d));
        assert_eq!(folder_count("/m"), Some(2));
        assert_eq!(folder_count("/m/"), Some(2));
        assert_eq!(folder_count("/m2"), Some(1));
        assert_eq!(folder_count("/none"), Some(0));
    }

    fn rows(n: usize) -> Vec<LibItem> {
        (0..n)
            .map(|i| LibItem {
                thumb_key: format!("/m/{}.mkv", i % (n / 2).max(1)).into(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn thumb_index_fills_every_row_of_its_key() {
        let t = ThumbModel {
            model: Rc::new(VecModel::from(rows(10))),
            rows: thumb_rows(&rows(10)),
            window: None,
        };
        t.set_thumb("/m/3.mkv", &slint::Image::default());
        let lit: Vec<usize> = (0..t.model.row_count())
            .filter(|&i| t.model.row_data(i).is_some_and(|r| r.has_thumb))
            .collect();
        assert_eq!(lit, vec![3, 8]);
    }

    /// `cargo test --release thumb_index_perf -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn thumb_index_perf() {
        const N: usize = 20_000;
        const K: usize = 2_000;
        let img = slint::Image::default();
        let keys: Vec<String> = (0..K).map(|i| format!("/m/{}.mkv", i * 5)).collect();
        // Before: scan every row for every finished thumbnail.
        let m = VecModel::from(rows(N));
        let t0 = std::time::Instant::now();
        for k in &keys {
            for i in 0..m.row_count() {
                if let Some(mut r) = m.row_data(i).filter(|r| r.thumb_key.as_str() == k) {
                    r.has_thumb = true;
                    r.thumb = img.clone();
                    m.set_row_data(i, r);
                }
            }
        }
        let scan = t0.elapsed();
        // After: index once per model fill, then touch only the key's rows.
        let t0 = std::time::Instant::now();
        let t = ThumbModel {
            model: Rc::new(VecModel::from(rows(N))),
            rows: thumb_rows(&rows(N)),
            window: None,
        };
        let index = t0.elapsed();
        let t0 = std::time::Instant::now();
        for k in &keys {
            t.set_thumb(k, &img);
        }
        let indexed = t0.elapsed();
        println!(
            "{K} thumbnails over {N} rows: scan {scan:?}, index build {index:?} + lookups {indexed:?}"
        );
    }

    #[test]
    fn nav_sections() {
        assert_eq!(Nav::parse("home"), (Nav::Home, ""));
        assert_eq!(Nav::parse("video:/m/Films"), (Nav::Video, "/m/Films"));
        assert_eq!(Nav::parse("music:artists:A:B"), (Nav::Music, "artists:A:B"));
        assert_eq!(Nav::parse("service:radio"), (Nav::Service, "radio"));
        assert_eq!(Nav::parse("whatever"), (Nav::Home, ""));
    }
}
