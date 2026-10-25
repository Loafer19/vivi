//! The Streams hub (#86): one Library section for everything that is not a file.
//!
//! * A URL bar (paste a URL or search; history in `urls.json`), tabs Recent / Favorites /
//!   Radio / TV / Music, the same stream cards everywhere (★ on each) and one shared
//!   now-playing card ([`nowplaying`]).
//! * Radio and TV are the Network Streams built-in (radio-browser + IPTV) in "hub" mode;
//!   when it is off the tabs are not shown. Music lists the music services: plugins that
//!   contribute `streams.music` (or a known music `sidebar.source`).
//! * Plugin dialogs (the `MusicDialogState` protocol) are shown as pages here instead of
//!   modals: inside a hub tab, or full width as a service page (`service:<id>`, opened
//!   from the plugin's sidebar icon) with a back arrow to the hub. [`open_dialog`]
//!   decides; every plugin opens its dialog through it.
//! * Favorites, Recent and the URL history are the core's ([`store`]), migrated once from
//!   Network Streams' `saved.json` / `recent.json` and the global Recent list.
//! * The 0.13 entry points (Open Network Stream, Network Streams, the Radio widget) are
//!   hidden aliases for one release ([`route::alias`]).

pub mod nowplaying;
pub mod route;
pub mod store;

pub use store::Stream;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::player::Player;
use crate::plugins::{self, contrib, network_streams as ns};
use crate::AppWindow;
use route::{Page, Tab};

/// The radio / TV directory built-in (Radio and TV tabs).
pub const DIRECTORY: &str = ns::ID;
/// Music `sidebar.source` plugins that predate the `streams.music` point.
const KNOWN_MUSIC: [&str; 2] = ["free-music", "youtube-music"];
/// Cards on Home's Streams row.
const HOME_CARDS: usize = 12;
/// A plugin asked to open its page is not asked again before this.
const RETRY: Duration = Duration::from_secs(6);

struct Hub {
    app: slint::Weak<AppWindow>,
    player: Option<Rc<Player>>,
    /// Plugin the hub asked to open its dialog (shown as a page when it does).
    pending: Option<String>,
    /// Plugin whose dialog is the page now, and the directory tab it shows.
    owner: String,
    owner_tab: String,
    /// Library nav last rendered.
    nav: String,
    /// Last automatic start of a page owner.
    tried: Option<(String, Instant)>,
    /// URL bar words (filter Recent / Favorites).
    filter: String,
    /// Words for the Radio tab's search when it opens.
    search: Option<String>,
    /// Card logos by logo URL / path, and the downloads in flight.
    logos: HashMap<String, slint::Image>,
    asked: HashSet<String>,
    logo_tx: Sender<(String, Option<PathBuf>)>,
    logo_rx: Receiver<(String, Option<PathBuf>)>,
    dirty: bool,
    np: nowplaying::State,
    /// What the plugin registry said last (it cannot be asked while a plugin's own
    /// command runs, which is when plugins open their dialogs): the directory is on,
    /// the music services, the sidebar sources.
    directory: bool,
    services: Vec<plugins::Contributed>,
    sources: Vec<plugins::Contributed>,
    ticks: u32,
}

impl Default for Hub {
    fn default() -> Self {
        let (logo_tx, logo_rx) = channel();
        Hub {
            app: Default::default(),
            player: None,
            pending: None,
            owner: String::new(),
            owner_tab: String::new(),
            nav: String::new(),
            tried: None,
            filter: String::new(),
            search: None,
            logos: HashMap::new(),
            asked: HashSet::new(),
            logo_tx,
            logo_rx,
            dirty: true,
            np: Default::default(),
            directory: true,
            services: Vec::new(),
            sources: Vec::new(),
            ticks: 0,
        }
    }
}

impl Hub {
    fn np_poll(&mut self) -> u32 {
        self.ticks = self.ticks.wrapping_add(1);
        self.ticks
    }
}

