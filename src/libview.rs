//! Library UI controller: navigation, search, sorting, home sections, context menu, thumbnails.

use crate::config::Config;
use crate::library::{self, Media};
use crate::player::Player;
use crate::{store, AppWindow, Item, LibItem};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
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
            if let Some(img) = tp.exists().then(|| library::load_thumb(&tp)).flatten() {
                it.thumb = img;
                it.has_thumb = true;
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
        if let Some(img) = store::playlist_cover(playlist).and_then(|c| library::load_thumb(&c)) {
            it.thumb = img;
            it.has_thumb = true;
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
            "network" => {
                crumbs.push(("network".into(), "Network".into()));
                out.push(Self::action("action:network", "Add Network Stream", "add"));
                for u in store::recent().into_iter().filter(|u| u.contains("://")) {
                    let mut it = Self::group(
                        u.clone(),
                        &store::display_name(&u),
                        u.clone(),
                        "video",
                        false,
                    );
                    it.icon = "streaming".into();
                    out.push(it);
                }
            }
            "recent" => {
                crumbs.push(("recent".into(), "Recent".into()));
                for p in store::recent() {
                    if p.contains("://") || Path::new(&p).exists() {
                        let m = self.find(&p);
                        out.push(self.item(&m, cfg));
                    }
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
            "playlists" => {
                crumbs.push(("playlists".into(), "Playlists".into()));
                out.push(Self::action(
                    "action:create-playlist",
                    "Create playlist",
                    "add",
                ));
                for (name, path) in store::playlists() {
                    let n = std::fs::read_to_string(&path).map_or(0, |s| {
                        s.lines()
                            .filter(|l| !l.starts_with('#') && !l.is_empty())
                            .count()
                    });
                    let mut it = Self::group(
                        path.to_string_lossy().into_owned(),
                        &name,
                        format!("{n} items"),
                        "playlist",
                        false,
                    );
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
        let recent: Vec<LibItem> = store::recent()
            .iter()
            .filter(|p| Path::new(p).exists())
            .take(12)
            .map(|p| {
                let m = self.find(p);
                self.item(&m, cfg)
            })
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

fn strings(v: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.into_iter().map(SharedString::from).collect::<Vec<_>>(),
    ))
}

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
        .set_lib_listen(ModelRc::from(listen.clone()));
    app.global::<crate::Library>()
        .set_lib_folders(ModelRc::from(folders.clone()));
    app.global::<crate::Library>()
        .set_lib_lists(ModelRc::from(lists.clone()));
    app.global::<crate::Library>()
        .set_lib_artists(ModelRc::from(artists.clone()));
    app.global::<crate::Library>()
        .set_lib_items(ModelRc::from(items.clone()));
    app.global::<crate::Library>()
        .set_lib_continue(ModelRc::from(cont.clone()));
    app.global::<crate::Library>()
        .set_lib_recent(ModelRc::from(recent.clone()));
    app.global::<crate::Library>()
        .set_lib_pinned(ModelRc::from(pinned.clone()));
    app.global::<crate::Library>()
        .set_ytdlp(library::ytdlp_available());

    let thumbs = (cfg.borrow().library.thumbnails && library::ffmpeg_available()).then(|| {
        library::thumb_worker(move |path, file| {
            let _ = slint::invoke_from_event_loop(move || {
                let Some(img) = library::load_thumb(&file) else {
                    return;
                };
                MODELS.with(|ms| {
                    for m in ms.borrow().iter() {
                        for i in 0..m.row_count() {
                            if let Some(mut r) =
                                m.row_data(i).filter(|r| r.thumb_key.as_str() == path)
                            {
                                r.thumb = img.clone();
                                r.has_thumb = true;
                                m.set_row_data(i, r);
                            }
                        }
                    }
                });
            });
        })
    });

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
            a.global::<crate::Library>()
                .set_lib_nav(l.nav.clone().into());
            a.global::<crate::Library>()
                .set_lib_crumb_keys(strings(crumbs.iter().map(|c| c.0.clone())));
            a.global::<crate::Library>()
                .set_lib_crumb_labels(strings(crumbs.into_iter().map(|c| c.1)));
            a.global::<crate::Library>()
                .set_lib_sort(c.library.sort.clone().into());
            a.global::<crate::Library>()
                .set_lib_grid(c.library.view != "list");
            a.global::<crate::Library>()
                .set_lib_count(l.media.len() as i32);
            a.global::<crate::Library>()
                .set_lib_can_back(!l.back.is_empty());
            a.global::<crate::Library>()
                .set_lib_can_forward(!l.fwd.is_empty());
            drop(l);
            a.global::<crate::Panel>()
                .invoke_panel_lib_search("".into());
        })
    };

    // Background scan; re-render when done.
    let rescan: Rc<dyn Fn()> = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        Rc::new(move || {
            let roots = cfg.borrow().library.folders.clone();
            if let Some(a) = w.upgrade() {
                a.global::<crate::Library>().set_lib_scanning(true);
            }
            let w = w.clone();
            let db = db.clone();
            std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let res = library::open_db(&db)
                    .and_then(|mut c| library::scan(&mut c, &roots).map(|n| (n, library::all(&c))));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(a) = w.upgrade() else { return };
                    a.global::<crate::Library>().set_lib_scanning(false);
                    match res {
                        Ok((n, media)) => {
                            if log {
                                eprintln!("library scan: {n} files in {:?}", t.elapsed());
                            }
                            LOADED.with(|l| *l.borrow_mut() = Some(media));
                            a.global::<crate::Library>().invoke_lib_refresh();
                        }
                        Err(e) => eprintln!("vivi: library scan failed: {e}"),
                    }
                });
            });
        })
    };

    {
        let lib = lib.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_lib_refresh(move || {
            if let Some(m) = LOADED.with(|l| l.borrow_mut().take()) {
                lib.borrow_mut().media = m;
            }
            render();
        });
    }
    {
        let lib = lib.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_lib_navigate(move |key| {
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
        app.global::<crate::Library>()
            .on_lib_history(move |forward| {
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
        app.global::<crate::Library>().on_lib_search(move |q| {
            lib.borrow_mut().search = q.to_string();
            render();
        });
    }
    {
        let cfg = cfg.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_lib_set_sort(move |s| {
            cfg.borrow_mut().library.sort = s.to_string();
            let _ = cfg.borrow().save();
            render();
        });
    }
    {
        let cfg = cfg.clone();
        let render = render.clone();
        app.global::<crate::Library>().on_lib_set_grid(move |g| {
            cfg.borrow_mut().library.view = if g { "grid" } else { "list" }.into();
            let _ = cfg.borrow().save();
            render();
        });
    }
    {
        let p = player.clone();
        let w = app.as_weak();
        let lib = lib.clone();
        app.global::<crate::Library>()
            .on_lib_open(move |key, kind| {
                let Some(a) = w.upgrade() else { return };
                match kind.as_str() {
                    "action" => match key.as_str() {
                        "action:add-folder" => a.global::<crate::Library>().invoke_lib_add_folder(),
                        "action:network" => a.global::<crate::Overlays>().set_network_open(true),
                        "action:create-playlist" => {
                            a.global::<crate::Library>().invoke_lib_create_playlist()
                        }
                        k => a.global::<crate::Library>().invoke_lib_navigate(k.into()),
                    },
                    "folder" | "group" => a.global::<crate::Library>().invoke_lib_navigate(key),
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
                        if a.global::<crate::Library>().get_lib_full()
                            || !a.global::<crate::Playback>().get_has_media()
                        {
                            a.global::<crate::Overlays>().set_library_open(false);
                        }
                        a.global::<crate::Library>().set_lib_full(false);
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
            .on_lib_menu(move |key, kind, action| {
                let Some(a) = w.upgrade() else { return };
                let path = key.strip_prefix("video:").unwrap_or(&key).to_string();
                match action.as_str() {
                    "play" => a.global::<crate::Library>().invoke_lib_open(key, kind),
                    "enqueue" => {
                        if a.global::<crate::Playback>().get_has_media() {
                            p.enqueue(&path);
                        } else {
                            p.open(&path);
                        }
                        a.set_resume_text(
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
                            a.global::<crate::Library>().invoke_lib_navigate(
                                format!("video:{}", d.to_string_lossy()).into(),
                            );
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
                        a.global::<crate::Library>().invoke_lib_open(key, kind);
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
                                Ok(_) => a.set_resume_text(
                                    format!("Exported to {}", to.display()).into(),
                                ),
                                Err(e) => eprintln!("vivi: export playlist: {e}"),
                            }
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
                            Ok(()) => a.set_resume_text(
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
                    "add-folder" => a.global::<crate::Library>().invoke_lib_add_folder(),
                    "edit-subs" | "edit-audio" => {
                        a.set_settings_category(crate::settings::category(
                            if action == "edit-subs" {
                                "Subtitles"
                            } else {
                                "Audio"
                            },
                        ));
                        a.global::<crate::Overlays>().set_settings_open(true);
                        a.invoke_settings_refresh(a.get_settings_category(), "".into(), false);
                    }
                    "resync" => a.global::<crate::Library>().invoke_lib_rescan(),
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
        app.on_open_url(move |url| {
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
        app.global::<crate::Panel>().on_panel_lib_search(move |q| {
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
            let items: Vec<Item> = v
                .iter()
                .take(60)
                .map(|m| Item {
                    key: m.path.clone().into(),
                    label: m.title.clone().into(),
                    selected: false,
                })
                .collect();
            a.global::<crate::Panel>()
                .set_panel_lib(ModelRc::new(VecModel::from(items)));
        });
    }

    {
        let rescan = rescan.clone();
        app.global::<crate::Library>()
            .on_lib_rescan(move || rescan());
    }
    {
        let cfg = cfg.clone();
        let rescan = rescan.clone();
        app.global::<crate::Library>().on_lib_add_folder(move || {
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
        app.global::<crate::Library>()
            .on_lib_create_playlist(move || {
                // Figma "New Playlist" dialog (src/dialogs.rs saves it).
                if let Some(a) = w.upgrade() {
                    a.global::<crate::Overlays>().set_new_playlist_open(true);
                }
            });
    }
    render();
    rescan();
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
fn reveal(path: &str) {
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
