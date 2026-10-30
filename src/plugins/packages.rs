//! Downloadable plugin packages.
//!
//! A package is a zip with a `manifest.toml` at its root and an mpv Lua/JavaScript entry
//! script (see `docs/plugins-architecture.md`). The catalog is `plugins/index.json` of
//! <https://github.com/Loafer19/vivi-plugins>,
//! served by the plugin marketplace at [`INDEX_URL`] (GitHub Pages copy of that repository at
//! [`FALLBACK_INDEX_URL`]); package entries carry the download URL (a
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
//! [`rollback`]; older ones are pruned.
//!
//! A package may ship native helper programs (`binaries = ["vivi-discord"]` in its
//! manifest). The zip stays platform independent; the catalog entry lists one asset per
//! helper and platform ([`Binary`], keyed by [`platform`] such as `linux-x86_64`), each
//! with its own sha256. The installer downloads the ones for this platform, checks them
//! and puts them at `<id>/<version>/bin/<name>` (`.exe` on Windows, mode 0755). Network and disk work run on background threads;
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

/// Catalog published by the plugin marketplace (`plugins/index.json`).
pub const INDEX_URL: &str = "https://plugins.viviplayer.app/index.json";
/// Same catalog on GitHub Pages, tried when [`INDEX_URL`] cannot be loaded.
pub const FALLBACK_INDEX_URL: &str = "https://loafer19.github.io/vivi-plugins/index.json";
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

/// URLs to try in order: the configured catalog, then the GitHub Pages mirror when the
/// default is used.
fn index_urls() -> Vec<String> {
    let url = index_url();
    if url == INDEX_URL {
        vec![url, FALLBACK_INDEX_URL.into()]
    } else {
        vec![url]
    }
}

/// Plugin id from a marketplace deep link: `vivi://plugin/<id>` (also `vivi://install/<id>`).
pub fn deep_link(arg: &str) -> Option<String> {
    let rest = arg.strip_prefix("vivi://")?;
    let (kind, id) = rest.split_once('/')?;
    let id = id.trim_end_matches('/');
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.split('-').all(|w| {
            !w.is_empty()
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
    (matches!(kind, "plugin" | "install") && ok).then(|| id.to_string())
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
    /// Button label of an `action` setting.
    #[serde(default)]
    pub label: String,
}

fn yes() -> bool {
    true
}

/// A per-platform helper program of a package (catalog `binaries`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Binary {
    pub name: String,
    pub platform: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

/// This build's platform key: `linux-x86_64`, `windows-x86_64`, `macos-aarch64`, …
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// File name of a helper on this OS.
pub fn binary_file(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
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
    /// Helper names the package needs (from its manifest).
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub binaries: Vec<Binary>,
    /// Extra download the plugin does itself, e.g. "+ yt-dlp ~35 MB unless a system yt-dlp is used".
    #[serde(default)]
    pub size_note: String,
    /// Version history, newest first (from `plugins/<id>/store.toml`).
    #[serde(default)]
    pub changelog: Vec<Change>,
    /// Set when the plugin is replaced (built into Vivi or by another plugin).
    #[serde(default)]
    pub deprecated: Option<Deprecation>,
    /// Curated position (1 = first) in the marketplace's Featured block and at the top of
    /// Browse, where it is marked *Recommended* (`featured_rank` in `store.toml`).
    #[serde(default)]
    pub featured_rank: Option<u32>,
}

/// `deprecated` of a catalog entry (from the plugin's manifest).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Deprecation {
    /// Vivi version that replaces it ("0.13.0").
    #[serde(default)]
    pub since: String,
    #[serde(default)]
    pub message: String,
    /// What replaces it: `builtin:equalizer` / `builtin:ab-loop` / `builtin:auto-tagger`
    /// (a core feature) or `plugin:<id>`.
    #[serde(default)]
    pub replacement: String,
    /// Button that opens the replacement ("Open Equalizer").
    #[serde(default)]
    pub action: String,
}

impl Deprecation {
    /// Notice text for plugin `name`.
    pub fn text(&self, name: &str) -> String {
        if !self.message.is_empty() {
            return self.message.clone();
        }
        if self.since.is_empty() {
            format!("{name} is deprecated.")
        } else {
            format!(
                "{name} is deprecated: built into Vivi since {}.",
                self.since
            )
        }
    }

    /// One-line toast: "The Equalizer plugin is deprecated: built into Vivi 0.13.0".
    pub fn short(&self, name: &str) -> String {
        if self.replacement.starts_with("builtin:") && !self.since.is_empty() {
            format!(
                "The {name} plugin is deprecated: built into Vivi {}",
                self.since
            )
        } else {
            format!("The {name} plugin is deprecated")
        }
    }

    /// Whether this Vivi can open the replacement.
    pub fn can_open(&self) -> bool {
        matches!(
            self.replacement.as_str(),
            "builtin:equalizer" | "builtin:ab-loop" | "builtin:auto-tagger"
        ) || self.replacement.starts_with("plugin:")
    }
}

/// Open what replaces a deprecated plugin (see [`Deprecation::replacement`]), or run
/// another toast button (`Notices.toast-run`): the Auto-tagger's *Review tags* after an
/// automatic identification.
pub fn open_replacement(a: &AppWindow, replacement: &str) {
    if replacement == crate::plugins::auto_tagger::REVIEW_ACTION {
        crate::plugins::command(crate::plugins::auto_tagger::ID, "review-identified", &[]);
    } else if replacement == "builtin:auto-tagger" {
        // AcoustID package: the Auto-tagger's settings (fingerprinting, auto-identify).
        crate::extwin::open(a, crate::plugins::auto_tagger::ID, true);
    } else if replacement == "builtin:equalizer" {
        crate::fx::open_panel(a);
    } else if replacement == "builtin:ab-loop" {
        // The built-in A-B loop is keys plus the seek-bar markers: show the shortcuts.
        a.global::<crate::Overlays>().set_shortcuts_open(true);
    } else if let Some(id) = replacement.strip_prefix("plugin:") {
        crate::extwin::open(a, id, false);
    }
}

/// One `changelog` item of a catalog entry.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Change {
    pub version: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub notes: String,
}