thread_local! {
    static HUB: RefCell<Hub> = RefCell::new(Hub::default());
    /// Card logos decoded by the image cache, drained by the tick.
    static DECODED: RefCell<Vec<(String, slint::Image)>> = const { RefCell::new(Vec::new()) };
}

fn with<R>(f: impl FnOnce(&mut Hub) -> R) -> R {
    HUB.with(|h| f(&mut h.borrow_mut()))
}

/// Run `f` from the event loop, outside whatever callback (often a plugin's own
/// command) is running now.
pub(crate) fn later(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}

/// Rebuild the cards on the next tick (favorites or recent changed).
pub fn refresh_later() {
    let _ = HUB.try_with(|h| {
        if let Ok(mut h) = h.try_borrow_mut() {
            h.dirty = true;
        }
    });
}

/// Ask the plugin registry again (outside plugin commands: from the tick or `later`).
fn learn_plugins() {
    // An empty answer while the registry is busy is not news.
    if plugins::manifests().is_empty() {
        return;
    }
    let directory = plugins::active(DIRECTORY);
    let mut services = plugins::contributions(contrib::STREAMS_MUSIC);
    let sources = plugins::contributions_all(contrib::SIDEBAR_SOURCE);
    for c in &sources {
        if KNOWN_MUSIC.contains(&c.id.as_str()) && !services.iter().any(|o| o.id == c.id) {
            services.push(c.clone());
        }
    }
    let mut seen = HashSet::new();
    services.retain(|c| seen.insert(c.id.clone()));
    with(|h| {
        h.directory = directory;
        h.services = services;
        h.sources = sources;
    });
}

fn directory_on() -> bool {
    with(|h| h.directory)
}

/// The Music tab's services: `streams.music` contributors, then the known music
/// `sidebar.source` plugins.
pub fn services() -> Vec<plugins::Contributed> {
    with(|h| h.services.clone())
}

/// Command that opens plugin `id`'s page (its `streams.music` or `sidebar.source` one).
fn page_command(id: &str) -> String {
    with(|h| {
        h.services
            .iter()
            .chain(h.sources.iter())
            .find(|c| c.id == id && !c.command.is_empty())
            .map(|c| c.command.clone())
            .unwrap_or_default()
    })
}

/// Whether plugin `id`'s dialog is a page (a source: the directory, a sidebar source, a
/// music service) rather than a modal (Auto-tagger review, a package's settings dialog).
fn is_page_service(id: &str) -> bool {
    id == DIRECTORY
        || with(|h| {
            h.services
                .iter()
                .chain(h.sources.iter())
                .any(|c| c.id == id)
        })
}

/// Title of a service page.
pub fn service_name(id: &str) -> String {
    if id == DIRECTORY {
        return "Radio & TV".into();
    }
    with(|h| {
        h.sources
            .iter()
            .chain(h.services.iter())
            .find(|c| c.id == id)
            .map(|c| {
                if c.name.is_empty() {
                    c.label.clone()
                } else {
                    c.name.clone()
                }
            })
    })
    .unwrap_or_else(|| id.to_string())
}

fn strings(v: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.into_iter().map(SharedString::from).collect::<Vec<_>>(),
    ))
}

// ---------- navigation ----------

/// Open a hub place by its `VIVI_PAGE` / IPC syntax or an old entry point id.
pub fn open_page(a: &AppWindow, s: &str) {
    match route::parse(s).or_else(|| route::alias(s)) {
        Some(p) => go(a, p),
        None => eprintln!("vivi: streams: unknown page {s:?}"),
    }
}

