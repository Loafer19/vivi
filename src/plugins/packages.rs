//! Downloadable plugin packages.
//!
//! A package is a zip with a `manifest.toml` at its root and an mpv Lua/JavaScript entry
//! script (see `docs/plugins-architecture.md`). The catalog is `plugins/index.json`,
//! served by GitHub Pages at [`INDEX_URL`]; package entries carry the download URL (a
//! GitHub release asset tagged `plugin-<id>-v<version>`), its sha256 and size.
//!
//! Layout on disk (`$XDG_DATA_HOME/vivi/plugins`):
//!
//! ```text
//! <id>/<version>/manifest.toml, <entry>, ...   one directory per installed version
//! <id>/current                                  the active version (atomically replaced)
//! ```
//!
//! Install = download → verify sha256 → unpack into a staging dir → validate the manifest
//! → rename to `<id>/<version>` → switch `current`. The previous version stays for
//! [`rollback`]; older ones are pruned. Network and disk work run on background threads;
//! results come back to the UI thread with `slint::invoke_from_event_loop`.

use super::{Manifest, Origin, SettingKind, SettingSpec};
use crate::AppWindow;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Catalog published by the docs site (`plugins/index.json`).
pub const INDEX_URL: &str = "https://loafer19.github.io/vivi/index.json";
/// Config entry (`[plugins.packages]`) for the catalog settings.
pub const GLOBAL_ID: &str = "packages";
/// Automatic catalog checks run at most this often.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
const MAX_DOWNLOAD: u64 = 20 << 20;
const MAX_UNPACKED: u64 = 50 << 20;
/// Permissions a package may declare; shown before install.
pub const PERMISSIONS: [(&str, &str); 3] = [
    ("network", "connect to the internet"),
    ("filesystem", "read and write files outside its own folder"),
    ("subprocess", "run other programs"),
];

/// Catalog URL; `VIVI_PLUGIN_INDEX` overrides it (testing, mirrors).
pub fn index_url() -> String {
    std::env::var("VIVI_PLUGIN_INDEX").unwrap_or_else(|_| INDEX_URL.into())
}

/// Where packages are installed.
pub fn root() -> PathBuf {
    crate::store::data_dir().join("plugins")
}

fn cache_file() -> PathBuf {
    crate::store::cache_dir().join("plugin-index.json")
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct IndexSetting {
    pub key: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub default: String,
}

fn yes() -> bool {
    true
}

/// One catalog entry (only `kind = "package"` entries are kept).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub icon: String,
    pub kind: String,
    #[serde(default)]
    pub min_vivi: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct Index {
    schema: u32,
    plugins: Vec<Entry>,
}

/// Parse a catalog and keep the downloadable packages.
pub fn parse_index(s: &str) -> Result<Vec<Entry>, String> {
    let idx: Index = serde_json::from_str(s).map_err(|e| format!("bad catalog: {e}"))?;
    if idx.schema != 1 {
        return Err(format!("unsupported catalog schema {}", idx.schema));
    }
    Ok(idx
        .plugins
        .into_iter()
        .filter(|e| e.kind == "package" && !e.url.is_empty() && e.sha256.len() == 64)
        .collect())
}

/// `manifest.toml` inside a package.
#[derive(Debug, Clone, Deserialize)]
pub struct PkgManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub kind: String,
    pub entry: String,
    #[serde(default)]
    pub min_vivi: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default = "yes")]
    pub default_enabled: bool,
    #[serde(default)]
    pub settings: Vec<IndexSetting>,
}

impl PkgManifest {
    fn validate(&self) -> Result<(), String> {
        let id_ok = !self.id.is_empty()
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !id_ok {
            return Err(format!("invalid plugin id {:?}", self.id));
        }
        if parse_version(&self.version).is_none() {
            return Err(format!("invalid version {:?}", self.version));
        }
        if !matches!(self.kind.as_str(), "mpv-lua" | "mpv-js") {
            return Err(format!("unsupported plugin kind {:?}", self.kind));
        }
        let entry = Path::new(&self.entry);
        if entry.components().count() != 1 || self.entry.starts_with('.') {
            return Err(format!("invalid entry {:?}", self.entry));
        }
        if let Some(p) = self
            .permissions
            .iter()
            .find(|p| !PERMISSIONS.iter().any(|(k, _)| k == p))
        {
            return Err(format!("unknown permission {p:?}"));
        }
        Ok(())
    }