impl Entry {
    /// "What's new" since `installed`: notes of every newer version, newest first.
    pub fn whats_new(&self, installed: &str) -> String {
        self.changelog
            .iter()
            .filter(|c| cmp_version(&c.version, installed).is_gt())
            .map(|c| format!("{}: {}", c.version, c.notes))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

impl Entry {
    /// Helper assets for `plat`, or the name of the first missing one.
    pub fn binaries_for(&self, plat: &str) -> Result<Vec<Binary>, String> {
        self.needs
            .iter()
            .map(|n| {
                self.binaries
                    .iter()
                    .find(|b| &b.name == n && b.platform == plat && b.sha256.len() == 64)
                    .cloned()
                    .ok_or_else(|| format!("no {n} helper for {plat} yet"))
            })
            .collect()
    }
    /// Whether it can be installed on this platform.
    pub fn available(&self) -> bool {
        self.binaries_for(&platform()).is_ok()
    }
    /// Download size on this platform: zip plus helpers.
    pub fn download_size(&self) -> u64 {
        self.size
            + self
                .binaries_for(&platform())
                .map(|v| v.iter().map(|b| b.size).sum())
                .unwrap_or(0)
    }
}

/// "12 KB" / "1.4 MB".
pub fn human_size(n: u64) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else {
        format!("{:.0} KB", (n as f64 / 1024.0).ceil())
    }
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
    /// Native helpers shipped per platform (see the module docs).
    #[serde(default)]
    pub binaries: Vec<String>,
    /// `source` | `feature` (see `plugins::contrib`); missing = known default / feature.
    #[serde(default)]
    pub role: Option<String>,
    /// `[[contributes]]` entries (point, label, command, icon).
    #[serde(default)]
    pub contributes: Option<Vec<super::contrib::Contribution>>,
    /// `secret` settings reach the package through a private file instead of mpv's
    /// `script-opts`, which mpv writes to its log (Vivi 0.13+; see
    /// docs/plugins-architecture.md, "Secret settings").
    #[serde(default)]
    pub secrets_file: bool,
}

/// Setting keys Vivi passes to every package itself (`<id>-pkgdir`, `<id>-secrets`).
pub const RESERVED_KEYS: [&str; 2] = ["pkgdir", "secrets"];

fn safe_name(n: &str) -> bool {
    !n.is_empty()
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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
        if let Some(b) = self.binaries.iter().find(|b| !safe_name(b)) {
            return Err(format!("invalid helper name {b:?}"));
        }
        if let Some(s) = self
            .settings
            .iter()
            .find(|s| RESERVED_KEYS.contains(&s.key.as_str()))
        {
            return Err(format!("setting key {:?} is reserved", s.key));
        }
        Ok(())
    }