/// Show `page` in the Library.
pub fn go(a: &AppWindow, page: Page) {
    let page = page.available(directory_on());
    let ov = a.global::<crate::Overlays>();
    ov.set_network_open(false);
    ov.set_settings_open(false);
    ov.set_extensions_open(false);
    ov.set_library_open(true);
    a.global::<crate::Library>().set_full(true);
    let st = a.global::<crate::Streams>();
    if let Page::Hub {
        tab,
        service,
        focus_url,
    } = &page
    {
        st.set_tab(tab.key().into());
        if let Some(s) = service {
            st.set_service(s.as_str().into());
        }
        if *focus_url {
            st.set_focus_url(st.get_focus_url() + 1);
        }
    }
    a.global::<crate::Library>()
        .invoke_navigate(page.nav().into());
}

/// The plugin dialog the hub page needs for the current nav: (owner, directory tab).
fn wanted(a: &AppWindow, nav: &str) -> Option<(String, String)> {
    if let Some(id) = nav.strip_prefix("service:") {
        return Some((id.to_string(), String::new()));
    }
    if nav != "streams" {
        return None;
    }
    let st = a.global::<crate::Streams>();
    match Tab::parse(&st.get_tab())? {
        Tab::Radio | Tab::Tv if directory_on() => {
            Some((DIRECTORY.into(), st.get_tab().to_string()))
        }
        Tab::Music => {
            let sv = services();
            let cur = st.get_service().to_string();
            let id = sv
                .iter()
                .find(|c| c.id == cur)
                .or(sv.first())
                .map(|c| c.id.clone())?;
            if id != cur {
                st.set_service(id.as_str().into());
            }
            Some((id, String::new()))
        }
        _ => None,
    }
}

/// Close the page dialog (its plugin gets "close").
fn close_page(a: &AppWindow) {
    let owner = with(|h| {
        h.owner_tab.clear();
        std::mem::take(&mut h.owner)
    });
    a.global::<crate::Streams>().set_page_owner("".into());
    let ms = a.global::<crate::MusicDialogState>();
    if !ms.get_page() {
        return;
    }
    ms.set_page(false);
    if a.global::<crate::Overlays>().get_md_open() && ms.get_owner() == owner.as_str() {
        a.global::<crate::Overlays>().set_md_open(false);
    }
    if !owner.is_empty() {
        later(move || plugins::command(&owner, "close", &[]));
    }
}

/// Ask `owner` for its dialog (a page once [`open_dialog`] sees it).
fn start(owner: &str, tab: &str) {
    let search = with(|h| {
        h.pending = Some(owner.to_string());
        h.tried = Some((owner.to_string(), Instant::now()));
        h.search.take()
    });
    let (o, t) = (owner.to_string(), tab.to_string());
    later(move || {
        if o == DIRECTORY {
            let mut args = vec![t];
            args.extend(search);
            plugins::command(DIRECTORY, "hub", &args);
        } else {
            let c = page_command(&o);
            if !c.is_empty() {
                plugins::run(&o, &c, &[]);
            }
        }
    });
}

/// Make the page dialog match what the Library shows: close the one that is not
/// wanted any more, ask for the one that is.
fn reconcile(a: &AppWindow, retry: bool) {
    let nav = with(|h| h.nav.clone());
    let want = wanted(a, &nav);
    let (owner, owner_tab, tried, pending) = with(|h| {
        (
            h.owner.clone(),
            h.owner_tab.clone(),
            h.tried.clone(),
            h.pending.clone(),
        )
    });
    let ms = a.global::<crate::MusicDialogState>();
    let md = a.global::<crate::Overlays>().get_md_open();
    // A modal dialog (Auto-tagger, a package's settings) is up: leave it alone.
    if md && !ms.get_page() {
        return;
    }
    match want {
        None => {
            if !owner.is_empty() {
                close_page(a);
            }
            with(|h| h.pending = None);
        }
        Some((o, tab)) => {
            if !owner.is_empty() && owner != o {
                close_page(a);
            }
            let showing = md && ms.get_page() && ms.get_owner() == o.as_str();
            if showing {
                if o == DIRECTORY && owner_tab != tab {
                    with(|h| h.owner_tab = tab.clone());
                    start(&o, &tab);
                }
                return;
            }
            let recent = tried
                .as_ref()
                .is_some_and(|(t, at)| *t == o && at.elapsed() < RETRY);
            if pending.as_deref() == Some(o.as_str()) && recent {
                return; // asked already, waiting for the dialog
            }
            if !retry && recent {
                return;
            }
            start(&o, &tab);
        }
    }
}