    /// Plugin manifest for the registry / Settings page.
    pub fn to_manifest(&self) -> Manifest {
        Manifest {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            version: self.version.clone(),
            author: self.author.clone(),
            icon: if self.icon.is_empty() {
                "plugins".into()
            } else {
                self.icon.clone()
            },
            settings: self.settings.iter().filter_map(to_spec).collect(),
            default_enabled: self.default_enabled,
            origin: Origin::Package,
        }
    }
}

fn to_spec(s: &IndexSetting) -> Option<SettingSpec> {
    let kind = match s.kind.as_str() {
        "toggle" => SettingKind::Toggle,
        "choice" if !s.options.is_empty() => SettingKind::Choice(s.options.clone()),
        "text" => SettingKind::Text(String::new()),
        "secret" => SettingKind::Secret,
        "action" => SettingKind::Action("Run".into()),
        _ => return None,
    };
    let default = match (&kind, s.default.is_empty()) {
        (SettingKind::Choice(o), true) => o[0].clone(),
        (SettingKind::Toggle, true) => "false".into(),
        _ => s.default.clone(),
    };
    Some(SettingSpec {
        key: s.key.clone(),
        title: s.title.clone(),
        info: String::new(),
        kind,
        default,
    })
}

// ---------- versions ----------

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let r = (
        it.next()??,
        it.next().flatten().unwrap_or(0),
        it.next().flatten().unwrap_or(0),
    );
    it.next().is_none().then_some(r)
}

/// Compare two semver-ish versions (pre-release tags ignored; invalid sorts first).
pub fn cmp_version(a: &str, b: &str) -> Ordering {
    parse_version(a).cmp(&parse_version(b))
}

/// Whether this Vivi is new enough for a package.
pub fn compatible(min_vivi: &str) -> bool {
    min_vivi.is_empty() || cmp_version(env!("CARGO_PKG_VERSION"), min_vivi) != Ordering::Less
}

// ---------- installed packages ----------

#[derive(Debug, Clone)]
pub struct Installed {
    pub manifest: PkgManifest,
    /// `<root>/<id>/<version>`
    pub dir: PathBuf,
    /// Other installed versions (rollback targets), newest first.
    pub previous: Vec<String>,
}

impl Installed {
    pub fn entry_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.entry)
    }
}

fn read_manifest(dir: &Path) -> Result<PkgManifest, String> {
    let s =
        fs::read_to_string(dir.join("manifest.toml")).map_err(|e| format!("manifest.toml: {e}"))?;
    let m: PkgManifest = toml::from_str(&s).map_err(|e| format!("manifest.toml: {e}"))?;
    m.validate()?;
    Ok(m)
}

fn versions(id_dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(id_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && parse_version(n).is_some())
        .collect();
    v.sort_by(|a, b| cmp_version(b, a));
    v
}

/// One installed package by id.
pub fn installed_one(root: &Path, id: &str) -> Option<Installed> {
    let id_dir = root.join(id);
    let cur = fs::read_to_string(id_dir.join("current")).ok()?;
    let cur = cur.trim();
    let dir = id_dir.join(cur);
    let manifest = read_manifest(&dir).ok()?;
    if manifest.id != id || manifest.version != cur || !dir.join(&manifest.entry).is_file() {
        return None;
    }
    let previous = versions(&id_dir).into_iter().filter(|v| v != cur).collect();
    Some(Installed {
        manifest,
        dir,
        previous,
    })
}

/// All installed packages, sorted by id.
pub fn installed(root: &Path) -> Vec<Installed> {
    let mut v: Vec<Installed> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| installed_one(root, e.file_name().to_str()?))
        .collect();
    v.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    v
}

