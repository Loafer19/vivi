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

pub mod auto_tagger;
pub mod contrib;
pub mod epg;
pub mod free_music;
pub mod host;
pub mod lrclib;
pub mod lyrics;
pub mod network_streams;
pub mod packages;
pub mod protocol;
pub mod scripts;
pub mod skip_intro;
pub mod subtitle_search;
pub mod tagsources;
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
    /// Icon key mapped to an SVG in `ui/core/icons.slint` (`Icons.plugin`).
    pub icon: String,
    pub settings: Vec<SettingSpec>,
    pub default_enabled: bool,
    pub origin: Origin,
    /// Source (content browser, listed under Sources) or feature (embedded in the UI).
    pub role: contrib::Role,
    /// Where the plugin shows up (see [`contrib`]).
    pub contributes: Vec<contrib::Contribution>,
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
            role: contrib::Role::Feature,
            contributes: Vec::new(),
        }
    }
    /// A content browser: listed under Sources in the sidebar; `command` opens it.
    pub fn source(mut self, command: &str) -> Self {
        self.role = contrib::Role::Source;
        let name = self.name.clone();
        self.contribute(contrib::SIDEBAR_SOURCE, &name, command)
    }
    /// Show `label` at contribution point `point`; choosing it runs `command`.
    pub fn contribute(mut self, point: &str, label: &str, command: &str) -> Self {
        self.contributes
            .push(contrib::Contribution::new(point, label, command));
        self
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
    scripts: &'a [scripts::Script],
}

impl Ctx<'_> {
    /// mpv script name of an enabled downloadable plugin (package) `id`, for
    /// `script-message-to` (e.g. `offline-downloads` → `offline_downloads`).
    pub fn package_script(&self, id: &str) -> Option<String> {
        let s = self
            .scripts
            .iter()
            .find(|s| s.manifest.id == id && s.manifest.origin == Origin::Package)?;
        is_enabled(&self.cfg.borrow(), &s.manifest)
            .then(|| s.path.file_stem())
            .flatten()
            .map(|n| n.to_string_lossy().replace('-', "_"))
    }
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
        toast(self.app, msg);
    }
    /// A desktop notification (`notify` contribution point): only for plugins that
    /// declare `notify` and while the user allows it; otherwise the toast. Host API for
    /// built-ins; none needs it right now (Free Music's notices come from Downloads).
    #[allow(dead_code)]
    pub fn notify(&self, title: &str, body: &str) {
        notify_or_toast(self.app, &self.cfg.borrow(), self.manifest, title, body);
    }
    /// Local path of the playing file, if it is a file.
    pub fn local_path(&self) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(self.player.get("path")?);
        p.is_file().then_some(p)
    }
}