/// The Library rendered `nav` (src/libview.rs): show the hub, a service page or
/// nothing, and get the right plugin dialog.
pub fn nav_changed(a: &AppWindow, nav: &str) {
    let view = if nav == "streams" {
        "hub"
    } else if nav.starts_with("service:") {
        "service"
    } else {
        ""
    };
    let st = a.global::<crate::Streams>();
    st.set_view(view.into());
    if let Some(id) = nav.strip_prefix("service:") {
        st.set_page_title(service_name(id).into());
    }
    let changed = with(|h| {
        let c = h.nav != nav;
        h.nav = nav.to_string();
        h.dirty = true;
        c
    });
    if changed {
        with(|h| h.tried = None);
    }
    let w = a.as_weak();
    later(move || {
        if let Some(a) = w.upgrade() {
            refresh(&a);
            reconcile(&a, true);
        }
    });
}

/// A plugin opens its dialog (instead of `md_open = true`): as a hub page when it is a
/// source the hub asked for or that has a page (palette commands and hotkeys of sources
/// open the page), as a modal otherwise.
pub fn open_dialog(a: &AppWindow) {
    let ms = a.global::<crate::MusicDialogState>();
    let owner = ms.get_owner().to_string();
    let asked = with(|h| {
        let asked = h.pending.as_deref() == Some(owner.as_str());
        if asked {
            h.pending = None;
        }
        asked
    });
    let ov = a.global::<crate::Overlays>();
    if !asked && !is_page_service(&owner) {
        // A modal over everything; a page dialog of another plugin is closed (they share
        // the dialog state) and asked for again when the modal is gone.
        let prev = with(|h| h.owner.clone());
        if !prev.is_empty() && prev != owner {
            with(|h| {
                h.owner.clear();
                h.owner_tab.clear();
                h.tried = None;
            });
            a.global::<crate::Streams>().set_page_owner("".into());
            later(move || plugins::command(&prev, "close", &[]));
        }
        ms.set_page(false);
        ov.set_md_open(true);
        return;
    }
    let prev = with(|h| std::mem::replace(&mut h.owner, owner.clone()));
    if !prev.is_empty() && prev != owner {
        later(move || plugins::command(&prev, "close", &[]));
    }
    ms.set_page(true);
    ov.set_md_open(true);
    a.global::<crate::Streams>()
        .set_page_owner(owner.as_str().into());
    let st = a.global::<crate::Streams>();
    if owner == DIRECTORY {
        let tab = if st.get_tab() == "tv" { "tv" } else { "radio" };
        with(|h| h.owner_tab = tab.into());
    }
    if asked {
        return; // the hub is on the right place already
    }
    // Opened from elsewhere (palette, hotkey, the plugin itself): go to its place.
    let lib_nav = a.global::<crate::Library>().get_nav().to_string();
    let in_music =
        lib_nav == "streams" && st.get_tab() == "music" && st.get_service() == owner.as_str();
    let ov_lib = ov.get_library_open();
    if owner == DIRECTORY {
        if !(ov_lib && lib_nav == "streams" && (st.get_tab() == "radio" || st.get_tab() == "tv")) {
            go(a, Page::tab(Tab::Radio));
        }
    } else if !(ov_lib && (in_music || lib_nav == format!("service:{owner}"))) {
        go(a, Page::Service(owner));
    }
}

