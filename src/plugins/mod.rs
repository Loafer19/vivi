//! In-process plugin system.
//!
//! A plugin is a type implementing [`Plugin`], compiled into the binary and listed in
//! [`builtin`]. Each one describes itself with a [`Manifest`] (id, name, description,
//! version, author, icon and a list of [`SettingSpec`]s). The [`Registry`] owns all
//! plugins, keeps their enabled state and settings in `config.toml` (`[plugins.<id>]`),
//! and forwards player events to the enabled ones:
//!
//! * [`Plugin::enable`] / [`Plugin::disable`] when toggled (and `enable` at startup),
//! * [`Plugin::file_loaded`] after mpv loaded a new file,
//! * [`Plugin::tick`] every [`TICK`] while enabled,
//! * [`Plugin::setting_changed`] and [`Plugin::command`] for settings and UI callbacks.
//!
//! mpv Lua/JS user scripts in `~/.config/vivi/scripts` show up as *external plugins*
//! (see [`scripts`]); enabled ones are loaded with mpv's `load-script` command.
//! Downloadable *packages* (see [`packages`]) are mpv scripts too, installed from the
//! catalog into `~/.local/share/vivi/plugins`; their manifest settings are passed to the
//! script as mpv `script-opts` (`<id>-<key>=<value>`).
//!
//! The Settings → Extensions page is generated from the manifests (see `settings.rs`).

pub mod free_music;
pub mod lyrics;
pub mod packages;
pub mod scripts;
pub mod skip_intro;
pub mod streams;
pub mod subtitles;
pub mod tagger;
pub mod web;

use crate::config::Config;
use crate::player::Player;
use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// How often enabled plugins get [`Plugin::tick`].
pub const TICK: Duration = Duration::from_millis(100);
/// User-Agent for plugin HTTP requests.
pub const USER_AGENT: &str = concat!("vivi/", env!("CARGO_PKG_VERSION"));

/// Kind of a plugin setting; maps to a Settings row kind.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    Toggle,
    Choice(Vec<String>),
    /// Free text; the string is the placeholder.
    Text(String),
    /// Hidden text (API keys, passwords).
    Secret,
    /// A button; the string is its label. Runs [`Plugin::command`] with the key.
    Action(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    pub key: String,
    pub title: String,
    pub info: String,
    pub kind: SettingKind,
    pub default: String,
}

impl SettingSpec {
    fn new(key: &str, title: &str, info: &str, kind: SettingKind, default: &str) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            info: info.into(),
            kind,
            default: default.into(),
        }
    }
    pub fn toggle(key: &str, title: &str, info: &str, default: bool) -> Self {
        Self::new(key, title, info, SettingKind::Toggle, &default.to_string())
    }
    pub fn choice(key: &str, title: &str, info: &str, options: &[&str]) -> Self {
        let o = options.iter().map(|s| s.to_string()).collect();
        Self::new(key, title, info, SettingKind::Choice(o), options[0])
    }
    pub fn text(key: &str, title: &str, info: &str, placeholder: &str, default: &str) -> Self {
        Self::new(
            key,
            title,
            info,
            SettingKind::Text(placeholder.into()),
            default,
        )
    }
    pub fn secret(key: &str, title: &str, info: &str) -> Self {
        Self::new(key, title, info, SettingKind::Secret, "")
    }
    pub fn action(key: &str, title: &str, info: &str, label: &str) -> Self {
        Self::new(key, title, info, SettingKind::Action(label.into()), "")
    }
}

/// Where a plugin comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    /// An mpv user script (Lua or JavaScript).
    Script,
    /// A downloaded plugin package (an mpv script with a manifest).
    Package,
}

/// Static description of a plugin (also the format of `plugins/index.json` entries).
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Stable identifier, lowercase with dashes (`skip-intro`). Config key.
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    /// Icon key mapped to an SVG in `ui/settings.slint` (`PluginIcons`).
    pub icon: String,
    pub settings: Vec<SettingSpec>,
    pub default_enabled: bool,
    pub origin: Origin,
}

impl Manifest {
    /// A built-in manifest with the crate version and author.
    pub fn builtin(id: &str, name: &str, description: &str, icon: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            author: "Vivi".into(),
            icon: icon.into(),
            settings: Vec::new(),
            default_enabled: true,
            origin: Origin::Builtin,
        }
    }
    pub fn setting(mut self, s: SettingSpec) -> Self {
        self.settings.push(s);
        self
    }
    /// Start disabled until the user turns it on.
    #[cfg(test)]
    pub fn default_off(mut self) -> Self {
        self.default_enabled = false;
        self
    }
}