/// The mpv script name of package `id` when it is installed and enabled (outside
/// plugin callbacks; inside use [`Ctx::package_script`]).
pub fn package_script(id: &str) -> Option<String> {
    with(|r| {
        let c = r.cfg.borrow();
        let name = r
            .scripts
            .iter()
            .find(|s| s.manifest.id == id && s.manifest.origin == Origin::Package)
            .filter(|s| is_enabled(&c, &s.manifest))
            .and_then(|s| s.path.file_stem())
            .map(|n| n.to_string_lossy().replace('-', "_"));
        name
    })
    .flatten()
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

/// Whether built-in plugin `id` is on right now.
pub fn active(id: &str) -> bool {
    with(|r| r.entries.iter().any(|e| e.active && e.manifest.id == id)).unwrap_or(false)
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
    /// A value the app asks for (e.g. the control socket), downcast by the caller.
    fn query(&mut self, _cx: &Ctx, _name: &str) -> Option<Box<dyn std::any::Any>> {
        None
    }
}

/// All compiled-in plugins, in Settings order.
pub fn builtin() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(crate::viz::VizPlugin),
        Box::new(skip_intro::SkipIntro::default()),
        Box::new(lyrics::Lyrics::default()),
        Box::new(subtitle_search::SubtitleSearch::default()),
        Box::new(auto_tagger::AutoTagger::default()),
        Box::new(free_music::FreeMusic::default()),
        Box::new(network_streams::NetworkStreams::default()),
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
            scripts: &$reg.scripts,
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

/// All plugins with whether each is enabled (for the control socket).
#[cfg(unix)]
pub fn states() -> Vec<(Manifest, bool)> {
    with(|r| {
        let c = r.cfg.borrow();
        r.entries
            .iter()
            .map(|e| (e.manifest.clone(), e.active))
            .chain(
                r.scripts
                    .iter()
                    .map(|s| (s.manifest.clone(), is_enabled(&c, &s.manifest))),
            )
            .collect()
    })
    .unwrap_or_default()
}

/// Create the registry, enable plugins per config and start the tick timer.
pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>) {
    // Packages check this before sending `vivi-playlist` (older Vivi: unset; they write
    // the playlist file themselves).
    if let Err(e) = player.mpv.set_property("user-data/vivi/playlists", 1i64) {
        eprintln!("vivi: set user-data/vivi/playlists: {e:?}");
    }
    // The A-B loop keys and frame step are built in (src/abloop.rs): the deprecated
    // A-B Loop package checks this and leaves keys and display to the core.
    if let Err(e) = player.mpv.set_property("user-data/vivi/ab-loop", 1i64) {
        eprintln!("vivi: set user-data/vivi/ab-loop: {e:?}");
    }
    // The equalizer is built in (src/fx.rs): the deprecated Equalizer package checks this,
    // stops applying its own filter and hands its preset over (`vivi-eq-preset` /
    // `vivi-eq-import`).
    if let Err(e) = player.mpv.set_property("user-data/vivi/equalizer", 1i64) {
        eprintln!("vivi: set user-data/vivi/equalizer: {e:?}");
    }
    // The per-file subtitle offset is built in (src/suboffset.rs): Subtitle Auto-Sync
    // checks `user-data/vivi/sub-offset` and leaves Alt+Z / Alt+X and storage to it.
    crate::suboffset::setup(player);
    // Repeat / shuffle follow mpv (src/loopsync.rs); packages offer shuffle when this exists.
    crate::loopsync::setup(app, player);
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
        let c = protocol::PluginCommand::from_ui(&id, &name, &args);
        command(&c.id, &c.name, &c.args);
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
            secrets_file: p.manifest.secrets_file,
        });
    }
    v
}

/// mpv `script-opts` entries for a package setting (mp.options wants yes/no booleans).
///
/// `mp.options.read_options(o, "youtube")` takes every `youtube-*` key, so the
/// `youtube-music-folder` of a `youtube-music` package would reach the `youtube` script
/// as `music-folder`. Script-opts keys are global, so the host avoids the clash: a
/// package id with dashes also gets its options under the id with underscores
/// (`youtube_music-folder`, mpv's own script name, which no other id can be a dashed
/// prefix of, since ids have no underscores), and the dashed form is only set while no
/// other script's id plus `-` is a prefix of it. Packages read
/// `read_options(o, id:gsub("-", "_"))` to be safe; older ones that use the dashed id keep
/// working while there is no clash.
fn script_opts<'a>(
    id: &str,
    key: &str,
    value: &str,
    others: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let v = match value {
        "true" => "yes",
        "false" => "no",
        v => v,
    };
    let v = v.replace(',', " ");
    let safe = id.replace('-', "_");
    let clash = others
        .into_iter()
        .any(|o| o != id && id.strip_prefix(o).is_some_and(|r| r.starts_with('-')));
    let mut out = Vec::new();
    if !clash {
        out.push(format!("{id}-{key}={v}"));
    }
    if safe != id || clash {
        out.push(format!("{safe}-{key}={v}"));
    }
    out
}

/// Private file with the `secret` settings of package `id` (`secrets_file = true`).
pub fn secrets_path(id: &str) -> std::path::PathBuf {
    host::data_dir(id).join("vivi-secrets.json")
}

/// JSON object of `secret` setting values for [`secrets_path`].
fn secrets_json(c: &Config, m: &Manifest) -> String {
    let map: serde_json::Map<String, serde_json::Value> = m
        .settings
        .iter()
        .filter(|st| matches!(st.kind, SettingKind::Secret))
        .map(|st| (st.key.clone(), setting_value(c, m, &st.key).into()))
        .collect();
    serde_json::Value::Object(map).to_string()
}