/// The plugin closed its own dialog (md_open went false while it was the page).
fn page_gone(a: &AppWindow) {
    let ms = a.global::<crate::MusicDialogState>();
    let md = a.global::<crate::Overlays>().get_md_open();
    let owner = with(|h| h.owner.clone());
    if !owner.is_empty() && (!md || ms.get_owner() != owner.as_str()) {
        with(|h| {
            h.owner.clear();
            h.owner_tab.clear();
        });
        if !md {
            ms.set_page(false);
        }
        a.global::<crate::Streams>().set_page_owner("".into());
    }
}

// ---------- cards ----------

fn logo_of(h: &mut Hub, logo: &str) -> Option<slint::Image> {
    if logo.is_empty() {
        return None;
    }
    if let Some(i) = h.logos.get(logo) {
        return Some(i.clone());
    }
    let key = logo.to_string();
    let decode = |p: &Path, key: String| {
        crate::images::get(p, crate::images::TILE, move |i| {
            DECODED.with(|d| d.borrow_mut().push((key, i)))
        })
    };
    if logo.starts_with("http") {
        if let Some(p) = ns::cached_logo(logo) {
            let i = decode(&p, key.clone());
            if let Some(i) = &i {
                h.logos.insert(key, i.clone());
            }
            return i;
        }
        if h.asked.insert(key.clone()) {
            let tx = h.logo_tx.clone();
            std::thread::spawn(move || {
                let p = ns::fetch_logo(&key);
                let _ = tx.send((key, p));
            });
        }
        return None;
    }
    let p = Path::new(logo.strip_prefix("file://").unwrap_or(logo));
    if p.is_file() {
        let i = decode(p, key.clone());
        if let Some(i) = &i {
            h.logos.insert(key, i.clone());
        }
        return i;
    }
    None
}