fn set_current(root: &Path, id: &str, version: &str) -> std::io::Result<()> {
    let f = root.join(id).join("current");
    let tmp = root.join(id).join(".current.tmp");
    fs::write(&tmp, version)?;
    fs::rename(tmp, f)
}

/// Keep the current version and the newest other one.
fn prune(root: &Path, id: &str, current: &str) {
    let id_dir = root.join(id);
    let others: Vec<String> = versions(&id_dir)
        .into_iter()
        .filter(|v| v != current)
        .collect();
    for v in others.iter().skip(1) {
        let _ = fs::remove_dir_all(id_dir.join(v));
    }
    // Leftovers of interrupted installs.
    for e in fs::read_dir(&id_dir).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(".staging") {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Verify and unpack a downloaded package for catalog `entry`, then make it current.
pub fn install_bytes(root: &Path, entry: &Entry, data: &[u8]) -> Result<Installed, String> {
    let got = sha256_hex(data);
    if !got.eq_ignore_ascii_case(&entry.sha256) {
        return Err(format!(
            "checksum mismatch (expected {}, got {got})",
            entry.sha256
        ));
    }
    let id_dir = root.join(&entry.id);
    fs::create_dir_all(&id_dir).map_err(|e| e.to_string())?;
    let staging = id_dir.join(format!(".staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    let res = unpack(data, &staging).and_then(|()| {
        let m = read_manifest(&staging)?;
        if m.id != entry.id || m.version != entry.version {
            return Err(format!(
                "package is {} {}, catalog says {} {}",
                m.id, m.version, entry.id, entry.version
            ));
        }
        if !staging.join(&m.entry).is_file() {
            return Err(format!("entry {} missing from package", m.entry));
        }
        if !compatible(&m.min_vivi) {
            return Err(format!("needs Vivi {} or newer", m.min_vivi));
        }
        let dest = id_dir.join(&m.version);
        let _ = fs::remove_dir_all(&dest);
        fs::rename(&staging, &dest).map_err(|e| e.to_string())?;
        set_current(root, &m.id, &m.version).map_err(|e| e.to_string())?;
        prune(root, &m.id, &m.version);
        Ok(())
    });
    if res.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    res?;
    installed_one(root, &entry.id).ok_or_else(|| "installed package is unreadable".into())
}

fn unpack(data: &[u8], dest: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = f.enclosed_name() else {
            return Err(format!(
                "unsafe path in package: {}",
                f.name().unwrap_or_default()
            ));
        };
        let out = dest.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if f.is_symlink() {
            return Err(format!(
                "symlinks are not allowed: {}",
                f.name().unwrap_or_default()
            ));
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::new();
        (&mut f)
            .take(MAX_UNPACKED - total + 1)
            .read_to_end(&mut buf)
            .map_err(|e| e.to_string())?;
        total += buf.len() as u64;
        if total > MAX_UNPACKED {
            return Err("package is too large".into());
        }
        fs::write(&out, buf).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Switch to the newest other installed version. Returns it.
pub fn rollback(root: &Path, id: &str) -> Result<String, String> {
    let cur = installed_one(root, id).ok_or("not installed")?;
    let prev = cur.previous.first().ok_or("no previous version")?.clone();
    read_manifest(&root.join(id).join(&prev))?;
    set_current(root, id, &prev).map_err(|e| e.to_string())?;
    Ok(prev)
}

pub fn uninstall(root: &Path, id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err("invalid id".into());
    }
    fs::remove_dir_all(root.join(id)).map_err(|e| e.to_string())
}

/// Download with a size cap.
pub fn download(url: &str) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url)
        .header("User-Agent", super::USER_AGENT)
        .call()
        .map_err(|e| e.to_string())?;
    resp.into_body()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Permission keys → readable list.
pub fn describe_permissions(p: &[String]) -> String {
    if p.is_empty() {
        return "No special permissions".into();
    }
    let v: Vec<String> = p
        .iter()
        .map(|k| {
            PERMISSIONS
                .iter()
                .find(|(n, _)| n == k)
                .map_or_else(|| k.clone(), |(_, d)| d.to_string())
        })
        .collect();
    format!("Can {}", v.join(", "))
}

// ---------- UI-thread state & background jobs ----------

/// What the Extensions page shows about the catalog.
#[derive(Default)]
pub struct State {
    /// "Installed" or "Browse".
    pub tab: String,
    pub catalog: Vec<Entry>,
    pub fetched: Option<SystemTime>,
    pub fetching: bool,
    pub status: String,
    /// Package being downloaded.
    pub busy: Option<String>,
    /// Package whose permissions prompt is open.
    pub confirm: Option<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { tab: "Installed".into(), ..Default::default() });
    static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

pub fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Catalog entries newer than the installed version.
pub fn updates() -> Vec<Entry> {
    let inst = installed(&root());
    with_state(|s| {
        s.catalog
            .iter()
            .filter(|e| {
                compatible(&e.min_vivi)
                    && inst.iter().any(|i| {
                        i.manifest.id == e.id
                            && cmp_version(&e.version, &i.manifest.version) == Ordering::Greater
                    })
            })
            .cloned()
            .collect()
    })
}

fn toast(msg: &str) {
    let Some(a) = APP.with(|a| a.borrow().as_ref().and_then(|w| w.upgrade())) else {
        return;
    };
    a.set_resume_text(msg.into());
    let w = a.as_weak();
    let m = msg.to_string();
    slint::Timer::single_shot(Duration::from_secs(6), move || {
        if let Some(a) = w.upgrade() {
            if a.get_resume_text().as_str() == m {
                a.set_resume_text("".into());
            }
        }
    });
}

fn status(msg: String) {
    with_state(|s| s.status = msg);
    crate::settings::refresh_ui();
}

/// Load the cached catalog and start the daily check if it is due.
pub fn setup(app: &AppWindow, autocheck: bool) {
    APP.with(|a| *a.borrow_mut() = Some(app.as_weak()));
    let f = cache_file();
    let age = fs::metadata(&f).and_then(|m| m.modified()).ok();
    if let Ok(s) = fs::read_to_string(&f) {
        if let Ok(c) = parse_index(&s) {
            with_state(|st| {
                st.catalog = c;
                st.fetched = age;
            });
        }
    }
    let due = age
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|e| e >= CHECK_EVERY);
    if autocheck && due {
        check(false);
    }
}

/// Fetch the catalog in the background. `manual` reports the result in Settings even
/// when nothing changed; automatic checks only toast when updates are available.
pub fn check(manual: bool) {
    if with_state(|s| std::mem::replace(&mut s.fetching, true)) {
        return;
    }
    if manual {
        status("Checking for plugin updates…".into());
    }
    let url = index_url();
    std::thread::spawn(move || {
        let res = download(&url).and_then(|b| {
            let s = String::from_utf8(b).map_err(|e| e.to_string())?;
            let c = parse_index(&s)?;
            if let Err(e) = crate::store::write_atomic(&cache_file(), &s) {
                eprintln!("vivi: plugin catalog cache: {e}");
            }
            Ok(c)
        });
        let _ = slint::invoke_from_event_loop(move || {
            with_state(|s| s.fetching = false);
            match res {
                Ok(c) => {
                    let n = c.len();
                    with_state(|s| {
                        s.catalog = c;
                        s.fetched = Some(SystemTime::now());
                    });
                    let up = updates();
                    if !up.is_empty() {
                        let names: Vec<&str> = up.iter().map(|e| e.name.as_str()).collect();
                        toast(&format!(
                            "Plugin updates available: {} (Settings → Extensions)",
                            names.join(", ")
                        ));
                    }
                    status(match (manual, up.len()) {
                        (_, 0) => format!("Catalog updated: {n} plugins, everything is up to date"),
                        (_, u) => format!("{u} plugin update(s) available"),
                    });
                }
                Err(e) => {
                    eprintln!("vivi: plugin catalog {url}: {e}");
                    status(if manual {
                        format!("Could not load the plugin catalog: {e}")
                    } else {
                        String::new()
                    });
                }
            }
        });
    });
}

/// Download and install (or update) a catalog package in the background.
pub fn install(id: &str) {
    let Some(entry) = with_state(|s| s.catalog.iter().find(|e| e.id == id).cloned()) else {
        return;
    };
    if with_state(|s| s.busy.is_some()) {
        status("Another plugin is being installed".into());
        return;
    }
    if !compatible(&entry.min_vivi) {
        status(format!("{} needs Vivi {}", entry.name, entry.min_vivi));
        return;
    }
    let was = installed_one(&root(), id).map(|i| i.manifest.version);
    with_state(|s| {
        s.busy = Some(id.into());
        s.confirm = None;
    });
    status(format!("Downloading {} {}…", entry.name, entry.version));
    std::thread::spawn(move || {
        let res = download(&entry.url).and_then(|b| install_bytes(&root(), &entry, &b));
        let _ = slint::invoke_from_event_loop(move || {
            with_state(|s| s.busy = None);
            let msg = match res {
                Ok(i) => {
                    let loaded = super::rescan_package(&i.manifest.id);
                    match (was, loaded) {
                        (None, true) => {
                            format!("Installed {} {}", i.manifest.name, i.manifest.version)
                        }
                        (None, false) => format!(
                            "Installed {} {} (enable it to load)",
                            i.manifest.name, i.manifest.version
                        ),
                        (Some(_), _) => format!(
                            "Updated {} to {}. Restart Vivi to use the new version",
                            i.manifest.name, i.manifest.version
                        ),
                    }
                }
                Err(e) => format!("Could not install {}: {e}", entry.name),
            };
            toast(&msg);
            status(msg);
        });
    });
}

/// Extensions page button for package `id` (labels from `settings.rs`).
pub fn action(id: &str, label: &str) {
    let r = root();
    match label {
        "Install" | "Update" => {
            let entry = with_state(|s| s.catalog.iter().find(|e| e.id == id).cloned());
            let Some(entry) = entry else { return };
            let had: Vec<String> = installed_one(&r, id)
                .map(|i| i.manifest.permissions)
                .unwrap_or_default();
            let new_perms = entry.permissions.iter().any(|p| !had.contains(p));
            if new_perms {
                with_state(|s| s.confirm = Some(id.into()));
                status(format!(
                    "{} asks for permissions: review and allow",
                    entry.name
                ));
            } else {
                install(id);
            }
        }
        "Allow & install" => install(id),
        "Cancel" => {
            with_state(|s| s.confirm = None);
            status(String::new());
        }
        "Roll back" => status(match rollback(&r, id) {
            Ok(v) => format!("Rolled back to {v}. Restart Vivi to use it"),
            Err(e) => format!("Could not roll back: {e}"),
        }),
        "Uninstall" => {
            let res = uninstall(&r, id);
            super::rescan();
            status(match res {
                Ok(()) => "Uninstalled. A running script stops when Vivi restarts".into(),
                Err(e) => format!("Could not uninstall: {e}"),
            })
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vivi-pkg-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn pkg(id: &str, ver: &str, extra: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let man = format!(
            "id = \"{id}\"\nname = \"T\"\nversion = \"{ver}\"\nkind = \"mpv-lua\"\nentry = \"{id}.lua\"\nmin_vivi = \"0.1.0\"\npermissions = [\"network\"]\n[[settings]]\nkey = \"a\"\ntitle = \"A\"\ntype = \"toggle\"\ndefault = \"true\"\n"
        );
        w.start_file("manifest.toml", o).unwrap();
        w.write_all(man.as_bytes()).unwrap();
        w.start_file(format!("{id}.lua"), o).unwrap();
        w.write_all(b"-- hi").unwrap();
        for (n, c) in extra {
            w.start_file(*n, o).unwrap();
            w.write_all(c.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn entry(id: &str, ver: &str, data: &[u8]) -> Entry {
        Entry {
            id: id.into(),
            name: "T".into(),
            version: ver.into(),
            kind: "package".into(),
            url: "x".into(),
            sha256: sha256_hex(data),
            ..Default::default()
        }
    }

    #[test]
    fn versions_compare() {
        assert_eq!(cmp_version("1.10.0", "1.9.3"), Ordering::Greater);
        assert_eq!(cmp_version("v1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(cmp_version("1.2.0-beta", "1.2.0"), Ordering::Equal);
        assert_eq!(cmp_version("junk", "0.0.1"), Ordering::Less);
        assert!(compatible("0.8.0"));
        assert!(compatible(""));
        assert!(!compatible("99.0.0"));
    }

    #[test]
    fn install_update_rollback_uninstall() {
        let r = tmp("cycle");
        let a = pkg("demo", "1.0.0", &[("lib/util.lua", "x")]);
        let i = install_bytes(&r, &entry("demo", "1.0.0", &a), &a).unwrap();
        assert_eq!(i.manifest.version, "1.0.0");
        assert!(i.entry_path().is_file());
        assert!(i.dir.join("lib/util.lua").is_file());
        let m = i.manifest.to_manifest();
        assert_eq!(m.origin, Origin::Package);
        assert_eq!(m.settings[0].default, "true");

        let b = pkg("demo", "1.1.0", &[]);
        let i = install_bytes(&r, &entry("demo", "1.1.0", &b), &b).unwrap();
        assert_eq!(i.manifest.version, "1.1.0");
        assert_eq!(i.previous, ["1.0.0"]);
        let c = pkg("demo", "1.2.0", &[]);
        install_bytes(&r, &entry("demo", "1.2.0", &c), &c).unwrap();
        // Only the current and one previous version are kept.
        assert_eq!(versions(&r.join("demo")), ["1.2.0", "1.1.0"]);

        assert_eq!(rollback(&r, "demo").unwrap(), "1.1.0");
        assert_eq!(installed(&r)[0].manifest.version, "1.1.0");
        uninstall(&r, "demo").unwrap();
        assert!(installed(&r).is_empty());
        let _ = fs::remove_dir_all(&r);
    }

    #[test]
    fn rejects_bad_packages() {
        let r = tmp("bad");
        let a = pkg("demo", "1.0.0", &[]);
        let mut e = entry("demo", "1.0.0", &a);
        e.sha256 = "0".repeat(64);
        assert!(install_bytes(&r, &e, &a).unwrap_err().contains("checksum"));
        // Catalog/package version mismatch.
        assert!(install_bytes(&r, &entry("demo", "2.0.0", &a), &a).is_err());
        // Path traversal.
        let evil = pkg("demo", "1.0.0", &[("../evil.lua", "x")]);
        assert!(install_bytes(&r, &entry("demo", "1.0.0", &evil), &evil)
            .unwrap_err()
            .contains("unsafe"));
        assert!(!r.join("evil.lua").exists());
        assert!(installed(&r).is_empty());
        assert!(uninstall(&r, "../x").is_err());
        let _ = fs::remove_dir_all(&r);
    }

    /// Every package in `plugins/index.json` matches its `plugins/<id>/manifest.toml`.
    #[test]
    fn repo_index_packages() {
        let c = parse_index(include_str!("../../plugins/index.json")).unwrap();
        assert!(c.len() >= 3);
        for e in &c {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins")
                .join(&e.id);
            let m = read_manifest(&dir).unwrap();
            assert_eq!(m.version, e.version, "{}", e.id);
            assert_eq!(m.permissions, e.permissions, "{}", e.id);
            assert!(e.url.contains(&format!("plugin-{}-v{}", e.id, e.version)));
            assert!(dir.join(&m.entry).is_file());
            assert_eq!(Path::new(&m.entry).file_stem().unwrap(), e.id.as_str());
            assert!(compatible(&e.min_vivi));
        }
    }
}
