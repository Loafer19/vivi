//! System tray icon and background playback (Linux: StatusNotifierItem + dbusmenu over
//! the session bus with zbus, like MPRIS).
//!
//! * Started off the startup path: a second after the window shows, a thread connects to
//!   the bus and registers with `org.kde.StatusNotifierWatcher`. No watcher (no tray
//!   host) means no tray: the close button quits as before.
//! * Left click toggles the window, middle click plays / pauses; the menu has the title,
//!   Play / Pause, Previous, Next, Show / Hide, plugin `tray.item` entries, the
//!   downloads line and Quit. The tooltip shows the title and active downloads.
//! * With the tray up and Settings → General → Close button = "Minimize to tray",
//!   closing hides the window and playback goes on (a video's picture is switched off
//!   while hidden and back on when shown).
//! * While the tray is up Vivi also owns `io.github.loafer19.Vivi`: launching `vivi`
//!   again (with or without files) calls its `Open` method, which shows the window and
//!   opens the files, instead of starting a second player ([`forward`]).
//!
//! Windows / macOS: no tray yet; the close button quits.

use crate::config::Config;
use crate::player::Player;
use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What the tray asks the UI thread to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Toggle,
    PlayPause,
    Prev,
    Next,
    Downloads,
    Quit,
    /// A plugin's `tray.item`: plugin id, command.
    Plugin(String, String),
    /// Another `vivi` was started: show the window and open these files / URLs.
    Open(Vec<String>),
}

/// What the tray shows (copied to the D-Bus thread, compared to emit change signals).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct State {
    pub title: String,
    pub playing: bool,
    pub has_media: bool,
    pub visible: bool,
    /// "2 downloading · 45%" (empty: no active downloads).
    pub downloads: String,
    /// Plugin entries: (plugin id, command, label).
    pub items: Vec<(String, String, String)>,
}

/// Menu entry ids (plugin entries are `PLUGIN + index`).
const TITLE: i32 = 1;
const PLAY: i32 = 3;
const PREV: i32 = 4;
const NEXT: i32 = 5;
const SHOW: i32 = 7;
const DOWNLOADS: i32 = 8;
const QUIT: i32 = 10;
const PLUGIN: i32 = 100;

/// One menu row: id, label ("" = separator), enabled.
pub fn entries(s: &State) -> Vec<(i32, String, bool)> {
    let sep = |id| (id, String::new(), true);
    let mut v = vec![(
        TITLE,
        if s.has_media {
            s.title.clone()
        } else {
            "Nothing playing".into()
        },
        false,
    )];
    v.push(sep(2));
    v.push((
        PLAY,
        if s.playing { "Pause" } else { "Play" }.into(),
        s.has_media,
    ));
    v.push((PREV, "Previous".into(), s.has_media));
    v.push((NEXT, "Next".into(), s.has_media));
    v.push(sep(6));
    v.push((
        SHOW,
        if s.visible {
            "Hide window"
        } else {
            "Show window"
        }
        .into(),
        true,
    ));
    if !s.downloads.is_empty() {
        v.push((DOWNLOADS, format!("Downloads: {}", s.downloads), true));
    }
    for (i, (_, _, label)) in s.items.iter().enumerate() {
        v.push((PLUGIN + i as i32, label.clone(), true));
    }
    v.push(sep(9));
    v.push((QUIT, "Quit Vivi".into(), true));
    v
}

/// The command of menu entry `id`.
pub fn command_of(s: &State, id: i32) -> Option<Cmd> {
    Some(match id {
        PLAY => Cmd::PlayPause,
        PREV => Cmd::Prev,
        NEXT => Cmd::Next,
        SHOW => Cmd::Toggle,
        DOWNLOADS => Cmd::Downloads,
        QUIT => Cmd::Quit,
        i if i >= PLUGIN => {
            let (id, cmd, _) = s.items.get((i - PLUGIN) as usize)?;
            Cmd::Plugin(id.clone(), cmd.clone())
        }
        _ => return None,
    })
}

/// Tooltip text under the "Vivi" heading.
pub fn tooltip(s: &State) -> String {
    let mut t = if s.has_media {
        format!("{} {}", if s.playing { "▶" } else { "⏸" }, s.title)
    } else {
        "Nothing playing".into()
    };
    if !s.downloads.is_empty() {
        t.push_str(&format!("\nDownloads: {}", s.downloads));
    }
    t
}