fn card(h: &mut Hub, s: &Stream, favs: &HashSet<String>, playing: &str) -> crate::StreamCard {
    let kind = store::kind_of(s);
    let (badge, icon) = match kind {
        "radio" => ("Radio", "streaming"),
        "tv" => ("TV", "tv"),
        "music" => ("Music", "music"),
        "web" => ("Web", "youtube"),
        _ => ("", "link"),
    };
    let title = ns::display_name(&s.name);
    let host = crate::store::url_host(&s.url).unwrap_or_default();
    let title = if title.is_empty() || title == s.url {
        host.clone()
    } else {
        title
    };
    let mut sub: Vec<String> = Vec::new();
    if !s.group.is_empty() {
        sub.push(
            s.group
                .split([',', ';'])
                .take(2)
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if !s.country.is_empty() {
        sub.push(s.country.clone());
    }
    if sub.is_empty() {
        sub.push(host);
    }
    let logo = logo_of(h, &s.logo);
    crate::StreamCard {
        url: s.url.as_str().into(),
        title: title.into(),
        sub: sub.join(" · ").into(),
        badge: badge.into(),
        has_logo: logo.is_some(),
        logo: logo.unwrap_or_default(),
        fav: favs.contains(&s.url),
        playing: !playing.is_empty() && s.url == playing,
        icon: icon.into(),
    }
}

fn matches(s: &Stream, q: &str) -> bool {
    q.is_empty()
        || [&s.name, &s.url, &s.group, &s.country]
            .iter()
            .any(|f| f.to_lowercase().contains(q))
}

/// Fill the hub's lists, Home's Streams row, the services and the URL history.
fn refresh(a: &AppWindow) {
    learn_plugins();
    let favs = store::favorites();
    let recent = store::recent();
    let fav_urls: HashSet<String> = favs.iter().map(|s| s.url.clone()).collect();
    let playing = a.global::<crate::NowPlaying>().get_url().to_string();
    let st = a.global::<crate::Streams>();
    let directory = directory_on();
    st.set_directory(directory);
    if !directory && (st.get_tab() == "radio" || st.get_tab() == "tv") {
        st.set_tab("recent".into());
    }
    let (fc, rc, home) = with(|h| {
        h.dirty = false;
        let q = h.filter.to_lowercase();
        let fc: Vec<_> = favs
            .iter()
            .filter(|s| matches(s, &q))
            .map(|s| card(h, s, &fav_urls, &playing))
            .collect();
        let rc: Vec<_> = recent
            .iter()
            .filter(|s| matches(s, &q))
            .map(|s| card(h, s, &fav_urls, &playing))
            .collect();
        // Home: favorites first, then recent streams that are not favorites.
        let home: Vec<_> = favs
            .iter()
            .chain(recent.iter().filter(|s| !fav_urls.contains(&s.url)))
            .take(HOME_CARDS)
            .map(|s| card(h, s, &fav_urls, &playing))
            .collect();
        (fc, rc, home)
    });
    st.set_favorites(ModelRc::new(VecModel::from(fc)));
    st.set_recent(ModelRc::new(VecModel::from(rc)));
    st.set_home(ModelRc::new(VecModel::from(home)));
    st.set_history(strings(store::urls()));
    let sv: Vec<crate::StreamService> = services()
        .into_iter()
        .map(|c| crate::StreamService {
            name: if c.name.is_empty() {
                c.label.into()
            } else {
                c.name.into()
            },
            icon: c.icon.into(),
            id: c.id.into(),
        })
        .collect();
    st.set_services(ModelRc::new(VecModel::from(sv)));
    if !playing.is_empty() {
        a.global::<crate::NowPlaying>()
            .set_fav(fav_urls.contains(&playing));
    }
}

fn find(url: &str) -> Stream {
    store::favorites()
        .into_iter()
        .chain(store::recent())
        .find(|s| s.url == url)
        .unwrap_or_else(|| Stream {
            url: url.to_string(),
            ..Default::default()
        })
}

/// Play `s` without leaving the hub.
fn play(s: &Stream) {
    let Some(p) = with(|h| h.player.clone()) else {
        return;
    };
    let name = ns::display_name(&s.name);
    if !name.is_empty() {
        crate::loading::set_name(&s.url, &name);
    }
    crate::store::recent_hint(crate::store::RecentEntry {
        target: s.url.clone(),
        kind: if store::kind_of(s) == "web" {
            crate::store::RecentKind::Web
        } else {
            crate::store::RecentKind::Stream
        },
        title: name,
        sub: s.group.clone(),
        image: s.logo.clone(),
    });
    store::push_recent(s);
    p.open(&s.url);
    refresh_later();
}

// ---------- setup ----------

pub fn setup(app: &AppWindow, player: &Rc<Player>) {
    with(|h| {
        h.app = app.as_weak();
        h.player = Some(player.clone());
    });
    let st = app.global::<crate::Streams>();
    let w = app.as_weak();
    st.on_open_page(move |s| {
        if let Some(a) = w.upgrade() {
            open_page(&a, &s);
        }
    });
    let w = app.as_weak();
    st.on_select_tab(move |t| {
        let Some(a) = w.upgrade() else { return };
        let Some(tab) = Tab::parse(&t) else { return };
        go(&a, Page::tab(tab));
    });
    let w = app.as_weak();
    st.on_pick_service(move |id| {
        let Some(a) = w.upgrade() else { return };
        go(
            &a,
            Page::Hub {
                tab: Tab::Music,
                service: Some(id.to_string()),
                focus_url: false,
            },
        );
    });
    let w = app.as_weak();
    st.on_edited(move |t| {
        let t = t.trim().to_string();
        let q = if store::looks_like_url(&t) {
            String::new()
        } else {
            t
        };
        with(|h| h.filter = q);
        if let Some(a) = w.upgrade() {
            refresh(&a);
        }
    });
    let w = app.as_weak();
    st.on_submit(move |t| {
        let Some(a) = w.upgrade() else { return };
        let t = t.trim().to_string();
        if t.is_empty() {
            return;
        }
        let st = a.global::<crate::Streams>();
        if store::looks_like_url(&t) {
            let url = store::normalize_url(&t);
            store::push_url(&url);
            st.set_url("".into());
            with(|h| h.filter.clear());
            play(&find(&url));
            refresh(&a);
        } else if directory_on() {
            // Words: search the radio directory (the list filter stays meanwhile).
            with(|h| h.search = Some(t.clone()));
            if st.get_tab() == "radio" && with(|h| h.owner == DIRECTORY) {
                let q = with(|h| h.search.take()).unwrap_or_default();
                later(move || plugins::command(DIRECTORY, "hub", &["radio".into(), q]));
            } else {
                go(&a, Page::tab(Tab::Radio));
            }
        }
    });
    let w = app.as_weak();
    st.on_clear_history(move || {
        store::clear_urls();
        if let Some(a) = w.upgrade() {
            refresh(&a);
        }
    });
    st.on_play(|url| play(&find(&url)));
    let w = app.as_weak();
    st.on_toggle_fav(move |url| {
        store::toggle_favorite(&find(&url));
        if let Some(a) = w.upgrade() {
            refresh(&a);
        }
        // The directory's own stars follow.
        later(|| plugins::command(DIRECTORY, "favorites-changed", &[]));
    });
    let w = app.as_weak();
    st.on_remove(move |url| {
        store::remove_recent(&url);
        if let Some(a) = w.upgrade() {
            refresh(&a);
        }
    });
    let w = app.as_weak();
    st.on_clear_recent(move || {
        store::save_recent(&[]);
        if let Some(a) = w.upgrade() {
            refresh(&a);
        }
    });
    let w = app.as_weak();
    st.on_more_options(move || {
        if let Some(a) = w.upgrade() {
            let url = a.global::<crate::Streams>().get_url();
            if !url.is_empty() {
                a.global::<crate::Dialogs>().set_net_urls(url);
            }
            a.global::<crate::Overlays>().set_network_open(true);
        }
    });
    let w = app.as_weak();
    st.on_back(move || {
        if let Some(a) = w.upgrade() {
            go(&a, Page::tab(Tab::Music));
        }
    });
    let w = app.as_weak();
    app.global::<crate::NowPlaying>().on_action(move |what| {
        let Some(a) = w.upgrade() else { return };
        let mut np = with(|h| std::mem::take(&mut h.np));
        nowplaying::action(&mut np, &a, &what);
        with(|h| h.np = np);
    });
    learn_plugins();
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), tick);
}

