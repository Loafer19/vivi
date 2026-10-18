//! External plugins: mpv Lua/JavaScript user scripts in `~/.config/vivi/scripts`.
//!
//! Every `*.lua` / `*.js` file (or directory with `main.lua` / `main.js`) is one plugin.
//! Optional header comments fill in the manifest:
//!
//! ```lua
//! -- @name Auto Speed
//! -- @description Speeds up silent parts
//! -- @version 1.2
//! -- @author Jane
//! ```
//!
//! Enabled scripts are loaded with mpv's `load-script` command at startup (and when
//! switched on). mpv cannot unload a script, so switching one off needs a restart.

use super::{Manifest, Origin};
use crate::config::Config;
use std::path::{Path, PathBuf};

/// Config entry holding the "Load add-ons on startup" flag.
pub const GLOBAL_ID: &str = "mpv-scripts";

#[derive(Debug, Clone)]
pub struct Script {
    pub path: PathBuf,
    pub manifest: Manifest,
}

pub fn load_on_startup(c: &Config) -> bool {
    c.plugins
        .get(GLOBAL_ID)
        .and_then(|s| s.settings.get("startup"))
        .is_none_or(|v| v == "true")
}

/// Scripts found in `dir`, sorted by file name.
pub fn scan(dir: &Path) -> Vec<Script> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<Script> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let main = if p.is_dir() {
                ["main.lua", "main.js"]
                    .iter()
                    .map(|m| p.join(m))
                    .find(|m| m.is_file())?
            } else {
                let ext = p.extension()?.to_str()?.to_ascii_lowercase();
                if ext != "lua" && ext != "js" {
                    return None;
                }
                p.clone()
            };
            let stem = if p.is_dir() {
                p.file_name()?.to_string_lossy().into_owned()
            } else {
                p.file_stem()?.to_string_lossy().into_owned()
            };
            let head = std::fs::read_to_string(&main).unwrap_or_default();
            Some(Script {
                manifest: manifest_from(&stem, &main, &head),
                path: p,
            })
        })
        .collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

/// Build a manifest from a script's header comments.
pub fn manifest_from(stem: &str, main: &Path, src: &str) -> Manifest {
    let mut m = Manifest {
        id: format!("script-{}", slug(stem)),
        name: stem.to_string(),
        description: format!("mpv script {}", main.display()),
        version: String::new(),
        author: String::new(),
        icon: "script".into(),
        settings: Vec::new(),
        default_enabled: true,
        origin: Origin::Script,
        role: Default::default(),
        contributes: Vec::new(),
    };
    for line in src.lines().take(40) {
        let l = line.trim();
        let Some(c) = l.strip_prefix("--").or_else(|| l.strip_prefix("//")) else {
            if l.is_empty() {
                continue;
            }
            break;
        };
        let Some(c) = c.trim().strip_prefix('@') else {
            continue;
        };
        let (k, v) = c.split_once(char::is_whitespace).unwrap_or((c, ""));
        let v = v.trim().to_string();
        match k {
            "name" => m.name = v,
            "description" => m.description = v,
            "version" => m.version = v,
            "author" => m.author = v,
            _ => {}
        }
    }
    m
}

fn slug(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    s.split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header() {
        let m = manifest_from(
            "Auto Speed",
            Path::new("/x/auto.lua"),
            "-- @name Auto speed\n-- @version 1.2\n--   @author Jane Doe\nlocal x = 1\n-- @description late",
        );
        assert_eq!(m.id, "script-auto-speed");
        assert_eq!(m.name, "Auto speed");
        assert_eq!(m.version, "1.2");
        assert_eq!(m.author, "Jane Doe");
        assert!(m.description.contains("/x/auto.lua"));
        assert_eq!(m.origin, Origin::Script);
    }

    #[test]
    fn scan_dir() {
        let d = std::env::temp_dir().join(format!("vivi-scripts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("pack")).unwrap();
        std::fs::write(d.join("a.lua"), "-- @name A\n").unwrap();
        std::fs::write(d.join("b.js"), "// @name B\n").unwrap();
        std::fs::write(d.join("notes.txt"), "").unwrap();
        std::fs::write(d.join("pack/main.lua"), "").unwrap();
        let s = scan(&d);
        let names: Vec<&str> = s.iter().map(|s| s.manifest.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "pack"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
