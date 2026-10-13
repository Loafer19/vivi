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
        let mut out = Vec::new();
        let mut crumbs = Vec::new();
        match section {
            "video" => {
                crumbs.push(("video".into(), "Video".into()));
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
                library::sort(&mut files, sort);
                for m in &files {
                    out.push(self.item(m, cfg));
                }
            }
            "music" => {
                crumbs.push(("music:songs".into(), "Music".into()));
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
                library::sort(&mut songs, sort);
                if tab == "albums" && sort == "name" {
                    songs.sort_by_key(|m| m.track);
                }
                for m in &songs {
                    out.push(self.item(m, cfg));
                }
            }
            "playlists" => {
                crumbs.push(("playlists".into(), "Playlists".into()));
                for (name, path) in store::playlists() {
                    let n = std::fs::read_to_string(&path).map_or(0, |s| {
                        s.lines()
                            .filter(|l| !l.starts_with('#') && !l.is_empty())
                            .count()
                    });
                    out.push(Self::group(
                        path.to_string_lossy().into_owned(),
                        &name,
                        format!("{n} items"),
                        "playlist",
                        false,
                    ));
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
                    library::sort(&mut all, sort);
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
                p > 0.0 && p < 0.95
            })
            .cloned()
            .collect();
        // Files outside the library that were played recently also count.
        for p in store::recent() {
            if !cont.iter().any(|m| m.path == p)
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
    MODELS.with(|m| {
        m.borrow_mut()
            .extend([items.clone(), cont.clone(), recent.clone(), pinned.clone()])
    });
    app.set_lib_items(ModelRc::from(items.clone()));
    app.set_lib_continue(ModelRc::from(cont.clone()));
    app.set_lib_recent(ModelRc::from(recent.clone()));
    app.set_lib_pinned(ModelRc::from(pinned.clone()));
    app.set_ytdlp(library::ytdlp_available());

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
    }));

    let render: Rc<dyn Fn()> = {
        let w = app.as_weak();
        let lib = lib.clone();
        let cfg = cfg.clone();
        Rc::new(move || {
            let Some(a) = w.upgrade() else { return };
            let c = cfg.borrow();
            let mut l = lib.borrow_mut();
            let (v, crumbs) = l.build(&c);
            l.items.set_vec(v);
            l.home(&c);
            a.set_lib_nav(l.nav.clone().into());
            a.set_lib_crumb_keys(strings(crumbs.iter().map(|c| c.0.clone())));
            a.set_lib_crumb_labels(strings(crumbs.into_iter().map(|c| c.1)));
            a.set_lib_sort(c.library.sort.clone().into());
            a.set_lib_grid(c.library.view != "list");
            a.set_lib_count(l.media.len() as i32);
            drop(l);
            a.invoke_panel_lib_search("".into());
        })
    };

    // Background scan; re-render when done.
    let rescan: Rc<dyn Fn()> = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        Rc::new(move || {
            let roots = cfg.borrow().library.folders.clone();
            if let Some(a) = w.upgrade() {
                a.set_lib_scanning(true);
            }
            let w = w.clone();
            let db = db.clone();
            std::thread::spawn(move || {
                let t = std::time::Instant::now();
                let res = library::open_db(&db)
                    .and_then(|mut c| library::scan(&mut c, &roots).map(|n| (n, library::all(&c))));
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(a) = w.upgrade() else { return };
                    a.set_lib_scanning(false);
                    match res {
                        Ok((n, media)) => {
                            if log {
                                eprintln!("library scan: {n} files in {:?}", t.elapsed());
                            }
                            LOADED.with(|l| *l.borrow_mut() = Some(media));
                            a.invoke_lib_refresh();
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
        app.on_lib_refresh(move || {
            if let Some(m) = LOADED.with(|l| l.borrow_mut().take()) {
                lib.borrow_mut().media = m;
            }
            render();
        });
    }
    {
        let lib = lib.clone();
        let render = render.clone();
        app.on_lib_navigate(move |key| {
            {
                let mut l = lib.borrow_mut();
                l.nav = key.to_string();
                l.search.clear();
            }
            render();
        });
    }
    {
        let lib = lib.clone();
        let render = render.clone();
        app.on_lib_search(move |q| {
            lib.borrow_mut().search = q.to_string();
            render();
        });
    }
    {
        let cfg = cfg.clone();
        let render = render.clone();
        app.on_lib_set_sort(move |s| {
            cfg.borrow_mut().library.sort = s.to_string();
            let _ = cfg.borrow().save();
            render();
        });
    }
    {
        let cfg = cfg.clone();
        let render = render.clone();
        app.on_lib_set_grid(move |g| {
            cfg.borrow_mut().library.view = if g { "grid" } else { "list" }.into();
            let _ = cfg.borrow().save();
            render();
        });
    }
    {
        let p = player.clone();
        let w = app.as_weak();
        let lib = lib.clone();
        app.on_lib_open(move |key, kind| {
            let Some(a) = w.upgrade() else { return };
            match kind.as_str() {
                "folder" | "group" => a.invoke_lib_navigate(key),
                "playlist" => a.invoke_playlist_load(key),
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
                    a.set_library_open(false);
                }
            }
        });
    }
    {
        let p = player.clone();
        let w = app.as_weak();
        let cfg = cfg.clone();
        let render = render.clone();
        app.on_lib_menu(move |key, kind, action| {
            let Some(a) = w.upgrade() else { return };
            let path = key.strip_prefix("video:").unwrap_or(&key).to_string();
            match action.as_str() {
                "play" => a.invoke_lib_open(key, kind),
                "enqueue" => {
                    if a.get_has_media() {
                        p.enqueue(&path);
                    } else {
                        p.open(&path);
                    }
                    a.set_resume_text(format!("Added “{}” to the queue", name_of(&path)).into());
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
                a.set_network_open(false);
                a.set_library_open(false);
            }
        });
    }
    {
        // Compact library in the side panel.
        let w = app.as_weak();
        let lib = lib.clone();
        app.on_panel_lib_search(move |q| {
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
            a.set_panel_lib(ModelRc::new(VecModel::from(items)));
        });
    }

    {
        let rescan = rescan.clone();
        app.on_lib_rescan(move || rescan());
    }
    render();
    rescan();
}

thread_local! {
    static LOADED: RefCell<Option<Vec<Media>>> = const { RefCell::new(None) };
}