fn tick() {
    let Some((a, p)) = with(|h| Some((h.app.upgrade()?, h.player.clone()?))) else {
        return;
    };
    if with(|h| h.np_poll()) % 8 == 0 {
        learn_plugins();
    }
    let mut np = with(|h| std::mem::take(&mut h.np));
    nowplaying::tick(&mut np, &a, &p);
    with(|h| h.np = np);
    // Logos that arrived.
    let mut got = false;
    while let Some((key, path)) = with(|h| h.logo_rx.try_recv().ok()) {
        if let Some(path) = path {
            let k = key.clone();
            if let Some(i) = crate::images::get(&path, crate::images::TILE, move |i| {
                DECODED.with(|d| d.borrow_mut().push((k, i)))
            }) {
                with(|h| h.logos.insert(key, i));
                got = true;
            }
        }
    }
    for (k, i) in DECODED.with(|d| std::mem::take(&mut *d.borrow_mut())) {
        with(|h| h.logos.insert(k, i));
        got = true;
    }
    if got {
        with(|h| h.dirty = true);
    }
    // The page's plugin closed its dialog, or the Library was closed.
    page_gone(&a);
    let lib_open = a.global::<crate::Overlays>().get_library_open();
    let nav = with(|h| h.nav.clone());
    if !lib_open && with(|h| !h.owner.is_empty()) {
        close_page(&a);
    }
    if lib_open && (nav == "streams" || nav.starts_with("service:")) {
        reconcile(&a, false);
    }
    let dirty = with(|h| h.dirty);
    if dirty {
        refresh(&a);
    }
}