/// What a plugin sees on every call: the window, the player and its own settings.
pub struct Ctx<'a> {
    pub app: &'a AppWindow,
    pub player: &'a Rc<Player>,
    id: &'a str,
    manifest: &'a Manifest,
    cfg: &'a Rc<RefCell<Config>>,
}

impl Ctx<'_> {
    /// A setting value (or the manifest default).
    pub fn get(&self, key: &str) -> String {
        setting_value(&self.cfg.borrow(), self.manifest, key)
    }
    pub fn flag(&self, key: &str) -> bool {
        self.get(key) == "true"
    }
    /// Change a setting and save the config.
    pub fn set(&self, key: &str, value: &str) {
        let mut c = self.cfg.borrow_mut();
        c.plugins
            .entry(self.id.into())
            .or_default()
            .settings
            .insert(key.into(), value.into());
        if let Err(e) = c.save() {
            eprintln!("vivi: save config: {e}");
        }
    }
    /// Show a short message in the player toast.
    pub fn toast(&self, msg: &str) {
        show_toast(self.app, msg);
    }
    /// Local path of the playing file, if it is a file.
    pub fn local_path(&self) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(self.player.get("path")?);
        p.is_file().then_some(p)
    }
}

/// Value of a plugin setting from the config, falling back to the manifest default.
pub fn setting_value(c: &Config, m: &Manifest, key: &str) -> String {
    c.plugins
        .get(&m.id)
        .and_then(|s| s.settings.get(key).cloned())
        .or_else(|| {
            m.settings
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.default.clone())
        })
        .unwrap_or_default()
}

/// Whether a plugin is enabled in the config.
pub fn is_enabled(c: &Config, m: &Manifest) -> bool {
    c.plugins
        .get(&m.id)
        .and_then(|s| s.enabled)
        .unwrap_or(m.default_enabled)
}

/// A compiled-in plugin.
pub trait Plugin {
    fn manifest(&self) -> Manifest;
    fn enable(&mut self, _cx: &Ctx) {}
    fn disable(&mut self, _cx: &Ctx) {}
    fn file_loaded(&mut self, _cx: &Ctx) {}
    fn tick(&mut self, _cx: &Ctx) {}
    fn setting_changed(&mut self, _cx: &Ctx, _key: &str) {}
    /// A UI callback or an [`SettingKind::Action`] button addressed to this plugin.
    fn command(&mut self, _cx: &Ctx, _name: &str, _args: &[String]) {}
}

/// All compiled-in plugins, in Settings order.
pub fn builtin() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(crate::viz::VizPlugin),
        Box::new(skip_intro::SkipIntro::default()),
        Box::new(lyrics::Lyrics::default()),
        Box::new(subtitles::SubtitleSearch::default()),
        Box::new(tagger::AutoTagger::default()),
        Box::new(free_music::FreeMusic::default()),
        Box::new(streams::Streams::default()),
    ]
}

struct Entry {
    plugin: Box<dyn Plugin>,
    manifest: Manifest,
    active: bool,
}

pub struct Registry {
    app: slint::Weak<AppWindow>,
    player: Rc<Player>,
    cfg: Rc<RefCell<Config>>,
    entries: Vec<Entry>,
    scripts: Vec<scripts::Script>,
    /// Ids of scripts mpv has loaded (it cannot unload them).
    loaded: std::collections::HashSet<String>,
    /// Load status per script id for the Extensions page ("Loaded", "Failed: …").
    status: std::collections::HashMap<String, String>,
}

thread_local! {
    static REG: RefCell<Option<Registry>> = const { RefCell::new(None) };
}

/// Run `f` with the registry (UI thread only). Not re-entrant.
fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> Option<R> {
    REG.with(|r| r.try_borrow_mut().ok()?.as_mut().map(f))
}

macro_rules! ctx {
    ($reg:expr, $a:expr, $e:expr) => {
        Ctx {
            app: &$a,
            player: &$reg.player,
            id: &$e.manifest.id,
            manifest: &$e.manifest,
            cfg: &$reg.cfg,
        }
    };
}

