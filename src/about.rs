//! Settings → About: version and build info, the GitHub release update check,
//! install-type detection for the Download button, and the bug-report system info.

use crate::config::Config;
use crate::player::Player;
use crate::{About, AppWindow};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    /// What the Download button opens: the matching asset or the release page.
    static TARGET: RefCell<String> = const { RefCell::new(String::new()) };
}

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const REPO: &str = "https://github.com/Loafer19/vivi";
const LATEST_API: &str = "https://api.github.com/repos/Loafer19/vivi/releases/latest";

/// `"v1.2.3-rc.1"` → `([1, 2, 3], Some("rc.1"))`; missing parts are 0.
fn parse(v: &str) -> Option<(Vec<u64>, Option<String>)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split('+').next()?;
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p.to_string())),
        None => (v, None),
    };
    let mut nums = core
        .split('.')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if nums.is_empty() {
        return None;
    }
    nums.resize(3.max(nums.len()), 0);
    Some((nums, pre))
}

/// True when `latest` is a newer version than `current` (semver order; a
/// pre-release sorts before its release). Unparsable tags are never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some((a, ap)), Some((b, bp))) = (parse(latest), parse(current)) else {
        return false;
    };
    let n = a.len().max(b.len());
    let get = |v: &Vec<u64>, i: usize| v.get(i).copied().unwrap_or(0);
    for i in 0..n {
        match get(&a, i).cmp(&get(&b, i)) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            _ => {}
        }
    }
    match (ap, bp) {
        (None, Some(_)) => true,
        (Some(x), Some(y)) => pre_newer(&x, &y),
        _ => false,
    }
}

fn pre_newer(a: &str, b: &str) -> bool {
    let (pa, pb): (Vec<_>, Vec<_>) = (a.split('.').collect(), b.split('.').collect());
    for (x, y) in pa.iter().zip(&pb) {
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(m), Ok(n)) => m.cmp(&n),
            (Ok(_), Err(_)) => std::cmp::Ordering::Less,
            (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
            _ => x.cmp(y),
        };
        if o != std::cmp::Ordering::Equal {
            return o == std::cmp::Ordering::Greater;
        }
    }
    pa.len() > pb.len()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Install {
    AppImage,
    Flatpak,
    Pacman,
    Tarball,
    Windows,
    MacOs,
}

impl Install {
    pub fn name(self) -> &'static str {
        match self {
            Install::AppImage => "AppImage",
            Install::Flatpak => "Flatpak",
            Install::Pacman => "pacman package",
            Install::Tarball => "tarball",
            Install::Windows => "Windows",
            Install::MacOs => "macOS",
        }
    }
}

pub fn detect_install() -> Install {
    if cfg!(target_os = "windows") {
        return Install::Windows;
    }
    if cfg!(target_os = "macos") {
        return Install::MacOs;
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return Install::AppImage;
    }
    if std::env::var_os("FLATPAK_ID").is_some() || std::path::Path::new("/.flatpak-info").exists() {
        return Install::Flatpak;
    }
    let exe = std::env::current_exe().unwrap_or_default();
    if exe.starts_with("/usr") && std::path::Path::new("/var/lib/pacman/local").exists() {
        return Install::Pacman;
    }
    Install::Tarball
}

/// Release asset for this platform and install type, if one fits. Release
/// assets: `vivi-vX-linux-x86_64.tar.gz`, `Vivi-vX-x86_64.AppImage`,
/// `vivi-vX-x86_64.flatpak`, `vivi-vX-windows-x86_64.zip`, `vivi-vX-macos-arm64.zip`.
/// pacman installs update through the package manager, so they get the page.
pub fn pick_asset(install: Install, arch: &str, assets: &[(String, String)]) -> Option<String> {
    // (substring the name must contain, extension)
    let want: &[(&str, &str)] = match install {
        Install::AppImage => &[("", ".appimage")],
        Install::Flatpak => &[("", ".flatpak")],
        Install::Tarball => &[
            ("-linux-", ".tar.gz"),
            ("-linux-", ".tar.xz"),
            ("-linux-", ".tar.zst"),
        ],
        Install::Windows => &[("-windows-", ".zip")],
        Install::MacOs => &[("-macos-", ".zip")],
        Install::Pacman => return None,
    };
    let arch_ok = |n: &str| {
        let alt = if arch == "x86_64" { "amd64" } else { "arm64" };
        n.contains(arch)
            || n.contains(alt)
            || !(n.contains("x86_64")
                || n.contains("aarch64")
                || n.contains("amd64")
                || n.contains("arm64"))
    };
    for (part, ext) in want {
        if let Some((_, u)) = assets.iter().find(|(n, _)| {
            let n = n.to_lowercase();
            n.contains(part) && n.ends_with(ext) && arch_ok(&n) && !n.contains("source")
        }) {
            return Some(u.clone());
        }
    }
    None
}