    /// Plugin manifest for the registry / Settings page.
    pub fn to_manifest(&self) -> Manifest {
        use super::contrib;
        // Manifests without the fields get the host's defaults for that id.
        let (def_role, def_contrib) = contrib::known_defaults(&self.id);
        let role = self
            .role
            .as_deref()
            .map(contrib::Role::parse)
            .unwrap_or(def_role);
        let contributes = contrib::known(self.contributes.clone().unwrap_or(def_contrib));
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
            role,
            contributes,
        }
    }
}

fn to_spec(s: &IndexSetting) -> Option<SettingSpec> {
    let kind = match s.kind.as_str() {
        "toggle" => SettingKind::Toggle,
        "choice" if !s.options.is_empty() => SettingKind::Choice(s.options.clone()),
        "text" => SettingKind::Text(String::new()),
        "secret" => SettingKind::Secret,
        "action" => SettingKind::Action(if s.label.is_empty() {
            "Run".into()
        } else {
            s.label.clone()
        }),
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
#[cfg(test)]
pub fn install_bytes(root: &Path, entry: &Entry, data: &[u8]) -> Result<Installed, String> {
    install_with(root, entry, data, &[])
}

/// Like [`install_bytes`], plus downloaded helpers (`(asset, bytes)`, already for this
/// platform). Every helper the manifest names must be there and match its sha256.
pub fn install_with(
    root: &Path,
    entry: &Entry,
    data: &[u8],
    bins: &[(Binary, Vec<u8>)],
) -> Result<Installed, String> {
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
        for name in &m.binaries {
            let (b, bytes) = bins
                .iter()
                .find(|(b, _)| &b.name == name)
                .ok_or_else(|| format!("helper {name} missing"))?;
            if !sha256_hex(bytes).eq_ignore_ascii_case(&b.sha256) {
                return Err(format!("checksum mismatch for helper {name}"));
            }
            write_binary(&staging.join("bin").join(binary_file(name)), bytes)?;
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

fn write_binary(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    fs::write(path, bytes).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(())
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

/// Uninstall package `id` (Extensions → Installed, and the plugin's own page in the
/// Extensions window): remove its files and, unless `keep_data`, its settings and its
/// data folder. A running script keeps running until Vivi restarts.
pub fn remove(id: &str, keep_data: bool) -> Result<(), String> {
    let res = uninstall(&root(), id).and_then(|()| {
        if keep_data {
            return Ok(());
        }
        super::forget(id);
        let d = super::host::data_dir(id);
        if d.exists() {
            fs::remove_dir_all(&d).map_err(|e| e.to_string())?;
        }
        Ok(())
    });
    super::rescan();
    res
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
    /// Package to enable once installed ([`install_and_enable`]).
    pub enable_after: Option<String>,
    /// Package to offer for install once the catalog has been downloaded.
    pub install_after_fetch: Option<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { tab: "Installed".into(), ..Default::default() });
    static APP: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
}

pub fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Catalog deprecation of plugin `id`, if any.
pub fn deprecation(id: &str) -> Option<Deprecation> {
    with_state(|s| {
        s.catalog
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.deprecated.clone())
    })
}

/// Installed packages the catalog marks deprecated: (id, name, deprecation).
pub fn deprecated_installed() -> Vec<(String, String, Deprecation)> {
    let inst = installed(&root());
    with_state(|s| {
        s.catalog
            .iter()
            .filter(|e| inst.iter().any(|i| i.manifest.id == e.id))
            .filter_map(|e| Some((e.id.clone(), e.name.clone(), e.deprecated.clone()?)))
            .collect()
    })
}

fn notices_file() -> PathBuf {
    crate::config::path().with_file_name("deprecation-notices.txt")
}

/// Ids in `seen` (one per line) not yet noticed, among `ids`.
fn unseen<'a>(seen: &str, ids: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    ids.into_iter()
        .filter(|id| !seen.lines().any(|l| l.trim() == *id))
        .collect()
}

thread_local! { static NOTICE_DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// One-time toast for an installed plugin the catalog marks deprecated, with a button
/// that opens its replacement. Called when a file loads (the toast shows over media).
/// It shares the notice line with the resume prompt, which a file load sets (or clears)
/// right after: the toast waits until that line is free, else it would be wiped at once
/// and still be counted as seen.
pub fn deprecation_notice() {
    if NOTICE_DONE.with(|d| d.replace(true)) {
        return;
    }
    slint::Timer::single_shot(Duration::from_millis(1500), || show_deprecation_notice(8));
}

fn show_deprecation_notice(tries: u32) {
    let Some(a) = APP.with(|a| a.borrow().as_ref().and_then(|w| w.upgrade())) else {
        return;
    };
    if !a.global::<crate::Notices>().get_resume_text().is_empty() && tries > 0 {
        slint::Timer::single_shot(Duration::from_secs(2), move || {
            show_deprecation_notice(tries - 1)
        });
        return;
    }
    let dep = deprecated_installed();
    let seen = fs::read_to_string(notices_file()).unwrap_or_default();
    let Some(id) = unseen(&seen, dep.iter().map(|(id, _, _)| id.as_str()))
        .first()
        .map(|s| s.to_string())
    else {
        return;
    };
    let Some((_, name, d)) = dep.into_iter().find(|(i, _, _)| *i == id) else {
        return;
    };
    let mut lines = seen;
    lines.push_str(&id);
    lines.push('\n');
    if let Err(e) = crate::store::write_atomic(&notices_file(), &lines) {
        eprintln!("vivi: save deprecation notices: {e}");
    }
    let msg = d.short(&name);
    let n = a.global::<crate::Notices>();
    n.set_resume_text(msg.as_str().into());
    if d.can_open() {
        let label = if d.action.is_empty() {
            "Open"
        } else {
            d.action.as_str()
        };
        n.set_toast_action(label.into());
        n.set_toast_action_id(d.replacement.as_str().into());
        n.set_toast_action_text(msg.as_str().into());
    }
    let w = a.as_weak();
    slint::Timer::single_shot(Duration::from_secs(15), move || {
        if let Some(a) = w.upgrade() {
            if a.global::<crate::Notices>().get_resume_text().as_str() == msg {
                a.global::<crate::Notices>().set_resume_text("".into());
            }
        }
    });
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

pub fn toast(msg: &str) {
    let Some(a) = APP.with(|a| a.borrow().as_ref().and_then(|w| w.upgrade())) else {
        return;
    };
    a.global::<crate::Notices>().set_resume_text(msg.into());
    let w = a.as_weak();
    let m = msg.to_string();
    slint::Timer::single_shot(Duration::from_secs(6), move || {
        if let Some(a) = w.upgrade() {
            if a.global::<crate::Notices>().get_resume_text().as_str() == m {
                a.global::<crate::Notices>().set_resume_text("".into());
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
    let w = app.as_weak();
    app.global::<crate::Notices>().on_toast_run(move |id| {
        if let Some(a) = w.upgrade() {
            a.global::<crate::Notices>().set_resume_text("".into());
            open_replacement(&a, &id);
        }
    });
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
    let urls = index_urls();
    let url = urls[0].clone();
    std::thread::spawn(move || {
        let fetch = |u: &str| {
            download(u).and_then(|b| {
                let s = String::from_utf8(b).map_err(|e| e.to_string())?;
                let c = parse_index(&s)?;
                Ok((s, c))
            })
        };
        let mut res = fetch(&urls[0]);
        for u in &urls[1..] {
            if let Err(e) = &res {
                eprintln!("vivi: plugin catalog {}: {e}; trying {u}", urls[0]);
                res = fetch(u);
            }
        }
        let res = res.map(|(s, c)| {
            if let Err(e) = crate::store::write_atomic(&cache_file(), &s) {
                eprintln!("vivi: plugin catalog cache: {e}");
            }
            c
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
                    if let Some(id) = with_state(|s| s.install_after_fetch.take()) {
                        if with_state(|s| s.catalog.iter().any(|e| e.id == id)) {
                            action(&id, "Install");
                        } else {
                            with_state(|s| s.enable_after = None);
                            status(format!("{id} is not in the plugin catalog"));
                        }
                    }
                }
                Err(e) => {
                    with_state(|s| {
                        s.install_after_fetch = None;
                        s.enable_after = None;
                    });
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
    let bins = match entry.binaries_for(&platform()) {
        Ok(b) => b,
        Err(e) => {
            status(format!("{}: {e}", entry.name));
            return;
        }
    };
    let was = installed_one(&root(), id).map(|i| i.manifest.version);
    with_state(|s| {
        s.busy = Some(id.into());
        s.confirm = None;
    });
    status(format!("Downloading {} {}…", entry.name, entry.version));
    std::thread::spawn(move || {
        let res = download(&entry.url).and_then(|b| {
            let got = bins
                .into_iter()
                .map(|a| download(&a.url).map(|d| (a, d)))
                .collect::<Result<Vec<_>, _>>()?;
            install_with(&root(), &entry, &b, &got)
        });
        let _ = slint::invoke_from_event_loop(move || {
            let enable = with_state(|s| {
                s.busy = None;
                let e = s.enable_after.as_deref() == Some(entry.id.as_str());
                if e {
                    s.enable_after = None;
                }
                e
            });
            let msg = match res {
                Ok(i) => {
                    let mut loaded = super::rescan_package(&i.manifest.id);
                    // Installed from a suggestion (seek-bar hint): turn it on as well.
                    if enable && !loaded {
                        super::setting_changed(&super::toggle_id(&i.manifest.id), "true");
                        loaded = super::rescan_package(&i.manifest.id);
                    }
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

/// Plugins Browse marks "Recommended", in order, when the catalog has no `featured_rank`
/// (catalogs from before the field). Keep in sync with `featured_rank` in vivi-plugins.
pub const RECOMMENDED: &[&str] = &[
    "chapter-thumbnails",
    "youtube",
    "sponsorblock",
    "night-mode",
    "subtitle-sync",
    "remote-control",
];

/// Recommended position of `e` (1 = first) or `None`. The catalog's `featured_rank` decides
/// when any entry has one; otherwise [`RECOMMENDED`]. Deprecated plugins never are.
pub fn recommended_rank(catalog: &[Entry], e: &Entry) -> Option<u32> {
    if e.deprecated.is_some() {
        return None;
    }
    if catalog.iter().any(|c| c.featured_rank.is_some()) {
        return e.featured_rank;
    }
    RECOMMENDED
        .iter()
        .position(|id| *id == e.id)
        .map(|i| i as u32 + 1)
}

/// Browse order: recommended plugins first by rank, then the rest in catalog order.
pub fn browse_order(catalog: &[Entry]) -> Vec<&Entry> {
    let mut v: Vec<&Entry> = catalog.iter().collect();
    v.sort_by_key(|e| recommended_rank(catalog, e).unwrap_or(u32::MAX));
    v
}

/// Install package `id` on a suggestion outside the Extensions window (the seek-bar hint)
/// with the usual flow: the window opens on All / Marketplace → Browse, where the
/// permission prompt asks first; the package is enabled once installed. Downloads the
/// catalog first when it does not list `id` yet.
pub fn install_and_enable(a: &AppWindow, id: &str) {
    with_state(|s| {
        s.tab = "Browse".into();
        s.enable_after = Some(id.into());
    });
    crate::extwin::open(a, "", false);
    if with_state(|s| s.catalog.iter().any(|e| e.id == id)) {
        action(id, "Install");
    } else {
        with_state(|s| s.install_after_fetch = Some(id.into()));
        check(true);
    }
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
            with_state(|s| {
                s.confirm = None;
                if s.enable_after.as_deref() == Some(id) {
                    s.enable_after = None;
                }
            });
            status(String::new());
        }
        "Roll back" => {
            let res = rollback(&r, id);
            super::rescan();
            status(match res {
                Ok(v) => format!("Rolled back to {v}. Restart Vivi to use it"),
                Err(e) => format!("Could not roll back: {e}"),
            })
        }
        "Uninstall" => {
            let res = remove(id, true);
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

    fn ranked(id: &str, rank: Option<u32>) -> Entry {
        Entry {
            id: id.into(),
            featured_rank: rank,
            ..Default::default()
        }
    }

    #[test]
    fn recommended_from_catalog_rank() {
        let sha = "a".repeat(64);
        let idx = format!(
            r#"{{"schema": 1, "plugins": [
            {{"id": "a", "name": "A", "version": "1.0.0", "kind": "package",
              "url": "https://x/a.zip", "sha256": "{sha}"}},
            {{"id": "b", "name": "B", "version": "1.0.0", "kind": "package",
              "url": "https://x/b.zip", "sha256": "{sha}", "featured_rank": 2}},
            {{"id": "youtube", "name": "Y", "version": "1.0.0", "kind": "package",
              "url": "https://x/y.zip", "sha256": "{sha}"}},
            {{"id": "c", "name": "C", "version": "1.0.0", "kind": "package",
              "url": "https://x/c.zip", "sha256": "{sha}", "featured_rank": 1}}]}}"#
        );
        let c = parse_index(&idx).unwrap();
        assert_eq!(c[1].featured_rank, Some(2));
        assert_eq!(c[0].featured_rank, None);
        // The catalog has ranks: the built-in list is ignored (youtube is not recommended).
        assert_eq!(recommended_rank(&c, &c[2]), None);
        let ids: Vec<&str> = browse_order(&c).iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["c", "b", "a", "youtube"]);
    }

    #[test]
    fn recommended_fallback_and_deprecated() {
        // No featured_rank anywhere (older catalog): RECOMMENDED order, rest keeps its order.
        let mut c = vec![
            ranked("ab-loop", None),
            ranked("remote-control", None),
            ranked("scrobbler", None),
            ranked("chapter-thumbnails", None),
            ranked("youtube", None),
        ];
        let ids: Vec<&str> = browse_order(&c).iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "chapter-thumbnails",
                "youtube",
                "remote-control",
                "ab-loop",
                "scrobbler"
            ]
        );
        assert_eq!(recommended_rank(&c, &c[2]), None);
        assert_eq!(recommended_rank(&c, &c[3]), Some(1));
        // Deprecated plugins are never recommended, with or without a rank.
        c[3].deprecated = Some(Deprecation::default());
        assert_eq!(recommended_rank(&c, &c[3]), None);
        c[4].featured_rank = Some(1);
        c[4].deprecated = Some(Deprecation::default());
        assert_eq!(recommended_rank(&c, &c[4]), None);
    }

    #[test]
    fn deprecated_entries() {
        let sha = "a".repeat(64);
        let idx = format!(
            r#"{{"schema": 1, "plugins": [
            {{"id": "equalizer", "name": "Equalizer", "version": "1.0.0", "kind": "package",
              "url": "https://x/e.zip", "sha256": "{sha}",
              "deprecated": {{"since": "0.13.0", "replacement": "builtin:equalizer",
                              "action": "Open Equalizer"}}}},
            {{"id": "b", "name": "B", "version": "1.0.0", "kind": "package",
              "url": "https://x/b.zip", "sha256": "{sha}"}}]}}"#
        );
        let c = parse_index(&idx).unwrap();
        let d = c[0].deprecated.clone().unwrap();
        assert_eq!(
            d.text("Equalizer"),
            "Equalizer is deprecated: built into Vivi since 0.13.0."
        );
        assert!(d.can_open());
        assert_eq!(
            d.short("Equalizer"),
            "The Equalizer plugin is deprecated: built into Vivi 0.13.0"
        );
        assert_eq!(c[1].deprecated, None);
        let custom = Deprecation {
            message: "Use X.".into(),
            ..Default::default()
        };
        assert_eq!(custom.text("B"), "Use X.");
        assert!(!custom.can_open());
        let ab = Deprecation {
            since: "0.13.0".into(),
            replacement: "builtin:ab-loop".into(),
            ..Default::default()
        };
        assert!(ab.can_open());
        assert_eq!(
            ab.short("A-B Loop & Frame Step"),
            "The A-B Loop & Frame Step plugin is deprecated: built into Vivi 0.13.0"
        );
        assert_eq!(unseen("equalizer\n", ["equalizer", "b"]), ["b"]);
        assert!(unseen(" equalizer \nb\n", ["equalizer", "b"]).is_empty());
    }

    #[test]
    fn manifest_contributions_and_defaults() {
        use crate::plugins::contrib::Role;
        let base = "id = \"x\"\nname = \"X\"\nversion = \"1.0.0\"\nkind = \"mpv-lua\"\nentry = \"x.lua\"\n";
        let m: PkgManifest = toml::from_str(base).unwrap();
        let m = m.to_manifest();
        assert_eq!(m.role, Role::Feature);
        assert!(m.contributes.is_empty());
        let t = format!("{base}role = \"source\"\n[[contributes]]\npoint = \"sidebar.source\"\n[[contributes]]\npoint = \"menu.subtitles\"\nlabel = \"Sync\"\ncommand = \"sync\"\n[[contributes]]\npoint = \"future.point\"\n");
        let m: PkgManifest = toml::from_str(&t).unwrap();
        let m = m.to_manifest();
        assert_eq!(m.role, Role::Source);
        assert_eq!(m.contributes.len(), 2);
        assert_eq!(m.contributes[1].command, "sync");
        // Older catalog packages get the host defaults by id; `contributes = []` opts out.
        let yt = base.replace("\"x\"", "\"youtube\"");
        let m: PkgManifest = toml::from_str(&yt).unwrap();
        assert_eq!(m.to_manifest().role, Role::Source);
        let m: PkgManifest = toml::from_str(&format!("{yt}contributes = []\n")).unwrap();
        assert!(m.to_manifest().contributes.is_empty());
    }

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

    #[test]
    fn installs_platform_helpers() {
        let r = tmp("bins");
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default();
        w.start_file("manifest.toml", o).unwrap();
        w.write_all(b"id = \"helped\"\nname = \"H\"\nversion = \"1.0.0\"\nkind = \"mpv-lua\"\nentry = \"helped.lua\"\nbinaries = [\"vivi-x\"]\n").unwrap();
        w.start_file("helped.lua", o).unwrap();
        w.write_all(b"--").unwrap();
        let z = w.finish().unwrap().into_inner();
        let helper = b"#!/bin/sh\necho hi\n".to_vec();
        let mut e = entry("helped", "1.0.0", &z);
        e.needs = vec!["vivi-x".into()];
        e.size = z.len() as u64;
        assert!(e.binaries_for(&platform()).is_err());
        assert!(!e.available());
        let b = Binary {
            name: "vivi-x".into(),
            platform: platform(),
            url: "u".into(),
            sha256: sha256_hex(&helper),
            size: helper.len() as u64,
        };
        e.binaries = vec![b.clone()];
        assert!(e.available());
        assert_eq!(e.download_size(), z.len() as u64 + helper.len() as u64);
        // Missing helper and bad helper checksum are refused.
        assert!(install_with(&r, &e, &z, &[])
            .unwrap_err()
            .contains("missing"));
        assert!(install_with(&r, &e, &z, &[(b.clone(), b"evil".to_vec())])
            .unwrap_err()
            .contains("checksum"));
        let i = install_with(&r, &e, &z, &[(b, helper.clone())]).unwrap();
        let p = i.dir.join("bin").join(binary_file("vivi-x"));
        assert_eq!(fs::read(&p).unwrap(), helper);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        assert_eq!(human_size(1536 * 1024), "1.5 MB");
        let _ = fs::remove_dir_all(&r);
    }

    /// Every package in the catalog matches its `plugins/<id>/manifest.toml`. Needs a checkout
    /// of https://github.com/Loafer19/vivi-plugins in `$VIVI_PLUGINS_DIR`; skipped without it.
    #[test]
    fn repo_index_packages() {
        let Some(root) = std::env::var_os("VIVI_PLUGINS_DIR").map(std::path::PathBuf::from) else {
            eprintln!("VIVI_PLUGINS_DIR not set, skipping");
            return;
        };
        let c = parse_index(&fs::read_to_string(root.join("plugins/index.json")).unwrap()).unwrap();
        assert!(c.len() >= 3);
        for e in &c {
            let dir = root.join("plugins").join(&e.id);
            let m = read_manifest(&dir).unwrap();
            assert_eq!(m.version, e.version, "{}", e.id);
            assert_eq!(m.permissions, e.permissions, "{}", e.id);
            assert!(e.url.contains(&format!("plugin-{}-v{}", e.id, e.version)));
            assert!(dir.join(&m.entry).is_file());
            assert_eq!(Path::new(&m.entry).file_stem().unwrap(), e.id.as_str());
            // At most the next minor release: packages may need core support that is on
            // master but not released yet (e.g. 0.13.0 while the crate is still 0.12.x).
            let (a, b, _) = parse_version(env!("CARGO_PKG_VERSION")).unwrap();
            let next = format!("{a}.{}.0", b + 1);
            assert!(cmp_version(&e.min_vivi, &next).is_le(), "{}", e.id);
            // A core replacement must be one this Vivi can open.
            if let Some(d) = &e.deprecated {
                if d.replacement.starts_with("builtin:") {
                    assert!(d.can_open(), "{}: {}", e.id, d.replacement);
                }
            }
            assert_eq!(m.binaries, e.needs, "{}", e.id);
        }
    }
}

#[cfg(test)]
mod deep_link_tests {
    use super::deep_link;

    #[test]
    fn parses_plugin_links() {
        assert_eq!(deep_link("vivi://plugin/ab-loop"), Some("ab-loop".into()));
        assert_eq!(deep_link("vivi://install/youtube/"), Some("youtube".into()));
        assert_eq!(deep_link("vivi://plugin/../etc"), None);
        assert_eq!(deep_link("vivi://plugin/"), None);
        assert_eq!(deep_link("vivi://status"), None);
        assert_eq!(deep_link("https://example.com/a"), None);
    }

    #[test]
    fn whats_new_lists_newer_versions() {
        let e: super::Entry = serde_json::from_str(
            r#"{"id":"x","name":"X","version":"1.2.0","kind":"package","changelog":[
            {"version":"1.2.0","notes":"B"},{"version":"1.1.0","notes":"A"},{"version":"1.0.0","notes":"first"}]}"#,
        )
        .unwrap();
        assert_eq!(e.whats_new("1.0.0"), "1.2.0: B · 1.1.0: A");
        assert_eq!(e.whats_new("1.2.0"), "");
    }
}