/// Manifests of all plugins (built-in first, then scripts), for the Settings page.
pub fn manifests() -> Vec<Manifest> {
    with(|r| {
        r.entries
            .iter()
            .map(|e| e.manifest.clone())
            .chain(r.scripts.iter().map(|s| s.manifest.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// Create the registry, enable plugins per config and start the tick timer.
pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>) {
    let entries: Vec<Entry> = builtin()
        .into_iter()
        .map(|p| Entry {
            manifest: p.manifest(),
            plugin: p,
            active: false,
        })
        .collect();
    let scripts = all_scripts(&entries);
    REG.with(|r| {
        *r.borrow_mut() = Some(Registry {
            app: app.as_weak(),
            player: player.clone(),
            cfg,
            entries,
            scripts,
            loaded: Default::default(),
            status: Default::default(),
        })
    });
    with(|r| {
        let c = r.cfg.borrow().clone();
        for e in &mut r.entries {
            e.active = is_enabled(&c, &e.manifest);
            if e.active {
                e.plugin.enable(&ctx!(r, *app, e));
            } else {
                e.plugin.disable(&ctx!(r, *app, e));
            }
        }
        let startup = scripts::load_on_startup(&c);
        let list = r.scripts.clone();
        for s in &list {
            let on = is_enabled(&c, &s.manifest);
            if startup && on {
                r.load_script(s);
            } else {
                let why = if on {
                    "Not loaded: \"Load add-ons on startup\" is off"
                } else {
                    "Not loaded: disabled"
                };
                eprintln!("vivi: script {}: {why}", s.manifest.id);
                r.status.insert(s.manifest.id.clone(), why.into());
            }
        }
    });
    // UI callbacks addressed to plugins.
    app.on_plugin_command(|id, name, args| {
        let args: Vec<String> = args.split('\u{1f}').map(String::from).collect();
        command(&id, &name, &args);
    });
    let auto = with(|r| {
        r.cfg
            .borrow()
            .plugins
            .get(packages::GLOBAL_ID)
            .and_then(|s| s.settings.get("autocheck"))
            .is_none_or(|v| v == "true")
    })
    .unwrap_or(true);
    packages::setup(app, auto);
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(slint::TimerMode::Repeated, TICK, || {
        with(|r| {
            let Some(a) = r.app.upgrade() else { return };
            for e in r.entries.iter_mut().filter(|e| e.active) {
                e.plugin.tick(&ctx!(r, a, e));
            }
        });
    });
}

use crate::config;

/// Re-read the scripts folder; enabled new scripts are loaded. Returns the count.
pub fn rescan() -> usize {
    with(|r| {
        let new = all_scripts(&r.entries);
        let c = r.cfg.borrow().clone();
        for s in &new {
            let known = r.scripts.iter().any(|o| o.path == s.path);
            if !known && scripts::load_on_startup(&c) && is_enabled(&c, &s.manifest) {
                r.load_script(s);
            }
        }
        r.scripts = new;
        r.scripts
            .iter()
            .filter(|s| s.manifest.origin == Origin::Script)
            .count()
    })
    .unwrap_or(0)
}

/// Rescan after installing package `id`; whether it is now loaded in mpv.
pub fn rescan_package(id: &str) -> bool {
    rescan();
    with(|r| r.loaded.contains(id)).unwrap_or(false)
}

/// mpv scripts from the scripts folder plus installed packages (ids that clash with a
/// built-in plugin are skipped).
fn all_scripts(entries: &[Entry]) -> Vec<scripts::Script> {
    let mut v = scripts::scan(&config::scripts_dir());
    for p in packages::installed(&packages::root()) {
        if entries.iter().any(|e| e.manifest.id == p.manifest.id) {
            eprintln!(
                "vivi: package {} clashes with a built-in plugin",
                p.manifest.id
            );
            continue;
        }
        v.push(scripts::Script {
            path: p.entry_path(),
            manifest: p.manifest.to_manifest(),
        });
    }
    v
}

/// mpv `script-opts` value for a package setting (mp.options wants yes/no booleans).
fn script_opt(id: &str, key: &str, value: &str) -> String {
    let v = match value {
        "true" => "yes",
        "false" => "no",
        v => v,
    };
    format!("{id}-{key}={}", v.replace(',', " "))
}

impl Registry {
    /// Load an mpv script once (packages get their settings as script-opts first).
    fn load_script(&mut self, s: &scripts::Script) {
        if !self.loaded.insert(s.manifest.id.clone()) {
            return;
        }
        if s.manifest.origin == Origin::Package {
            let c = self.cfg.borrow();
            for st in &s.manifest.settings {
                if matches!(st.kind, SettingKind::Action(_)) {
                    continue;
                }
                let v = setting_value(&c, &s.manifest, &st.key);
                self.player.cmd(
                    "change-list",
                    &[
                        "script-opts",
                        "append",
                        &script_opt(&s.manifest.id, &st.key, &v),
                    ],
                );
            }
            // Where the package lives (helpers are in `<pkgdir>/bin`).
            if let Some(dir) = s.path.parent() {
                self.player.cmd(
                    "change-list",
                    &[
                        "script-opts",
                        "append",
                        &format!("{}-pkgdir={}", s.manifest.id, dir.display()),
                    ],
                );
            }
        }
        let path = s.path.to_string_lossy();
        let st = match self.player.try_cmd("load-script", &[&path]) {
            Ok(()) => {
                eprintln!("vivi: loaded script {} ({path})", s.manifest.id);
                "Loaded".to_string()
            }
            Err(e) => {
                eprintln!("vivi: load-script {path} failed: {e}");
                format!("Failed to load: {e}")
            }
        };
        self.status.insert(s.manifest.id.clone(), st);
    }
}

/// mpv client name of a script (its file name without extension).
fn client_name(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Load status of a script or package for the Extensions page.
pub fn script_status(id: &str) -> Option<String> {
    with(|r| r.status.get(id).cloned()).flatten()
}

/// Toast text of a `script-message vivi-toast <text> [seconds]` from a plugin.
pub fn toast_text(args: &[String]) -> Option<&str> {
    match args {
        [m, text, ..] if m == "vivi-toast" && !text.trim().is_empty() => Some(text.trim()),
        _ => None,
    }
}

/// A `script-message` broadcast reached Vivi's mpv client.
pub fn client_message(args: &[String]) {
    match args {
        [m, id, key, value] if m == "vivi-setting" => return store_setting(id, key, value),
        [m, id, key] if m == "vivi-pick-folder" => return pick_folder(id, key),
        _ => {}
    }
    let Some(text) = toast_text(args) else { return };
    with(|r| {
        if let Some(a) = r.app.upgrade() {
            show_toast(&a, text);
        }
    });
}

/// `script-message vivi-setting <id> <key> <value>`: a package saves one of its own
/// declared (non-action) settings, e.g. a generated PIN. Applied as script-opt too.
fn store_setting(id: &str, key: &str, value: &str) {
    with(|r| {
        let ok = r.scripts.iter().any(|s| {
            s.manifest.id == id
                && s.manifest.origin == Origin::Package
                && s.manifest
                    .settings
                    .iter()
                    .any(|st| st.key == key && !matches!(st.kind, SettingKind::Action(_)))
        });
        if !ok {
            eprintln!("vivi: vivi-setting for unknown setting {id}.{key} ignored");
            return;
        }
        r.cfg
            .borrow_mut()
            .plugins
            .entry(id.into())
            .or_default()
            .settings
            .insert(key.into(), value.into());
        save(&r.cfg);
        r.player.cmd(
            "change-list",
            &["script-opts", "append", &script_opt(id, key, value)],
        );
    });
    crate::settings::refresh_ui();
}

/// `script-message vivi-pick-folder <id> <key>`: folder dialog, result stored like
/// [`store_setting`] and announced back with `script-message-to <script> folder-picked <path>`.
fn pick_folder(id: &str, key: &str) {
    let (id, key) = (id.to_string(), key.to_string());
    let Some(name) = with(|r| {
        r.scripts
            .iter()
            .find(|s| s.manifest.id == id && s.manifest.origin == Origin::Package)
            .and_then(|s| s.path.file_stem())
            .map(|n| n.to_string_lossy().replace('-', "_"))
    })
    .flatten() else {
        return;
    };
    // Same as File → Open folder: the dialog runs on the UI thread (GTK needs it).
    let _ = slint::invoke_from_event_loop(move || {
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Choose folder")
            .pick_folder()
        else {
            return;
        };
        let d = dir.to_string_lossy().into_owned();
        store_setting(&id, &key, &d);
        with(|r| {
            r.player
                .cmd("script-message-to", &[&name, "folder-picked", &d])
        });
    });
}

/// An mpv warning/error: a script error marks that script as failed.
pub fn mpv_log(prefix: &str, text: &str) {
    with(|r| {
        let Some(s) = r
            .scripts
            .iter()
            .find(|s| r.loaded.contains(&s.manifest.id) && client_name(&s.path) == prefix)
        else {
            return;
        };
        eprintln!("vivi: script {}: {text}", s.manifest.id);
        if text.to_ascii_lowercase().contains("error") {
            r.status
                .insert(s.manifest.id.clone(), format!("Error: {text}"));
        }
    });
}

fn show_toast(app: &AppWindow, msg: &str) {
    app.set_resume_text(msg.into());
    let w = app.as_weak();
    let m = msg.to_string();
    slint::Timer::single_shot(Duration::from_secs(4), move || {
        if let Some(a) = w.upgrade() {
            if a.get_resume_text().as_str() == m {
                a.set_resume_text("".into());
            }
        }
    });
}

/// mpv loaded a new file.
pub fn file_loaded() {
    with(|r| {
        let Some(a) = r.app.upgrade() else { return };
        for e in r.entries.iter_mut().filter(|e| e.active) {
            e.plugin.file_loaded(&ctx!(r, a, e));
        }
    });
}

/// Send a command to one plugin (no-op when it is disabled).
pub fn command(id: &str, name: &str, args: &[String]) {
    with(|r| {
        let Some(a) = r.app.upgrade() else { return };
        if let Some(e) = r
            .entries
            .iter_mut()
            .find(|e| e.active && e.manifest.id == id)
        {
            e.plugin.command(&ctx!(r, a, e), name, args);
        }
    });
}

/// Settings row id of a plugin's enable toggle / setting.
pub fn toggle_id(id: &str) -> String {
    format!("plugin.{id}")
}
pub fn setting_id(id: &str, key: &str) -> String {
    format!("plugin.{id}.{key}")
}

/// Handle a `plugin.*` settings change. Returns a status message, if any.
pub fn setting_changed(row_id: &str, value: &str) -> Option<String> {
    let rest = row_id.strip_prefix("plugin.")?;
    let (id, key) = match rest.split_once('.') {
        Some((i, k)) => (i, Some(k)),
        None => (rest, None),
    };
    with(|r| {
        let a = r.app.upgrade()?;
        // External scripts and packages.
        if let Some(s) = r.scripts.iter().find(|s| s.manifest.id == id).cloned() {
            if let Some(key) = key {
                let spec = s.manifest.settings.iter().find(|st| st.key == key)?;
                // mpv script name = entry file stem with dashes as underscores.
                let name = s.path.file_stem()?.to_string_lossy().replace('-', "_");
                if matches!(spec.kind, SettingKind::Action(_)) {
                    if !r.loaded.contains(id) {
                        return Some(format!("{} is not running", s.manifest.name));
                    }
                    r.player.cmd("script-message-to", &[&name, key]);
                    return None;
                }
                r.cfg
                    .borrow_mut()
                    .plugins
                    .entry(id.into())
                    .or_default()
                    .settings
                    .insert(key.into(), value.into());
                save(&r.cfg);
                r.player.cmd(
                    "change-list",
                    &["script-opts", "append", &script_opt(id, key, value)],
                );
                return None;
            }
            let on = value == "true";
            r.cfg
                .borrow_mut()
                .plugins
                .entry(id.into())
                .or_default()
                .enabled = Some(on);
            save(&r.cfg);
            return Some(if on {
                if r.loaded.contains(id) {
                    format!("{} is already running", s.manifest.name)
                } else {
                    r.load_script(&s);
                    format!("Loaded {}", s.manifest.name)
                }
            } else {
                "mpv cannot unload scripts: restart Vivi to stop it".into()
            });
        }
        let e = r.entries.iter_mut().find(|e| e.manifest.id == id)?;
        match key {
            None => {
                let on = value == "true";
                r.cfg
                    .borrow_mut()
                    .plugins
                    .entry(id.into())
                    .or_default()
                    .enabled = Some(on);
                save(&r.cfg);
                if on != e.active {
                    e.active = on;
                    let cx = ctx!(r, a, e);
                    if on {
                        e.plugin.enable(&cx);
                        e.plugin.file_loaded(&cx);
                    } else {
                        e.plugin.disable(&cx);
                    }
                }
                None
            }
            Some(key) => {
                let spec = e.manifest.settings.iter().find(|s| s.key == key)?;
                if matches!(spec.kind, SettingKind::Action(_)) {
                    if e.active {
                        e.plugin.command(&ctx!(r, a, e), key, &[]);
                    }
                    return None;
                }
                r.cfg
                    .borrow_mut()
                    .plugins
                    .entry(id.into())
                    .or_default()
                    .settings
                    .insert(key.into(), value.into());
                save(&r.cfg);
                if e.active {
                    e.plugin.setting_changed(&ctx!(r, a, e), key);
                }
                None
            }
        }
    })
    .flatten()
}

fn save(cfg: &Rc<RefCell<Config>>) {
    if let Err(e) = cfg.borrow().save() {
        eprintln!("vivi: save config: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let m = Manifest::builtin("x", "X", "", "plugins")
            .setting(SettingSpec::toggle("a", "A", "", true))
            .default_off();
        let mut c = Config::default();
        assert!(!is_enabled(&c, &m));
        assert_eq!(setting_value(&c, &m, "a"), "true");
        let st = c.plugins.entry("x".into()).or_default();
        st.enabled = Some(true);
        st.settings.insert("a".into(), "false".into());
        assert!(is_enabled(&c, &m));
        assert_eq!(setting_value(&c, &m, "a"), "false");
    }

    #[test]
    fn builtin_ids_unique() {
        let ids: Vec<String> = builtin().iter().map(|p| p.manifest().id).collect();
        let mut d = ids.clone();
        d.sort();
        d.dedup();
        assert_eq!(ids.len(), d.len());
        for id in &ids {
            assert!(id.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        }
    }

    /// `plugins/index.json` must describe every built-in plugin as its manifest does.
    #[test]
    fn index_json_matches_manifests() {
        let idx: serde_json::Value =
            serde_json::from_str(include_str!("../../plugins/index.json")).unwrap();
        let list = idx["plugins"].as_array().unwrap();
        for p in builtin() {
            let m = p.manifest();
            let e = list
                .iter()
                .find(|e| e["id"] == m.id.as_str())
                .unwrap_or_else(|| panic!("{} missing from index.json", m.id));
            assert_eq!(e["name"], m.name.as_str());
            assert_eq!(e["icon"], m.icon.as_str());
            assert_eq!(e["kind"], "builtin");
            assert_eq!(e["default_enabled"], m.default_enabled);
            let keys: Vec<&str> = e["settings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["key"].as_str().unwrap())
                .collect();
            let want: Vec<&str> = m.settings.iter().map(|s| s.key.as_str()).collect();
            assert_eq!(keys, want, "{}", m.id);
        }
    }

    #[test]
    fn vivi_toast_messages() {
        let m = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            toast_text(&m(&["vivi-toast", "Night mode on"])),
            Some("Night mode on")
        );
        assert_eq!(
            toast_text(&m(&["vivi-toast", "A: 0:01", "3"])),
            Some("A: 0:01")
        );
        assert_eq!(toast_text(&m(&["vivi-toast", "  "])), None);
        assert_eq!(toast_text(&m(&["other", "x"])), None);
    }

    /// End to end with real libmpv: load the packaged scripts the way Vivi does, send the
    /// Alt combos Vivi forwards and expect their `vivi-toast` messages.
    #[test]
    fn packaged_scripts_answer_forwarded_alt_keys() {
        use libmpv2::events::{Event, EventContext};
        let mpv = libmpv2::Mpv::with_initializer(|i| {
            i.set_property("vo", "null")?;
            i.set_property("ao", "null")?;
            i.set_property("idle", "yes")?;
            i.set_property("input-default-bindings", "no")?;
            i.set_property("config", "no")?;
            Ok(())
        })
        .expect("libmpv");
        let mut ev = EventContext::new(mpv.ctx);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
        for (id, key, want) in [
            ("night-mode", "Alt+n", "Night mode"),
            ("ab-loop", "Alt+c", "A-B loop cleared"),
        ] {
            let path = root.join(id).join(format!("{id}.lua"));
            mpv.command("load-script", &[&format!("\"{}\"", path.display())])
                .expect("load-script");
            let start = std::time::Instant::now();
            let mut got = None;
            let mut next = start;
            while got.is_none() && start.elapsed() < Duration::from_secs(15) {
                if std::time::Instant::now() >= next {
                    mpv.command("keypress", &[key]).unwrap();
                    next = std::time::Instant::now() + Duration::from_millis(300);
                }
                if let Some(Ok(Event::ClientMessage(a))) = ev.wait_event(0.1) {
                    let a: Vec<String> = a.iter().map(|s| s.to_string()).collect();
                    if let Some(t) = toast_text(&a) {
                        got = Some(t.to_string());
                    }
                }
            }
            let got = got.unwrap_or_else(|| panic!("{id}: no vivi-toast after {key}"));
            assert!(got.starts_with(want), "{id}: {got:?}");
        }
    }
}