/// Revision of the secrets files, so `<id>-secrets` changes on every write.
static SECRETS_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Registry {
    /// Set a package option in mpv's `script-opts` (see [`script_opts`]). `secret`
    /// settings of packages with `secrets_file` go to [`Registry::hand_secrets`] instead:
    /// mpv logs every `script-opts` change with its value (`change-list` at debug level,
    /// property writes at verbose level, and a log file always gets both).
    fn set_opt(&self, id: &str, key: &str, value: &str) {
        let s = self.scripts.iter().find(|s| s.manifest.id == id);
        let secret = s.is_some_and(|s| {
            s.secrets_file
                && s.manifest
                    .settings
                    .iter()
                    .any(|st| st.key == key && matches!(st.kind, SettingKind::Secret))
        });
        if secret {
            if let Some(s) = s {
                self.hand_secrets(&s.manifest);
            }
            return;
        }
        let ids = self.scripts.iter().map(|s| s.manifest.id.as_str());
        for o in script_opts(id, key, value, ids) {
            self.player
                .cmd("change-list", &["script-opts", "append", &o]);
        }
    }

    /// Write the `secret` settings of package `m` to its private file (0600 in a 0700
    /// folder) and tell the package with `<id>-secrets=<revision>:<path>` (no secret in
    /// mpv, so none in its log). The package reads the file on start and whenever that
    /// option changes.
    fn hand_secrets(&self, m: &Manifest) {
        let path = secrets_path(&m.id);
        let json = secrets_json(&self.cfg.borrow(), m);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
            host::make_private(d);
        }
        if let Err(e) = crate::store::write_private(&path, json.as_bytes()) {
            eprintln!("vivi: {}: write secrets: {e}", m.id);
            return;
        }
        let rev = SECRETS_REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let v = format!("{rev}:{}", path.display());
        let ids = self.scripts.iter().map(|s| s.manifest.id.as_str());
        for o in script_opts(&m.id, "secrets", &v, ids) {
            self.player
                .cmd("change-list", &["script-opts", "append", &o]);
        }
    }

    /// Load an mpv script once (packages get their settings as script-opts first).
    fn load_script(&mut self, s: &scripts::Script) {
        if !self.loaded.insert(s.manifest.id.clone()) {
            return;
        }
        if s.manifest.origin == Origin::Package {
            let c = self.cfg.borrow().clone();
            for st in &s.manifest.settings {
                if matches!(st.kind, SettingKind::Action(_))
                    || (s.secrets_file && matches!(st.kind, SettingKind::Secret))
                {
                    continue;
                }
                let v = setting_value(&c, &s.manifest, &st.key);
                self.set_opt(&s.manifest.id, &st.key, &v);
            }
            if s.secrets_file {
                self.hand_secrets(&s.manifest);
            }
            // Where the package lives (helpers are in `<pkgdir>/bin`).
            if let Some(dir) = s.path.parent() {
                self.set_opt(&s.manifest.id, "pkgdir", &dir.display().to_string());
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

/// Toast text of a `script-message vivi-toast <text> [seconds]` (tests).
#[cfg(test)]
fn toast_text(args: &[String]) -> Option<&str> {
    match protocol::ScriptMessage::parse(args)? {
        protocol::ScriptMessage::Toast { text, .. } => Some(text),
        _ => None,
    }
}

/// Desktop notification for plugin `m` when it contributes `notify` and the option is
/// on (and a notification service exists); otherwise a toast.
fn notify_or_toast(a: &AppWindow, c: &Config, m: &Manifest, title: &str, body: &str) {
    let declared = m.contributes.iter().any(|x| x.point == contrib::NOTIFY);
    if declared && crate::notify::enabled(c) && crate::notify::send(title, body) {
        return;
    }
    toast(a, &crate::notify::toast_text(title, body));
}

/// A `script-message` broadcast reached Vivi's mpv client.
pub fn client_message(args: &[String]) {
    use protocol::ScriptMessage as M;
    match M::parse(args) {
        Some(M::Setting { id, key, value }) => store_setting(id, key, value),
        Some(M::PickFolder { id, key }) => pick_folder(id, key),
        // Subtitle Auto-Sync 1.2+: offset keys and sync results, saved per file by Vivi.
        Some(M::SubOffset(v)) => {
            if let Some((a, p)) = with(|r| r.app.upgrade().map(|a| (a, r.player.clone()))).flatten()
            {
                crate::suboffset::command(&a, &p, v);
            }
        }
        Some(M::SubOffsetImport) => {
            if let Some(n) = crate::suboffset::import_package_offsets() {
                eprintln!("vivi: imported {n} subtitle offset(s) from Subtitle Auto-Sync");
            }
        }
        Some(M::TaggerAcoustid { path, json }) => tagger_acoustid(path, json),
        Some(M::Qr {
            title,
            text,
            caption,
        }) => {
            with(|r| {
                if let Some(a) = r.app.upgrade() {
                    crate::dialogs::show_qr(&a, title, text, caption);
                }
            });
        }
        Some(M::Notify { id, title, body }) => {
            with(|r| {
                let Some(a) = r.app.upgrade() else { return };
                let c = r.cfg.borrow();
                match r.scripts.iter().find(|s| s.manifest.id == id) {
                    Some(s) => notify_or_toast(&a, &c, &s.manifest, title, body),
                    None => toast(&a, &crate::notify::toast_text(title, body)),
                }
            });
        }
        Some(M::Download { id, job, json }) => match serde_json::from_str(json) {
            Ok(r) => crate::downloads::report(id, job, r),
            Err(e) => eprintln!("vivi: vivi-download {id} {job}: {e}"),
        },
        Some(M::DownloadRemove { id, job }) => crate::downloads::remove(id, job),
        Some(M::Playlist { id, json }) => match serde_json::from_str(json) {
            Ok(r) => save_playlist(id, r),
            Err(e) => eprintln!("vivi: vivi-playlist {id}: {e}"),
        },
        Some(M::DownloadsShow) => {
            if let Some(a) = with(|r| r.app.upgrade()).flatten() {
                crate::downloads::open(&a);
            }
        }
        Some(M::Toast { text, secs }) => toast_after_resume(text.to_string(), toast_secs(secs), 10),
        Some(m @ (M::Dialog { .. } | M::DialogClose(_) | M::Confirm { .. })) => {
            let Some(a) = with(|r| r.app.upgrade()).flatten() else {
                return;
            };
            match m {
                M::Dialog { id, json } => match serde_json::from_str(json) {
                    Ok(u) => host::dialog(&a, id, u),
                    Err(e) => eprintln!("vivi: {id}: bad vivi-dialog json: {e}"),
                },
                M::DialogClose(id) => host::dialog_close(&a, id),
                M::Confirm { id, key, json } => match serde_json::from_str(json) {
                    Ok(c) => host::confirm(&a, id, key, c),
                    Err(e) => eprintln!("vivi: {id}: bad vivi-confirm json: {e}"),
                },
                _ => {}
            }
        }
        Some(M::MediaMeta { id, json }) => match serde_json::from_str(json) {
            Ok(m) => host::media_meta(id, m),
            Err(e) => eprintln!("vivi: {id}: bad vivi-media-meta json: {e}"),
        },
        Some(M::Protect { id, path }) => {
            host::protect(id, path);
        }
        Some(M::Wipe { id, path }) => {
            host::wipe(id, path);
        }
        // Skip ranges from packages (SponsorBlock): one Skip button in Skip Intro &
        // Credits. Ignored while it is off (`user-data/vivi/skip-api` is 0 then).
        Some(M::SkipRanges { source, json }) => command(
            skip_intro::ID,
            "ranges",
            &[source.to_string(), json.to_string()],
        ),
        Some(M::SkipRangesClear(source)) => {
            command(skip_intro::ID, "ranges-clear", &[source.to_string()])
        }
        Some(M::SkipUndo) => command(skip_intro::ID, "undo", &[]),
        Some(M::EqPreset(name)) => {
            if let Some(a) = with(|r| r.app.upgrade()).flatten() {
                crate::fx::preset_action(&a, "select", name, "");
            }
        }
        Some(M::EqImport(json)) => {
            if let Some(a) = with(|r| r.app.upgrade()).flatten() {
                crate::fx::preset_action(&a, "import-text", "", json);
            }
        }
        _ => {}
    }
}

/// `script-message vivi-tagger-acoustid <path> <json>` (AcoustID package): open the
/// Auto-tagger's candidates and preview for that file.
fn tagger_acoustid(path: &str, json: &str) {
    let on = with(|r| {
        r.entries
            .iter()
            .any(|e| e.active && e.manifest.id == auto_tagger::ID)
    })
    .unwrap_or(false);
    if on {
        command(
            auto_tagger::ID,
            "acoustid",
            &[path.to_string(), json.to_string()],
        );
    } else {
        with(|r| {
            if let Some(a) = r.app.upgrade() {
                toast(
                    &a,
                    "Turn on the Auto-tagger plugin to review AcoustID matches",
                );
            }
        });
    }
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
        r.set_opt(id, key, value);
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

/// Default toast duration, and the range a plugin's `vivi-toast <text> <seconds>` is
/// clamped to (a toast is not a dialog: long messages belong in a notification).
pub const TOAST_SECS: f64 = 4.0;
pub const TOAST_MIN_SECS: f64 = 1.5;
pub const TOAST_MAX_SECS: f64 = 15.0;

/// How long a plugin toast stays: the asked seconds within [`TOAST_MIN_SECS`]..
/// [`TOAST_MAX_SECS`], [`TOAST_SECS`] when it asked for none.
pub fn toast_secs(asked: Option<f64>) -> f64 {
    asked
        .map(|s| s.clamp(TOAST_MIN_SECS, TOAST_MAX_SECS))
        .unwrap_or(TOAST_SECS)
}

/// Show `msg` in the toast for 4 s (on every screen: library, player, Settings,
/// Extensions and the other dialogs).
pub fn toast(app: &AppWindow, msg: &str) {
    toast_for(app, msg, TOAST_SECS);
}

/// Show `msg` in the toast for `secs` seconds.
pub fn toast_for(app: &AppWindow, msg: &str, secs: f64) {
    app.global::<crate::Notices>().set_resume_text(msg.into());
    let w = app.as_weak();
    let m = msg.to_string();
    slint::Timer::single_shot(Duration::from_secs_f64(secs), move || {
        if let Some(a) = w.upgrade() {
            if a.global::<crate::Notices>().get_resume_text().as_str() == m {
                a.global::<crate::Notices>().set_resume_text("".into());
            }
        }
    });
}

/// A plugin toast (`vivi-toast`) waits, up to `tries` seconds, while the resume prompt
/// ("Resumed from …" with *Start over*) holds the shared notice line: shown at once it
/// would take the prompt and its button away (packages often toast at `file-loaded`).
fn toast_after_resume(text: String, secs: f64, tries: u32) {
    let Some(a) = with(|r| r.app.upgrade()).flatten() else {
        return;
    };
    let n = a.global::<crate::Notices>();
    let offer = n.get_resume_offer();
    if tries > 0 && !offer.is_empty() && n.get_resume_text() == offer {
        slint::Timer::single_shot(Duration::from_secs(1), move || {
            toast_after_resume(text, secs, tries - 1)
        });
        return;
    }
    toast_for(&a, &text, secs);
}

/// mpv loaded a new file.
pub fn file_loaded() {
    packages::deprecation_notice();
    with(|r| {
        let Some(a) = r.app.upgrade() else { return };
        for e in r.entries.iter_mut().filter(|e| e.active) {
            e.plugin.file_loaded(&ctx!(r, a, e));
        }
    });
}

/// Send a command to one plugin (no-op when it is disabled).
/// UI callbacks of a package's host dialog are forwarded to it (see [`host`]).
pub fn command(id: &str, name: &str, args: &[String]) {
    let builtin = with(|r| {
        let Some(a) = r.app.upgrade() else {
            return true;
        };
        if let Some(e) = r
            .entries
            .iter_mut()
            .find(|e| e.active && e.manifest.id == id)
        {
            e.plugin.command(&ctx!(r, a, e), name, args);
            return true;
        }
        r.entries.iter().any(|e| e.manifest.id == id)
    })
    .unwrap_or(true);
    if !builtin {
        host::ui_event(id, name, args);
    }
}

/// One entry an enabled plugin contributes to a surface (see [`contrib`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Contributed {
    pub id: String,
    pub name: String,
    /// Icon key: the contribution's own or the plugin's.
    pub icon: String,
    pub label: String,
    pub command: String,
}

/// Config entry holding the order of the sidebar's Sources (`order = "a,b"`).
pub const SOURCES_ID: &str = "vivi-sources";
/// Per-plugin setting that hides a source from the sidebar (`"true"`).
pub const HIDDEN_KEY: &str = "ui.hidden";
/// Config entry holding the order of the Home widgets (`order = "a,b"`).
pub const HOME_ID: &str = "vivi-home";
/// Per-plugin setting that hides its Home widgets (`"true"`).
pub const HOME_HIDDEN_KEY: &str = "ui.home-hidden";

/// Surfaces the user can arrange: (config entry with the order, per-plugin hide key).
fn layout(point: &str) -> Option<(&'static str, &'static str)> {
    match point {
        contrib::SIDEBAR_SOURCE => Some((SOURCES_ID, HIDDEN_KEY)),
        contrib::HOME_WIDGET => Some((HOME_ID, HOME_HIDDEN_KEY)),
        _ => None,
    }
}

/// Whether the user hid plugin `id`'s entries at `point` (Sources, Home widgets).
pub fn hidden_at(c: &Config, point: &str, id: &str) -> bool {
    let Some((_, key)) = layout(point) else {
        return false;
    };
    c.plugins
        .get(id)
        .and_then(|s| s.settings.get(key))
        .is_some_and(|v| v == "true")
}

/// Whether the user hid source `id` from the sidebar.
pub fn source_hidden(c: &Config, id: &str) -> bool {
    hidden_at(c, contrib::SIDEBAR_SOURCE, id)
}

/// User order of the plugins at `point` (ids; unknown ones go last).
fn order_at(c: &Config, point: &str) -> Vec<String> {
    layout(point)
        .and_then(|(entry, _)| c.plugins.get(entry))
        .and_then(|s| s.settings.get("order"))
        .map(|o| {
            o.split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Sort `v` by the user's order, keeping manifest order for the rest.
fn order_by(order: &[String], v: &mut [Contributed]) {
    v.sort_by_key(|e| order.iter().position(|o| *o == e.id).unwrap_or(usize::MAX));
}

/// Move plugin `id` `"up"` / `"down"` at an arrangeable `point`, or `"hide"` / `"show"` it.
pub fn arrange(point: &str, id: &str, what: &str) {
    let Some((entry, key)) = layout(point) else {
        return;
    };
    let mut ids: Vec<String> = contributions_all(point).into_iter().map(|e| e.id).collect();
    ids.dedup();
    with(|r| {
        let mut c = r.cfg.borrow_mut();
        match what {
            "hide" | "show" => {
                c.plugins
                    .entry(id.into())
                    .or_default()
                    .settings
                    .insert(key.into(), (what == "hide").to_string());
            }
            "up" | "down" => {
                let mut order = ids.clone();
                if let Some(i) = order.iter().position(|o| o == id) {
                    let j = if what == "up" {
                        i.saturating_sub(1)
                    } else {
                        (i + 1).min(order.len() - 1)
                    };
                    order.swap(i, j);
                }
                c.plugins
                    .entry(entry.into())
                    .or_default()
                    .settings
                    .insert("order".into(), order.join(","));
            }
            _ => return,
        }
        drop(c);
        save(&r.cfg);
    });
}

/// Move source `id` up / down in the sidebar, or hide / show it (see [`arrange`]).
pub fn arrange_source(id: &str, what: &str) {
    arrange(contrib::SIDEBAR_SOURCE, id, what);
}

/// Contributions of every enabled plugin at `point`, hidden ones included, in the
/// user's order (Sources, Home widgets).
pub fn contributions_all(point: &str) -> Vec<Contributed> {
    with(|r| {
        let c = r.cfg.borrow();
        let mut v: Vec<Contributed> = r
            .entries
            .iter()
            .filter(|e| e.active)
            .map(|e| &e.manifest)
            .chain(
                r.scripts
                    .iter()
                    .map(|s| &s.manifest)
                    .filter(|m| is_enabled(&c, m)),
            )
            .flat_map(|m| {
                m.contributes
                    .iter()
                    .filter(|x| x.point == point)
                    .map(|x| Contributed {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        icon: if x.icon.is_empty() {
                            m.icon.clone()
                        } else {
                            x.icon.clone()
                        },
                        label: if x.label.is_empty() {
                            m.name.clone()
                        } else {
                            x.label.clone()
                        },
                        command: x.command.clone(),
                    })
            })
            .collect();
        order_by(&order_at(&c, point), &mut v);
        v
    })
    .unwrap_or_default()
}

/// What a surface shows at `point` (entries the user hid left out).
pub fn contributions(point: &str) -> Vec<Contributed> {
    let mut v = contributions_all(point);
    // A plugin's entry that moved to another plugin, when both are enabled (contrib::MOVED).
    let pairs: Vec<(&str, &str)> = v
        .iter()
        .map(|e| (e.id.as_str(), e.command.as_str()))
        .collect();
    let drop = contrib::superseded(&pairs);
    if !drop.is_empty() {
        let mut i = 0;
        v.retain(|_| {
            i += 1;
            !drop.contains(&(i - 1))
        });
    }
    if layout(point).is_some() {
        with(|r| {
            let c = r.cfg.borrow();
            v.retain(|e| !hidden_at(&c, point, &e.id));
        });
    }
    v
}

/// Run a contribution: a built-in plugin's command, a package's action
/// (`script-message-to`), or — with an empty command — the plugin's Extensions tab.
/// Returns false when the plugin is unknown or off.
pub fn run(id: &str, cmd: &str, args: &[String]) -> bool {
    enum Target {
        Builtin,
        Script(String, bool),
    }
    let t = with(|r| {
        if r.entries.iter().any(|e| e.active && e.manifest.id == id) {
            return Some(Target::Builtin);
        }
        let s = r.scripts.iter().find(|s| s.manifest.id == id)?;
        let name = client_name(&s.path).replace('-', "_");
        Some(Target::Script(name, r.loaded.contains(id)))
    })
    .flatten();
    let Some(t) = t else { return false };
    if cmd.is_empty() {
        if let Some(a) = with(|r| r.app.upgrade()).flatten() {
            crate::extwin::open(&a, id, false);
        }
        return true;
    }
    match t {
        Target::Builtin => command(id, cmd, args),
        Target::Script(name, loaded) => {
            with(|r| {
                if loaded {
                    let mut v: Vec<&str> = vec![&name, cmd];
                    v.extend(args.iter().map(String::as_str));
                    r.player.cmd("script-message-to", &v);
                } else if let Some(a) = r.app.upgrade() {
                    toast(&a, "This plugin is not running yet: restart Vivi");
                }
            });
        }
    }
    true
}

/// Redraw the sidebar playlists and the Library (a plugin wrote a playlist).
fn refresh_playlists() {
    // Outside `with`: the Library render reads the plugin registry.
    if let Some(a) = with(|r| r.app.upgrade()).flatten() {
        a.global::<crate::Panel>()
            .set_playlists(crate::playlist_items());
        a.global::<crate::Library>().invoke_refresh();
    }
}

/// A `vivi-playlist` message.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct PlaylistMsg {
    /// Display name (the file name unless that is taken by another playlist).
    pub name: String,
    /// Opaque id of the online list; the host passes it back in `playlist-sync` /
    /// `playlist-open`.
    pub source: String,
    /// Web page of the list (opened by "Open Source" when the plugin is off).
    pub url: String,
    /// Plain file paths, or `entries` with titles.
    pub files: Vec<String>,
    pub entries: Vec<crate::store::PlaylistEntry>,
    /// Local image (or http(s) URL) for the playlist cover.
    pub cover: String,
}

/// Write a plugin's playlist, mark it with its source, set the cover, refresh the
/// Playlists tab and answer `playlist-saved <source> <path>` (or `playlist-failed
/// <source> <error>`).
pub fn save_playlist(id: &str, m: PlaylistMsg) {
    let label = manifests()
        .into_iter()
        .find(|x| x.id == id)
        .map(|x| x.name)
        .unwrap_or_default();
    if label.is_empty() || m.source.trim().is_empty() {
        eprintln!("vivi: vivi-playlist {id}: unknown plugin or no source");
        return;
    }
    let src = crate::store::PlaylistSource {
        plugin: id.into(),
        source: m.source.trim().into(),
        url: m.url.trim().into(),
    };
    let mut entries = m.entries;
    entries.extend(m.files.into_iter().map(|path| crate::store::PlaylistEntry {
        path,
        ..Default::default()
    }));
    let name = if m.name.trim().is_empty() {
        label.clone()
    } else {
        m.name.trim().to_string()
    };
    let path = match crate::store::save_owned_playlist(&name, &src, &label, &entries) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("vivi: vivi-playlist {id}: {e}");
            run(id, "playlist-failed", &[src.source, e.to_string()]);
            return;
        }
    };
    let cover = m.cover.trim().to_string();
    let set_cover =
        |path: &std::path::Path, img: &std::path::Path| match crate::store::set_playlist_cover(
            path, img,
        ) {
            Ok(p) => crate::images::forget(&p),
            Err(e) => eprintln!("vivi: vivi-playlist cover: {e}"),
        };
    if cover.starts_with("https://") || cover.starts_with("http://") {
        match host::cached_cover(&cover) {
            Some(img) => set_cover(&path, &img),
            None => {
                // Fetched off the UI thread; the tab refreshes when it lands.
                let path = path.clone();
                std::thread::spawn(move || {
                    let Some(img) = host::fetch_cover(&cover) else {
                        return;
                    };
                    let _ = slint::invoke_from_event_loop(move || {
                        set_cover(&path, &img);
                        refresh_playlists();
                    });
                });
            }
        }
    } else if !cover.is_empty() {
        let img = std::path::PathBuf::from(cover.strip_prefix("file://").unwrap_or(&cover));
        if img.is_file() {
            set_cover(&path, &img);
        }
    }
    refresh_playlists();
    run(
        id,
        "playlist-saved",
        &[src.source, path.to_string_lossy().into_owned()],
    );
}

/// Ask an enabled plugin for a value (see [`Plugin::query`]).
pub fn query<T: 'static>(id: &str, name: &str) -> Option<T> {
    with(|r| {
        let a = r.app.upgrade()?;
        let e = r
            .entries
            .iter_mut()
            .find(|e| e.active && e.manifest.id == id)?;
        e.plugin.query(&ctx!(r, a, e), name)
    })
    .flatten()
    .and_then(|b| b.downcast::<T>().ok())
    .map(|b| *b)
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
                r.set_opt(id, key, value);
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
                    eprintln!("vivi: script {id}: enabled, loading it now");
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
    fn script_opts_do_not_leak_into_prefix_ids() {
        let ids = ["youtube", "youtube-music", "offline-downloads"];
        // The longer id is not set under `youtube-…`, which the `youtube` script would read.
        assert_eq!(
            script_opts("youtube-music", "folder", "/m", ids),
            ["youtube_music-folder=/m"]
        );
        assert_eq!(script_opts("youtube", "url", "u", ids), ["youtube-url=u"]);
        // No clash: the dashed form (older packages) plus the underscore one.
        assert_eq!(
            script_opts("offline-downloads", "on", "true", ids),
            ["offline-downloads-on=yes", "offline_downloads-on=yes"]
        );
        assert_eq!(
            script_opts("youtube-music", "x", "a,b", ["youtube-music"]),
            ["youtube-music-x=a b", "youtube_music-x=a b"]
        );
        // "you" is not a dashed prefix of "youtube-music".
        assert_eq!(script_opts("youtube-music", "x", "1", ["you"]).len(), 2);
    }

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
    fn contributions_point_at_known_places() {
        for p in builtin() {
            let m = p.manifest();
            for c in &m.contributes {
                assert!(contrib::POINTS.contains(&c.point.as_str()), "{}", m.id);
            }
            assert_eq!(
                m.role == contrib::Role::Source,
                m.contributes
                    .iter()
                    .any(|c| c.point == contrib::SIDEBAR_SOURCE),
                "{}",
                m.id
            );
        }
        let mut v: Vec<Contributed> = ["a", "b", "c"]
            .iter()
            .map(|id| Contributed {
                id: id.to_string(),
                name: String::new(),
                icon: String::new(),
                label: String::new(),
                command: String::new(),
            })
            .collect();
        order_by(&["c".into(), "a".into()], &mut v);
        let ids: Vec<&str> = v.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
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

    /// The catalog (`plugins/index.json` in https://github.com/Loafer19/vivi-plugins, checked out
    /// in `$VIVI_PLUGINS_DIR`) must describe every built-in plugin as its manifest does.
    /// Skipped when `VIVI_PLUGINS_DIR` is not set.
    #[test]
    fn index_json_matches_manifests() {
        let Some(root) = std::env::var_os("VIVI_PLUGINS_DIR") else {
            eprintln!("VIVI_PLUGINS_DIR not set, skipping");
            return;
        };
        let text = std::fs::read_to_string(std::path::Path::new(&root).join("plugins/index.json"))
            .unwrap();
        let idx: serde_json::Value = serde_json::from_str(&text).unwrap();
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
        assert_eq!(toast_secs(None), TOAST_SECS);
        assert_eq!(toast_secs(Some(8.0)), 8.0);
        assert_eq!(toast_secs(Some(0.1)), TOAST_MIN_SECS);
        assert_eq!(toast_secs(Some(600.0)), TOAST_MAX_SECS);
    }

    /// End to end with real libmpv: load the packaged scripts the way Vivi does, send the
    /// Alt combos Vivi forwards and expect their `vivi-toast` messages.
    #[test]
    fn packaged_scripts_answer_forwarded_alt_keys() {
        // Packaged plugins live in Loafer19/vivi-plugins; point VIVI_PLUGINS_DIR at a
        // checkout of it to run this test. Skip when the scripts are not available.
        let root = std::env::var_os("VIVI_PLUGINS_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins"));
        let ids = ["night-mode", "ab-loop"];
        if !ids
            .iter()
            .all(|id| root.join(id).join(format!("{id}.lua")).is_file())
        {
            eprintln!(
                "skipping: packaged plugin scripts not found in {}",
                root.display()
            );
            return;
        }
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