struct Ui {
    app: slint::Weak<AppWindow>,
    player: Rc<Player>,
    rx: std::sync::mpsc::Receiver<Cmd>,
    #[cfg(target_os = "linux")]
    bus: Option<linux::Bus>,
    last: State,
    /// `vid` to restore when the window comes back (set while hidden with video).
    vid: Option<String>,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static VISIBLE: Cell<bool> = const { Cell::new(true) };
    static DOWNLOADS_LINE: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Whether the tray icon is up (so the close button may hide instead of quit).
pub fn active() -> bool {
    #[cfg(target_os = "linux")]
    {
        UI.with(|u| u.borrow().as_ref().is_some_and(|u| u.bus.is_some()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Downloads summary for the tooltip / menu (from the Downloads panel).
pub fn set_downloads(line: &str) {
    DOWNLOADS_LINE.with(|d| {
        if *d.borrow() != line {
            *d.borrow_mut() = line.to_string();
        }
    });
}

/// Hook the close button and start the tray shortly after startup.
pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    UI.with(|u| {
        *u.borrow_mut() = Some(Ui {
            app: app.as_weak(),
            player: player.clone(),
            rx,
            #[cfg(target_os = "linux")]
            bus: None,
            last: State::default(),
            vid: None,
        })
    });
    let w = app.as_weak();
    app.window().on_close_requested(move || {
        let minimize = cfg.borrow().general.close_button != "Quit";
        if minimize && active() {
            if let Some(a) = w.upgrade() {
                hide(&a);
            }
            return slint::CloseRequestResponse::KeepWindowShown;
        }
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
            || std::env::var_os("VIVI_NO_TRAY").is_some()
        {
            return;
        }
        slint::Timer::single_shot(std::time::Duration::from_millis(1200), move || {
            std::thread::Builder::new()
                .name("vivi-tray".into())
                .spawn(move || {
                    if let Some(bus) = linux::Bus::start(tx) {
                        let _ = slint::invoke_from_event_loop(move || attach(bus));
                    }
                })
                .ok();
        });
    }
    #[cfg(not(target_os = "linux"))]
    drop(tx);
}

#[cfg(target_os = "linux")]
fn attach(bus: linux::Bus) {
    UI.with(|u| {
        if let Some(u) = u.borrow_mut().as_mut() {
            u.bus = Some(bus);
        }
    });
    let t = Box::leak(Box::new(slint::Timer::default()));
    t.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(700),
        update,
    );
    update();
}

/// Copy the player state to the tray and announce changes.
fn update() {
    let state = UI.with(|u| {
        let u = u.borrow();
        let u = u.as_ref()?;
        let a = u.app.upgrade()?;
        let pb = a.global::<crate::Playback>();
        Some(State {
            title: pb.get_media_title().to_string(),
            playing: pb.get_playing(),
            has_media: pb.get_has_media(),
            visible: VISIBLE.with(Cell::get),
            downloads: DOWNLOADS_LINE.with(|d| d.borrow().clone()),
            items: Vec::new(),
        })
    });
    let Some(mut state) = state else { return };
    // Outside UI's borrow: the plugin registry may call back into this module.
    state.items = crate::plugins::contributions(crate::plugins::contrib::TRAY_ITEM)
        .into_iter()
        .map(|e| (e.id, e.command, e.label))
        .collect();
    UI.with(|u| {
        let mut u = u.borrow_mut();
        let Some(u) = u.as_mut() else { return };
        if u.last == state {
            return;
        }
        let tip = tooltip(&state) != tooltip(&u.last);
        u.last = state.clone();
        #[cfg(target_os = "linux")]
        if let Some(b) = &u.bus {
            b.publish(state, tip);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = tip;
    });
}

/// Run what the tray (or another `vivi`) asked for.
pub fn drain() {
    loop {
        let next = UI.with(|u| {
            let u = u.borrow();
            let u = u.as_ref()?;
            Some((u.rx.try_recv().ok()?, u.app.upgrade()?, u.player.clone()))
        });
        let Some((cmd, a, p)) = next else { break };
        match cmd {
            Cmd::Toggle => {
                if VISIBLE.with(Cell::get) {
                    hide(&a)
                } else {
                    show(&a)
                }
            }
            Cmd::PlayPause => p.toggle_pause(),
            Cmd::Prev => p.playlist_step(false),
            Cmd::Next => p.playlist_step(true),
            Cmd::Downloads => {
                show(&a);
                a.global::<crate::Library>()
                    .invoke_navigate("downloads".into());
                a.global::<crate::Overlays>().set_library_open(true);
            }
            Cmd::Quit => {
                let _ = slint::quit_event_loop();
            }
            Cmd::Plugin(id, c) => {
                show(&a);
                crate::plugins::run(&id, &c, &[]);
            }
            Cmd::Open(files) => {
                show(&a);
                let mut it = files.iter();
                if let Some(f) = it.next() {
                    p.open(f);
                }
                for f in it {
                    p.enqueue(f);
                }
            }
        }
    }
    update();
}

/// Hide the window; playback goes on (video picture off while hidden).
pub fn hide(a: &AppWindow) {
    VISIBLE.with(|v| v.set(false));
    UI.with(|u| {
        if let Some(u) = u.borrow_mut().as_mut() {
            let audio = a.global::<crate::Viz>().get_audio();
            let vid = u.player.get("vid").unwrap_or_default();
            if !audio && !vid.is_empty() && vid != "no" && u.vid.is_none() {
                u.vid = Some(vid);
                u.player.set("vid", "no");
            }
        }
    });
    let _ = a.hide();
    update();
}

/// Show the window again (tray click, another `vivi`, MPRIS Raise).
pub fn show(a: &AppWindow) {
    VISIBLE.with(|v| v.set(true));
    let _ = a.show();
    UI.with(|u| {
        if let Some(u) = u.borrow_mut().as_mut() {
            if let Some(vid) = u.vid.take() {
                u.player.set("vid", &vid);
            }
        }
    });
    update();
}

/// Name another `vivi` forwards its files to while the tray is up.
pub const INSTANCE_NAME: &str = "io.github.loafer19.Vivi";
const INSTANCE_PATH: &str = "/io/github/loafer19/Vivi";

/// Second launch: when a Vivi with a tray icon is running, hand it `files` (made
/// absolute) and return true (the caller exits). False otherwise (start normally).
#[cfg(target_os = "linux")]
pub fn forward(files: &[String]) -> bool {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none()
        || std::env::var_os("VIVI_NO_TRAY").is_some()
    {
        return false;
    }
    let Ok(c) = zbus::blocking::Connection::session() else {
        return false;
    };
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&c) else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(INSTANCE_NAME) else {
        return false;
    };
    if !dbus.name_has_owner(name).unwrap_or(false) {
        return false;
    }
    let files: Vec<String> = files
        .iter()
        .map(|f| {
            let p = std::path::Path::new(f);
            if !f.contains("://") && p.exists() {
                std::fs::canonicalize(p).map_or(f.clone(), |p| p.to_string_lossy().into_owned())
            } else {
                f.clone()
            }
        })
        .collect();
    c.call_method(
        Some(INSTANCE_NAME),
        INSTANCE_PATH,
        Some(INSTANCE_NAME),
        "Open",
        &(files,),
    )
    .is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn forward(_files: &[String]) -> bool {
    false
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{command_of, entries, tooltip, Cmd, State, INSTANCE_NAME, INSTANCE_PATH};
    use std::collections::HashMap;
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use zbus::blocking::Connection;
    use zbus::interface;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Type, Value};

    const ITEM_PATH: &str = "/StatusNotifierItem";
    const MENU_PATH: &str = "/MenuBar";

    type Shared = Arc<Mutex<(State, u32)>>;

    fn send(tx: &Mutex<Sender<Cmd>>, c: Cmd) {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(c);
        }
        let _ = slint::invoke_from_event_loop(super::drain);
    }

    type Pixmap = Vec<(i32, i32, Vec<u8>)>;

    /// The app icon as ARGB32 (network byte order) at 32 and 64 px.
    fn pixmaps() -> Pixmap {
        static P: std::sync::OnceLock<Pixmap> = std::sync::OnceLock::new();
        P.get_or_init(|| {
            let Ok(img) = image::load_from_memory(include_bytes!("../packaging/vivi.png")) else {
                return Vec::new();
            };
            [32u32, 64]
                .iter()
                .map(|&n| {
                    let r = image::imageops::resize(
                        &img.to_rgba8(),
                        n,
                        n,
                        image::imageops::FilterType::Triangle,
                    );
                    let mut argb = Vec::with_capacity((n * n * 4) as usize);
                    for p in r.pixels() {
                        argb.extend_from_slice(&[p[3], p[0], p[1], p[2]]);
                    }
                    (n as i32, n as i32, argb)
                })
                .collect()
        })
        .clone()
    }

    struct Item {
        tx: Mutex<Sender<Cmd>>,
        st: Shared,
    }

    #[interface(name = "org.kde.StatusNotifierItem")]
    impl Item {
        #[zbus(property)]
        fn category(&self) -> &str {
            "ApplicationStatus"
        }
        #[zbus(property)]
        fn id(&self) -> &str {
            "vivi"
        }
        #[zbus(property)]
        fn title(&self) -> &str {
            "Vivi"
        }
        #[zbus(property)]
        fn status(&self) -> &str {
            "Active"
        }
        #[zbus(property)]
        fn window_id(&self) -> i32 {
            0
        }
        #[zbus(property)]
        fn icon_name(&self) -> &str {
            "vivi"
        }
        #[zbus(property)]
        fn icon_pixmap(&self) -> Pixmap {
            pixmaps()
        }
        #[zbus(property)]
        fn icon_theme_path(&self) -> &str {
            ""
        }
        #[zbus(property)]
        fn tool_tip(&self) -> (String, Pixmap, String, String) {
            let s = self.st.lock().map(|s| s.0.clone()).unwrap_or_default();
            ("vivi".into(), Vec::new(), "Vivi".into(), tooltip(&s))
        }
        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }
        #[zbus(property)]
        fn menu(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from(MENU_PATH).expect("valid path")
        }
        fn activate(&self, _x: i32, _y: i32) {
            send(&self.tx, Cmd::Toggle);
        }
        fn secondary_activate(&self, _x: i32, _y: i32) {
            send(&self.tx, Cmd::PlayPause);
        }
        fn context_menu(&self, _x: i32, _y: i32) {}
        fn scroll(&self, _delta: i32, _orientation: &str) {}
        #[zbus(signal)]
        async fn new_tool_tip(e: &SignalEmitter<'_>) -> zbus::Result<()>;
    }

    /// dbusmenu layout node: (id, properties, children as variants).
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Type, Value, OwnedValue)]
    pub struct Layout {
        id: i32,
        props: HashMap<String, OwnedValue>,
        children: Vec<OwnedValue>,
    }

    fn props(label: &str, enabled: bool) -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        let mut put = |k: &str, v: Value<'_>| {
            if let Ok(v) = v.try_to_owned() {
                m.insert(k.to_string(), v);
            }
        };
        if label.is_empty() {
            put("type", Value::from("separator"));
        } else {
            put("label", Value::from(label.replace('_', "__")));
            put("enabled", Value::from(enabled));
        }
        m
    }

    fn layout(s: &State) -> Layout {
        let children = entries(s)
            .into_iter()
            .filter_map(|(id, label, enabled)| {
                let l = Layout {
                    id,
                    props: props(&label, enabled),
                    children: Vec::new(),
                };
                OwnedValue::try_from(Value::from(l)).ok()
            })
            .collect();
        let mut root = HashMap::new();
        if let Ok(v) = Value::from("submenu").try_to_owned() {
            root.insert("children-display".to_string(), v);
        }
        Layout {
            id: 0,
            props: root,
            children,
        }
    }

    struct Menu {
        tx: Mutex<Sender<Cmd>>,
        st: Shared,
    }

    impl Menu {
        fn state(&self) -> (State, u32) {
            self.st.lock().map(|s| s.clone()).unwrap_or_default()
        }
        fn click(&self, id: i32) {
            if let Some(c) = command_of(&self.state().0, id) {
                send(&self.tx, c);
            }
        }
    }

    #[interface(name = "com.canonical.dbusmenu")]
    impl Menu {
        fn get_layout(&self, _parent: i32, _depth: i32, _names: Vec<String>) -> (u32, Layout) {
            let (s, rev) = self.state();
            (rev, layout(&s))
        }
        fn get_group_properties(
            &self,
            ids: Vec<i32>,
            _names: Vec<String>,
        ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
            let s = self.state().0;
            entries(&s)
                .into_iter()
                .filter(|(id, _, _)| ids.is_empty() || ids.contains(id))
                .map(|(id, l, e)| (id, props(&l, e)))
                .collect()
        }
        fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
            let s = self.state().0;
            entries(&s)
                .into_iter()
                .find(|(i, _, _)| *i == id)
                .and_then(|(_, l, e)| props(&l, e).remove(&name))
                .ok_or_else(|| zbus::fdo::Error::InvalidArgs(name))
        }
        fn event(&self, id: i32, event_id: String, _data: OwnedValue, _time: u32) {
            if event_id == "clicked" {
                self.click(id);
            }
        }
        fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
            for (id, ev, _, _) in events {
                if ev == "clicked" {
                    self.click(id);
                }
            }
            Vec::new()
        }
        fn about_to_show(&self, _id: i32) -> bool {
            false
        }
        fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
            (Vec::new(), Vec::new())
        }
        #[zbus(property)]
        fn version(&self) -> u32 {
            3
        }
        #[zbus(property)]
        fn text_direction(&self) -> &str {
            "ltr"
        }
        #[zbus(property)]
        fn status(&self) -> &str {
            "normal"
        }
        #[zbus(property)]
        fn icon_theme_path(&self) -> Vec<String> {
            Vec::new()
        }
        #[zbus(signal)]
        async fn layout_updated(
            e: &SignalEmitter<'_>,
            revision: u32,
            parent: i32,
        ) -> zbus::Result<()>;
    }

    /// `vivi` started again while this one has a tray icon.
    struct Instance {
        tx: Mutex<Sender<Cmd>>,
    }

    #[interface(name = "io.github.loafer19.Vivi")]
    impl Instance {
        fn open(&self, files: Vec<String>) {
            send(&self.tx, Cmd::Open(files));
        }
    }

    pub struct Bus {
        conn: Connection,
        st: Shared,
    }

    impl Bus {
        /// Connect and register with the tray host; None without a StatusNotifierWatcher.
        pub fn start(tx: Sender<Cmd>) -> Option<Bus> {
            let st: Shared = Arc::default();
            let conn = zbus::blocking::connection::Builder::session()
                .ok()?
                .serve_at(
                    ITEM_PATH,
                    Item {
                        tx: Mutex::new(tx.clone()),
                        st: st.clone(),
                    },
                )
                .ok()?
                .serve_at(
                    MENU_PATH,
                    Menu {
                        tx: Mutex::new(tx.clone()),
                        st: st.clone(),
                    },
                )
                .ok()?
                .serve_at(INSTANCE_PATH, Instance { tx: Mutex::new(tx) })
                .ok()?
                .build()
                .ok()?;
            let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
            conn.request_name(name.as_str()).ok()?;
            let r = conn.call_method(
                Some("org.kde.StatusNotifierWatcher"),
                "/StatusNotifierWatcher",
                Some("org.kde.StatusNotifierWatcher"),
                "RegisterStatusNotifierItem",
                &(name.as_str(),),
            );
            if let Err(e) = r {
                if std::env::var_os("VIVI_LOG").is_some() {
                    eprintln!("tray: no StatusNotifierWatcher ({e})");
                }
                return None;
            }
            // Only one Vivi takes later launches (the first with a tray icon).
            let _ = conn.request_name(INSTANCE_NAME);
            Some(Bus { conn, st })
        }

        /// New state for the D-Bus side; signals the menu (and tooltip) change.
        pub fn publish(&self, s: State, tip: bool) {
            let rev = {
                let Ok(mut g) = self.st.lock() else { return };
                g.0 = s;
                g.1 += 1;
                g.1
            };
            let os = self.conn.object_server();
            if let Ok(i) = os.interface::<_, Menu>(MENU_PATH) {
                let _ = zbus::block_on(Menu::layout_updated(i.signal_emitter(), rev, 0));
            }
            if tip {
                if let Ok(i) = os.interface::<_, Item>(ITEM_PATH) {
                    let _ = zbus::block_on(Item::new_tool_tip(i.signal_emitter()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_follows_state() {
        let mut s = State {
            title: "Low Tide".into(),
            playing: true,
            has_media: true,
            visible: false,
            downloads: "2 downloading · 45%".into(),
            items: vec![("free-music".into(), "open".into(), "Free Music".into())],
        };
        let e = entries(&s);
        let labels: Vec<&str> = e.iter().map(|(_, l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Low Tide",
                "",
                "Pause",
                "Previous",
                "Next",
                "",
                "Show window",
                "Downloads: 2 downloading · 45%",
                "Free Music",
                "",
                "Quit Vivi"
            ]
        );
        assert_eq!(command_of(&s, PLAY), Some(Cmd::PlayPause));
        assert_eq!(
            command_of(&s, PLUGIN),
            Some(Cmd::Plugin("free-music".into(), "open".into()))
        );
        assert_eq!(command_of(&s, TITLE), None);
        assert!(tooltip(&s).contains("Low Tide") && tooltip(&s).contains("45%"));
        s.has_media = false;
        s.downloads.clear();
        assert_eq!(entries(&s)[0].1, "Nothing playing");
        assert!(!entries(&s).iter().any(|(i, _, _)| *i == DOWNLOADS));
    }
}