pub struct Build {
    pub mpv: String,
}

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

fn build_rows(b: &Build) -> Vec<(&'static str, String)> {
    vec![
        ("Version", VERSION.to_string()),
        ("Commit", env!("VIVI_GIT_HASH").to_string()),
        ("Build date", env!("VIVI_BUILD_DATE").to_string()),
        (
            "Target",
            format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        ("libmpv", b.mpv.clone()),
        ("Slint", env!("VIVI_SLINT_VERSION").to_string()),
        ("Install type", detect_install().name().to_string()),
    ]
}

pub fn system_info(b: &Build) -> String {
    let mut s = String::from("vivi system info\n");
    for (k, v) in build_rows(b) {
        s += &format!("{k}: {v}\n");
    }
    s += &format!("OS: {}\n", os_name());
    if let Ok(d) = std::env::var("XDG_SESSION_TYPE") {
        s += &format!("Session: {d}\n");
    }
    s
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn bug_url(b: &Build) -> String {
    let body = format!(
        "**What happened**\n\n\n**Steps to reproduce**\n1. \n\n**Expected**\n\n\n<details><summary>System info</summary>\n\n```\n{}```\n</details>\n",
        system_info(b)
    );
    format!(
        "{REPO}/issues/new?title={}&body={}",
        enc(&format!("[v{VERSION}] ")),
        enc(&body)
    )
}

struct Release {
    tag: String,
    page: String,
    notes: String,
    assets: Vec<(String, String)>,
}

fn fetch_latest() -> Result<Release, String> {
    let mut r = crate::plugins::web::agent_with_timeout(15)
        .get(LATEST_API)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let st = r.status().as_u16();
    if st == 404 {
        return Err("No releases published yet.".into());
    }
    if st != 200 {
        return Err(format!("GitHub answered HTTP {st}."));
    }
    let j: serde_json::Value = r.body_mut().read_json().map_err(|e| e.to_string())?;
    let s = |k: &str| j[k].as_str().unwrap_or_default().to_string();
    let assets = j["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    (
                        x["name"].as_str().unwrap_or_default().to_string(),
                        x["browser_download_url"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Release {
        tag: s("tag_name"),
        page: s("html_url"),
        notes: s("body"),
        assets,
    })
}

/// First few non-empty lines of the release notes, markdown markers stripped.
pub fn excerpt(notes: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for l in notes.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let l = l.trim_start_matches('#').trim().replace("**", "");
        out.push(if l.chars().count() > 160 {
            format!("{}…", l.chars().take(160).collect::<String>())
        } else {
            l
        });
        if out.len() == 6 {
            out.push("…".into());
            break;
        }
    }
    out.join("\n")
}

pub fn setup(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &Rc<RefCell<Config>>,
    save: impl Fn() + 'static,
) {
    let mpv = player
        .get("mpv-version")
        .map(|v| v.trim_start_matches("mpv ").to_string())
        .unwrap_or_else(|| "unknown".into());
    let build = Rc::new(Build { mpv });
    let g = app.global::<About>();
    g.set_version(VERSION.into());
    let rows = build_rows(&build);
    let s = |v: Vec<String>| {
        ModelRc::new(VecModel::from(
            v.into_iter().map(SharedString::from).collect::<Vec<_>>(),
        ))
    };
    g.set_build_keys(s(rows.iter().map(|r| r.0.to_string()).collect()));
    g.set_build_values(s(rows.iter().map(|r| r.1.clone()).collect()));
    g.set_license("MIT License".into());
    g.set_sysinfo(system_info(&build).into());
    g.set_auto_check(cfg.borrow().general.check_updates);
    g.set_message(format!("Installed: v{VERSION} ({})", detect_install().name()).into());

    TARGET.with(|t| *t.borrow_mut() = format!("{REPO}/releases/latest"));
    let check = {
        let w = app.as_weak();
        move |quiet: bool| {
            let Some(a) = w.upgrade() else { return };
            if !quiet {
                a.global::<About>().set_state("checking".into());
            }
            let w = a.as_weak();
            std::thread::spawn(move || {
                let res = fetch_latest();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(a) = w.upgrade() else { return };
                    let g = a.global::<About>();
                    match res {
                        Ok(r) if is_newer(&r.tag, VERSION) => {
                            let install = detect_install();
                            let asset = pick_asset(install, std::env::consts::ARCH, &r.assets);
                            g.set_download_label(
                                if asset.is_some() {
                                    format!("Download {}", install.name())
                                } else {
                                    "Open release page".into()
                                }
                                .into(),
                            );
                            TARGET.with(|t| *t.borrow_mut() = asset.unwrap_or(r.page));
                            g.set_state("available".into());
                            g.set_latest(r.tag.trim_start_matches('v').into());
                            g.set_notes(excerpt(&r.notes).into());
                            g.set_message(format!("You have v{VERSION}.").into());
                        }
                        Ok(r) => {
                            if quiet {
                                return;
                            }
                            g.set_state("current".into());
                            g.set_message(
                                format!("v{VERSION} is the latest release ({}).", r.tag).into(),
                            );
                        }
                        Err(e) => {
                            if quiet {
                                return;
                            }
                            g.set_state("error".into());
                            g.set_message(e.into());
                        }
                    }
                });
            });
        }
    };
    let check = Rc::new(check);
    {
        let check = check.clone();
        g.on_check(move || check(false));
    }
    {
        g.on_download(|| TARGET.with(|t| crate::settings::open_url(&t.borrow())));
    }
    {
        let build = build.clone();
        g.on_link(move |u| match u.as_str() {
            "bug" => crate::settings::open_url(&bug_url(&build)),
            "changelog" => crate::settings::open_url(&format!("{REPO}/blob/master/CHANGELOG.md")),
            u => crate::settings::open_url(u),
        });
    }
    {
        let cfg = cfg.clone();
        g.on_auto_toggled(move |on| {
            cfg.borrow_mut().general.check_updates = on;
            save();
        });
    }
    if cfg.borrow().general.check_updates {
        check(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert!(is_newer("v0.12.0", "0.11.0"));
        assert!(is_newer("0.11.1", "0.11.0"));
        assert!(is_newer("1.0", "0.99.9"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.11.0", "0.11.0"));
        assert!(!is_newer("v0.11", "0.11.0"));
        assert!(!is_newer("0.10.9", "0.11.0"));
        assert!(!is_newer("garbage", "0.11.0"));
        assert!(!is_newer("", "0.11.0"));
    }

    #[test]
    fn prerelease_order() {
        assert!(is_newer("0.12.0", "0.12.0-rc.1"));
        assert!(!is_newer("0.12.0-rc.1", "0.12.0"));
        assert!(is_newer("0.12.0-rc.2", "0.12.0-rc.1"));
        assert!(is_newer("0.12.0-rc.10", "0.12.0-rc.9"));
        assert!(is_newer("0.12.0-beta", "0.12.0-alpha"));
        assert!(is_newer("0.12.0-rc.1", "0.11.0"));
        assert!(!is_newer("0.11.0+build5", "0.11.0"));
    }

    #[test]
    fn assets() {
        let a: Vec<(String, String)> = [
            ("SHA256SUMS", "S"),
            ("PKGBUILD", "P"),
            ("Vivi-v0.12.0-x86_64.AppImage", "A"),
            ("vivi-v0.12.0-linux-x86_64.tar.gz", "T"),
            ("vivi-v0.12.0-x86_64.flatpak", "F"),
            ("vivi-v0.12.0-windows-x86_64.zip", "W"),
            ("vivi-v0.12.0-macos-arm64.zip", "M"),
        ]
        .iter()
        .map(|(n, u)| (n.to_string(), u.to_string()))
        .collect();
        let pick = |i, arch| pick_asset(i, arch, &a);
        assert_eq!(pick(Install::AppImage, "x86_64").as_deref(), Some("A"));
        assert_eq!(pick(Install::Tarball, "x86_64").as_deref(), Some("T"));
        assert_eq!(pick(Install::Flatpak, "x86_64").as_deref(), Some("F"));
        assert_eq!(pick(Install::Windows, "x86_64").as_deref(), Some("W"));
        assert_eq!(pick(Install::MacOs, "aarch64").as_deref(), Some("M"));
        // No macOS x86_64 or Linux aarch64 builds; pacman updates via the package manager.
        assert_eq!(pick(Install::MacOs, "x86_64"), None);
        assert_eq!(pick(Install::AppImage, "aarch64"), None);
        assert_eq!(pick(Install::Tarball, "aarch64"), None);
        assert_eq!(pick(Install::Pacman, "x86_64"), None);
    }

    #[test]
    fn excerpt_and_url() {
        let e = excerpt("## Highlights\n\n**New** stuff\n");
        assert_eq!(e, "Highlights\nNew stuff");
        assert_eq!(enc("a b&c"), "a%20b%26c");
    }
}
