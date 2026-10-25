//! Library UI controller: navigation, search, sorting, home sections, context menu, thumbnails.

use crate::config::Config;
use crate::library::{self, Media};
use crate::player::Player;
use crate::{images, store, AppWindow, Item, LibItem};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

thread_local! {
    /// Models that may show thumbnails; updated when the worker finishes one.
    static MODELS: RefCell<Vec<Rc<VecModel<LibItem>>>> = const { RefCell::new(Vec::new()) };
}

pub struct Lib {
    media: Vec<Media>,
    nav: String,
    search: String,
    requested: HashSet<String>,
    thumbs: Option<std::sync::mpsc::Sender<Media>>,
    items: Rc<VecModel<LibItem>>,
    cont: Rc<VecModel<LibItem>>,
    recent: Rc<VecModel<LibItem>>,
    pinned: Rc<VecModel<LibItem>>,
    listen: Rc<VecModel<LibItem>>,
    folders: Rc<VecModel<LibItem>>,
    lists: Rc<VecModel<LibItem>>,
    artists: Rc<VecModel<LibItem>>,
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
        if m.kind == "audio" && !m.artist.is_empty() {
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
            kind: m.kind.clone().into(),
            progress: pr,
            pinned: cfg.library.pinned.contains(&m.path),
            watched: pr >= 0.95 || cfg.library.watched.contains(&m.path),
            ..Default::default()
        };
        if cfg.library.thumbnails {
            let tp = library::thumb_path(&m.path);
            if tp.exists() {
                // Decoded on the image worker; the row is filled when it is ready.
                let key = m.path.clone();
                if let Some(img) = images::get(&tp, images::TILE, move |i| set_thumb(&key, &i)) {
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
            if let Some(img) = images::get(&c, images::TILE, move |i| set_thumb(&key, &i)) {
                it.thumb = img;
                it.has_thumb = true;
            }
        }
    }

    fn matches(&self, m: &Media) -> bool {
        let q = self.search.to_lowercase();
        q.is_empty()
            || [&m.title, &m.artist, &m.album, &m.genre, &m.path]
                .iter()
                .any(|s| s.to_lowercase().contains(&q))
    }

    /// Rebuild the current view. Returns breadcrumb (key, label) pairs.
    fn build(&mut self, cfg: &Config) -> (Vec<LibItem>, Vec<(String, String)>) {
        let nav = self.nav.clone();
        let (section, arg) = nav.split_once(':').unwrap_or((nav.as_str(), ""));
        let sort = cfg.library.sort.as_str();
        let watched =
            |m: &Media| library::progress(m) >= 0.95 || cfg.library.watched.contains(&m.path);
        let mut out = Vec::new();
        let mut crumbs = Vec::new();
        match section {
            "video" => {
                crumbs.push(("video".into(), "Videos".into()));
                let mut files: Vec<Media> = self
                    .media
                    .iter()
                    .filter(|m| m.kind == "video")
                    .cloned()
                    .collect();
                if !self.search.is_empty() {
                    files.retain(|m| self.matches(m));
                } else if arg.is_empty() {
                    // Roots
                    out.push(Self::action("action:add-folder", "Add", "add"));
                    for r in &cfg.library.folders {
                        let n = files
                            .iter()
                            .filter(|m| m.path.starts_with(&format!("{r}/")))
                            .count();
                        if n > 0 {
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
                    files.clear();
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
                    let prefix = format!("{arg}/");
                    let mut dirs: std::collections::BTreeMap<String, usize> = Default::default();
                    files.retain(|m| {
                        let Some(rest) = m.path.strip_prefix(&prefix) else {
                            return false;
                        };
                        match rest.split_once('/') {
                            Some((d, _)) => {
                                *dirs.entry(d.to_string()).or_default() += 1;
                                false
                            }
                            None => true,
                        }
                    });
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
                }
                library::sort_with(&mut files, sort, &watched);
                for m in &files {
                    out.push(self.item(m, cfg));
                }
            }
            "music" if arg.is_empty() && self.search.is_empty() => {
                crumbs.push(("music".into(), "Music".into()));
                out.push(Self::action("action:add-folder", "Add", "add"));
                out.push(Self::action("music:songs", "Songs", "songs"));
                out.push(Self::action("music:albums", "Albums", "album"));
                out.push(Self::action("music:artists", "Artists", "artist"));
                out.push(Self::action("music:genres", "Genres", "genre"));
            }
            // The Streams hub and service pages (src/streams): drawn by the hub, no items.
            "streams" => crumbs.push(("streams".into(), "Streams".into())),
            "service" => {
                crumbs.push(("streams".into(), "Streams".into()));
                crumbs.push((nav.clone(), crate::streams::service_name(arg)));
            }
            "recent" => {
                crumbs.push(("recent".into(), "Recent".into()));
                for e in store::recent_entries() {
                    out.push(self.recent_item(&e, cfg));
                }
            }
            "music" => {
                crumbs.push(("music".into(), "Music".into()));
                let mut songs: Vec<Media> = self
                    .media
                    .iter()
                    .filter(|m| m.kind == "audio" && self.matches(m))
                    .cloned()
                    .collect();
                let (tab, val) = arg.split_once(':').unwrap_or((arg, ""));
                let tab = if tab.is_empty() { "songs" } else { tab };
                let field = |m: &Media| -> String {
                    let v = match tab {
                        "albums" => &m.album,
                        "artists" => &m.artist,
                        _ => &m.genre,
                    };
                    if v.is_empty() {
                        "Unknown".into()
                    } else {
                        v.clone()
                    }
                };
                let label = match tab {
                    "albums" => "Albums",
                    "artists" => "Artists",
                    "genres" => "Genres",
                    _ => "Songs",
                };
                crumbs.push((format!("music:{tab}"), label.into()));
                if tab != "songs" && val.is_empty() {
                    let mut groups: std::collections::BTreeMap<String, (usize, String)> =
                        Default::default();
                    for m in &songs {
                        let e = groups.entry(field(m)).or_insert((0, m.path.clone()));
                        e.0 += 1;
                    }
                    for (g, (n, first)) in groups {
                        let mut it = Self::group(
                            format!("music:{tab}:{g}"),
                            &g,
                            format!("{n} songs"),
                            "group",
                            false,
                        );
                        // Use the first song's cover as the group thumbnail.
                        if let Some(m) = songs.iter().find(|m| m.path == first).cloned() {
                            let t = self.item(&m, cfg);
                            it.thumb = t.thumb;
                            it.has_thumb = t.has_thumb;
                            it.thumb_key = t.thumb_key;
                        }
                        out.push(it);
                    }
                    songs.clear();
                } else if tab != "songs" {
                    songs.retain(|m| field(m) == val);
                    crumbs.push((nav.clone(), val.into()));
                }
                library::sort_with(&mut songs, sort, &watched);
                if tab == "albums" && sort == "name" {
                    songs.sort_by_key(|m| m.track);
                }
                for m in &songs {
                    out.push(self.item(m, cfg));
                }
            }
            // The shared Downloads panel (ui/downloads.slint) draws its own list.
            "downloads" => {
                crumbs.push(("downloads".into(), "Downloads".into()));
            }
            "playlists" => {
                crumbs.push(("playlists".into(), "Playlists".into()));
                out.push(Self::action(
                    "action:create-playlist",
                    "Create playlist",
                    "add",
                ));
                let names: Vec<(String, String)> = crate::plugins::manifests()
                    .into_iter()
                    .map(|m| (m.id, m.name))
                    .collect();
                for (name, path) in store::playlists() {
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
                    let mut all: Vec<Media> = self
                        .media
                        .iter()
                        .filter(|m| self.matches(m))
                        .cloned()
                        .collect();
                    library::sort_with(&mut all, sort, &watched);
                    for m in all.iter().take(200) {
                        out.push(self.item(m, cfg));
                    }
                } else {
                    // Recently added
                    let mut all = self.media.clone();
                    all.sort_by_key(|m| std::cmp::Reverse(m.mtime));
                    for m in all.iter().take(12) {
                        out.push(self.item(m, cfg));
                    }
                }
            }
        }
        (out, crumbs)
    }

    fn find(&self, path: &str) -> Media {
        self.media
            .iter()
            .find(|m| m.path == path)
            .cloned()
            .unwrap_or_else(|| Media {
                title: Path::new(path)
                    .file_stem()
                    .map_or(path.into(), |s| s.to_string_lossy().into_owned()),
                kind: if library::kind_of(Path::new(path)) == Some("audio") {
                    "audio"
                } else {
                    "video"
                }
                .into(),
                size: std::fs::metadata(path).map_or(0, |m| m.len() as i64),
                path: path.into(),
                ..Default::default()
            })
    }

    fn home(&mut self, cfg: &Config) {
        let mut cont: Vec<Media> = self
            .media
            .iter()
            .filter(|m| m.kind == "video")
            .filter(|m| {
                let p = library::progress(m);
                p > 0.0 && p < 0.95 && !cfg.library.watched.contains(&m.path)
            })
            .cloned()
            .collect();
        // Files outside the library that were played recently also count.
        for p in store::recent() {
            if !cont.iter().any(|m| m.path == p)
                && library::kind_of(Path::new(&p)) != Some("audio")
                && !cfg.library.watched.contains(&p)
                && store::resume_position(&p).is_some()
                && Path::new(&p).exists()
            {
                cont.push(self.find(&p));
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
            .filter(|p| library::kind_of(Path::new(p)) == Some("audio") && Path::new(p).exists())
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
        let mut count: std::collections::HashMap<String, usize> = Default::default();
        for m in self
            .media
            .iter()
            .filter(|m| m.kind == "audio" && !m.artist.is_empty())
        {
            *count.entry(m.artist.clone()).or_default() += 1;
        }
        for p in store::recent() {
            if let Some(m) = self
                .media
                .iter()
                .find(|m| m.path == p && !m.artist.is_empty())
            {
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
        self.listen.set_vec(listen);
        self.folders.set_vec(folders);
        self.lists.set_vec(lists);
        self.artists.set_vec(artists);
        self.cont.set_vec(cont);
        self.recent.set_vec(recent);
        self.pinned.set_vec(pinned);
    }
}

use crate::util::strings;

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>, log: bool) {
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
        m.borrow_mut().extend([
            items.clone(),
            cont.clone(),
            recent.clone(),
            pinned.clone(),
            listen.clone(),
        ])
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

    let thumbs = (cfg.borrow().library.thumbnails && library::ffmpeg_available()).then(|| {
        library::thumb_worker(move |path, file| {
            let _ = slint::invoke_from_event_loop(move || {
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
    let media = library::open_db(&db)
        .map(|c| library::all(&c))
        .unwrap_or_default();
    let lib = Rc::new(RefCell::new(Lib {
        media,
        nav: "home".into(),
        search: String::new(),
        requested: HashSet::new(),
        thumbs,
        items,
        cont,
        recent,
        pinned,
        listen,
        folders,
        lists,
        artists,
        back: Vec::new(),
        fwd: Vec::new(),
    }));

    let render: Rc<dyn Fn()> = {
        let w = app.as_weak();
        let lib = lib.clone();
        let cfg = cfg.clone();
        Rc::new(move || {
            let Some(a) = w.upgrade() else { return };
            let c = cfg.borrow();
            let mut l = lib.borrow_mut();
            let (v, mut crumbs) = l.build(&c);
            // Figma breadcrumb: long paths keep the last two levels behind "...".
            if crumbs.len() > 3 {
                let keep = crumbs.split_off(crumbs.len() - 2);
                let up = crumbs.pop().map(|c| c.0).unwrap_or_default();
                crumbs = std::iter::once((up, "...".to_string()))
                    .chain(keep)
                    .collect();
            }
            l.items.set_vec(v);
            l.home(&c);
            a.global::<crate::Library>().set_nav(l.nav.clone().into());
            a.global::<crate::Library>()
                .set_crumb_keys(strings(crumbs.iter().map(|c| c.0.clone())));
            a.global::<crate::Library>()
                .set_crumb_labels(strings(crumbs.into_iter().map(|c| c.1)));
            a.global::<crate::Library>()
                .set_sort(c.library.sort.clone().into());
            a.global::<crate::Library>()
                .set_grid(c.library.view != "list");
            a.global::<crate::Library>().set_count(l.media.len() as i32);
            a.global::<crate::Library>()
                .set_can_back(!l.back.is_empty());
            a.global::<crate::Library>()
                .set_can_forward(!l.fwd.is_empty());
            let nav = l.nav.clone();
            drop(l);
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
            let roots = cfg.borrow().library.folders.clone();
            if let Some(a) = w.upgrade().filter(|_| !quiet) {
                a.global::<crate::Library>().set_scanning(true);
            }
            let w = w.clone();
            let db = db.clone();
            std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let res = library::open_db(&db).and_then(|mut c| {
                    library::scan_changes(&mut c, &roots).map(|(n, changed)| {
                        (
                            n,
                            changed,
                            (!quiet || changed > 0).then(|| library::all(&c)),
                        )
                    })
                });
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(a) = w.upgrade() else { return };
                    a.global::<crate::Library>().set_scanning(false);
                    match res {
                        Ok((n, changed, media)) => {
                            if log {
                                eprintln!(
                                    "library scan: {n} files, {changed} changed in {:?}",
                                    t.elapsed()
                                );
                            }
                            if let Some(media) = media {
                                LOADED.with(|l| *l.borrow_mut() = Some(media));
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
            if let Some(m) = LOADED.with(|l| l.borrow_mut().take()) {
                lib.borrow_mut().media = m;
            }
            render();
        });
    }
    {
        let lib = lib.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_navigate(move |key| {
            // The 0.13 Network section is the Streams hub now.
            let key: slint::SharedString = if key == "network" {
                "streams".into()
            } else {
                key
            };
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
            cfg.borrow_mut().library.sort = s.to_string();
            let _ = cfg.borrow().save();
            render();
        });
    }
    {
        let cfg = cfg.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_set_grid(move |g| {
            cfg.borrow_mut().library.view = if g { "grid" } else { "list" }.into();
            let _ = cfg.borrow().save();
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
                    "action:network" => crate::streams::open_page(&a, "action:network"),
                    "action:create-playlist" => {
                        a.global::<crate::Library>().invoke_create_playlist()
                    }
                    k => a.global::<crate::Library>().invoke_navigate(k.into()),
                },
                "folder" | "group" => a.global::<crate::Library>().invoke_navigate(key),
                "playlist" => a.global::<crate::Panel>().invoke_playlist_load(key),
                _ => {
                    // Play the item and queue the rest of the current list after it.
                    p.open(&key);
                    let l = lib.borrow();
                    let rest: Vec<String> = l
                        .items
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
                match action.as_str() {
                    "play" => a.global::<crate::Library>().invoke_open(key, kind),
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
                        {
                            let mut c = cfg.borrow_mut();
                            c.library.pinned.retain(|x| x != &path);
                            if action == "pin" {
                                c.library.pinned.push(path);
                            }
                        }
                        let _ = cfg.borrow().save();
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
                        let is = cfg.borrow().library.watched.contains(&path)
                            || lib
                                .borrow()
                                .media
                                .iter()
                                .any(|m| m.path == path && library::progress(m) >= 0.95);
                        {
                            let mut c = cfg.borrow_mut();
                            c.library.watched.retain(|x| x != &path);
                            if !is {
                                c.library.watched.push(path.clone());
                            }
                        }
                        if is {
                            store::forget_position(&path);
                        }
                        let _ = cfg.borrow().save();
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
                        if let Some(files) = rfd::FileDialog::new()
                            .set_title("Add files to the queue")
                            .set_directory(&path)
                            .pick_files()
                        {
                            for (i, f) in files.iter().enumerate() {
                                let f = f.to_string_lossy();
                                if i == 0 && !a.global::<crate::Playback>().get_has_media() {
                                    p.open(&f);
                                } else {
                                    p.enqueue(&f);
                                }
                            }
                        }
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
            let l = lib.borrow();
            let q = q.to_lowercase();
            let mut v: Vec<&Media> = l
                .media
                .iter()
                .filter(|m| {
                    q.is_empty()
                        || m.title.to_lowercase().contains(&q)
                        || m.artist.to_lowercase().contains(&q)
                        || m.path.to_lowercase().contains(&q)
                })
                .collect();
            v.sort_by_key(|m| std::cmp::Reverse(m.mtime));
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
            {
                let mut c = cfg.borrow_mut();
                if !c.library.folders.contains(&d) {
                    c.library.folders.push(d);
                }
            }
            let _ = cfg.borrow().save();
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
            let inside: Vec<&Media> = l
                .media
                .iter()
                .filter(|m| m.path.starts_with(&format!("{path}/")))
                .collect();
            let size: i64 = inside.iter().map(|m| m.size).sum();
            let dur: f64 = inside.iter().map(|m| m.duration).sum();
            (
                name_of(path),
                vec![
                    ("Location", path.to_string()),
                    ("Media files", inside.len().to_string()),
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
            let n = l
                .media
                .iter()
                .filter(|m| {
                    m.kind == "audio"
                        && match tab {
                            "albums" => m.album == val,
                            "artists" => m.artist == val,
                            _ => m.genre == val,
                        }
                })
                .count();
            (val.to_string(), vec![("Songs", n.to_string())])
        }
        _ => {
            let m = l.find(path);
            let mut v = vec![("Location", m.path.clone()), ("Type", m.kind.clone())];
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
    static LOADED: RefCell<Option<Vec<Media>>> = const { RefCell::new(None) };
}

/// Show `img` on every library row whose `thumb_key` is `key`.
fn set_thumb(key: &str, img: &slint::Image) {
    MODELS.with(|ms| {
        for m in ms.borrow().iter() {
            for i in 0..m.row_count() {
                if let Some(mut r) = m.row_data(i).filter(|r| r.thumb_key.as_str() == key) {
                    r.thumb = img.clone();
                    r.has_thumb = true;
                    m.set_row_data(i, r);
                }
            }
        }
    });
}

/// The picture for `path` if it is in the image cache; otherwise it is decoded on the
/// image worker and the rows keyed by `path` are filled when it is ready.
fn picture(path: &str) -> Option<slint::Image> {
    let key = path.to_string();
    images::get(Path::new(path), images::TILE, move |i| set_thumb(&key, &i))
}
