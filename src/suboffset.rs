//! Per-file subtitle offset, built into the player (independent of the Subtitle Search
//! plugin): `z` / `x` and `Alt+Z` / `Alt+X` move subtitles 0.1 s earlier / later
//! (`Shift`: 1 s; keymap `SubtitleOffset`, `SubtitleOffsetAlt`), the side panel has the
//! same buttons, a reset and the delay slider, packages send `vivi-sub-offset`, and the
//! control socket has `sub_offset`. The value is mpv's `sub-delay`, saved per file in
//! `$XDG_DATA_HOME/vivi/subtitle-offsets.json` (the file Subtitle Search used, same keys)
//! and applied when the file opens; a file without one gets back the offset it had
//! before the previous file's was applied.
//!
//! Packages see `user-data/vivi/sub-offset` = 1 (and the older script-opt
//! `vivi-sub-offsets=1`) and leave keys and storage to the player. Offsets the Subtitle
//! Auto-Sync package kept itself on older Vivi (`plugin-data/subtitle-sync/offsets.json`,
//! `path|size` keys) are imported once at startup or on `vivi-sub-offset-import`; the
//! marker `offsets.imported` next to that file records the handover.

use crate::player::Player;
use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The package that aligns subtitles to the audio (side panel *Auto-sync*).
pub const SYNC_PACKAGE: &str = "subtitle-sync";

#[derive(Default)]
struct State {
    /// Key of the playing file (canonical path or URL).
    key: Option<String>,
    /// The saved offset applied to the playing file (undone on the next file).
    applied: Option<f64>,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State::default());
}

pub fn offsets_file() -> PathBuf {
    crate::store::data_dir().join("subtitle-offsets.json")
}

fn sync_dir() -> PathBuf {
    crate::store::data_dir()
        .join("plugin-data")
        .join(SYNC_PACKAGE)
}

/// Round to 0.05 s and clamp to ±10 min.
pub fn clamp_offset(o: f64) -> f64 {
    ((o * 20.0).round() / 20.0).clamp(-600.0, 600.0)
}

/// "+0.1 s", "−1.25 s", "0.0 s".
pub fn offset_label(o: f64) -> String {
    if o.abs() < 0.025 {
        return "0.0 s".into();
    }
    let a = o.abs();
    let n = if ((a * 10.0).round() - a * 10.0).abs() < 1e-6 {
        format!("{a:.1}")
    } else {
        format!("{a:.2}")
    };
    format!("{}{n} s", if o > 0.0 { "+" } else { "−" })
}

fn load(file: &Path) -> HashMap<String, f64> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(file: &Path, key: &str, o: f64) {
    let mut m = load(file);
    if o == 0.0 {
        m.remove(key);
    } else {
        m.insert(key.to_string(), o);
    }
    if let Ok(s) = serde_json::to_string_pretty(&m) {
        let _ = crate::store::write_atomic(file, &s);
    }
}

/// Key of a file as mpv's `path` gives it: canonical local path, else the URL.
pub fn key_of(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let p = Path::new(path);
    if p.is_file() {
        let c = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        return Some(c.to_string_lossy().into_owned());
    }
    Some(path.to_string())
}

/// Entries of the Auto-Sync package's `offsets.json` (`path|size` → seconds) as player
/// keys. A local file whose size changed since is skipped (the package would not have
/// matched it either), as are zero and unparsable values.
pub fn convert_package_offsets(m: &HashMap<String, serde_json::Value>) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = m
        .iter()
        .filter_map(|(k, v)| {
            let o = clamp_offset(v.as_f64()?);
            if o == 0.0 {
                return None;
            }
            let (path, size) = k.rsplit_once('|')?;
            let p = Path::new(path);
            if let Ok(md) = std::fs::metadata(p) {
                if md.is_file() && size.parse::<u64>().ok() != Some(md.len()) {
                    return None;
                }
            }
            Some((key_of(path)?, o))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Import the Auto-Sync package's own offsets once (entries the player already has
/// win). Returns how many were added; `None` when there was nothing to hand over.
pub fn import_package_offsets() -> Option<usize> {
    import_from(&sync_dir(), &offsets_file())
}

fn import_from(dir: &Path, into: &Path) -> Option<usize> {
    let src = dir.join("offsets.json");
    let marker = dir.join("offsets.imported");
    if marker.exists() || !src.is_file() {
        return None;
    }
    let m: HashMap<String, serde_json::Value> = std::fs::read_to_string(&src)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut have = load(into);
    let mut n = 0;
    for (k, o) in convert_package_offsets(&m) {
        if let std::collections::hash_map::Entry::Vacant(e) = have.entry(k) {
            e.insert(o);
            n += 1;
        }
    }
    if n > 0 {
        let s = serde_json::to_string_pretty(&have).ok()?;
        crate::store::write_atomic(into, &s).ok()?;
    }
    let _ = crate::store::write_atomic(
        &marker,
        &format!("{n} subtitle offset(s) imported into {}\n", into.display()),
    );
    Some(n)
}

/// Startup: host flags for packages and the one-time handover.
pub fn setup(player: &Player) {
    if let Err(e) = player.mpv.set_property("user-data/vivi/sub-offset", 1i64) {
        eprintln!("vivi: set user-data/vivi/sub-offset: {e:?}");
    }
    // Subtitle Auto-Sync 1.2 checks this script-opt (Vivi 0.12 set it only with
    // Subtitle Search on); with it the package sends `vivi-sub-offset` instead of
    // keeping offsets itself.
    player.cmd(
        "change-list",
        &["script-opts", "append", "vivi-sub-offsets=1"],
    );
    if let Some(n) = import_package_offsets() {
        eprintln!("vivi: imported {n} subtitle offset(s) from Subtitle Auto-Sync");
    }
}

pub fn current(p: &Player) -> f64 {
    p.get("sub-delay")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

fn show(a: &AppWindow, o: f64) {
    a.global::<crate::Subtitles>()
        .set_offset(offset_label(o).into());
    a.global::<crate::Panel>().set_pv_sub_delay(o as f32);
}

/// Set (and remember for the playing file) the offset.
pub fn set(a: &AppWindow, p: &Player, o: f64, toast: bool) {
    let o = clamp_offset(o);
    p.set("sub-delay", &format!("{o}"));
    let key = S.with(|s| {
        let mut s = s.borrow_mut();
        s.applied = Some(o);
        s.key.clone()
    });
    if let Some(k) = &key {
        save(&offsets_file(), k, o);
    }
    show(a, o);
    if toast {
        let what = if o == 0.0 {
            "Subtitle offset reset".to_string()
        } else {
            format!(
                "Subtitle offset {}{}",
                offset_label(o),
                if key.is_some() {
                    " · saved for this file"
                } else {
                    ""
                }
            )
        };
        crate::plugins::toast(a, &what);
    }
}

/// `reset`, `set:<seconds>` or a delta in seconds (keys, panel, `vivi-sub-offset`,
/// control socket).
pub fn command(a: &AppWindow, p: &Player, arg: &str) {
    let o = match arg.trim() {
        "reset" => 0.0,
        v => match v.strip_prefix("set:") {
            Some(x) => x.trim().parse().unwrap_or(0.0),
            None => current(p) + v.parse::<f64>().unwrap_or(0.0),
        },
    };
    set(a, p, o, true);
}

/// The side panel's delay slider moved (mpv already has the value).
pub fn changed(a: &AppWindow, p: &Player, v: &str) {
    set(a, p, v.parse().unwrap_or(0.0), false);
}

/// Side panel *Auto-sync*: hand the subtitle to the Subtitle Auto-Sync package.
pub fn sync(a: &AppWindow, p: &Player) {
    match crate::plugins::package_script(SYNC_PACKAGE) {
        Some(script) => p.cmd("script-message-to", &[&script, "sync"]),
        None => crate::plugins::toast(
            a,
            "Install Subtitle Auto-Sync (Settings → Extensions → Browse) to align subtitles to the audio",
        ),
    }
}

/// A file opened: apply its saved offset, or undo the previous file's one.
pub fn file_loaded(a: &AppWindow, p: &Player) {
    let key = key_of(&p.get("path").unwrap_or_default());
    let saved = key.as_ref().and_then(|k| load(&offsets_file()).remove(k));
    let prev = S.with(|s| {
        let mut s = s.borrow_mut();
        s.key = key;
        std::mem::replace(&mut s.applied, saved)
    });
    match saved {
        Some(o) => {
            p.set("sub-delay", &format!("{o}"));
            show(a, o);
        }
        None => {
            let cur = current(p);
            if prev.is_some_and(|x| (x - cur).abs() < 1e-6) {
                p.set("sub-delay", "0");
                show(a, 0.0);
            } else {
                show(a, cur);
            }
        }
    }
    a.global::<crate::Subtitles>()
        .set_sync(crate::plugins::package_script(SYNC_PACKAGE).is_some());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_labels() {
        assert_eq!(clamp_offset(0.123), 0.1);
        assert_eq!(clamp_offset(0.13), 0.15);
        assert_eq!(clamp_offset(-9999.0), -600.0);
        assert_eq!(offset_label(0.0), "0.0 s");
        assert_eq!(offset_label(0.1), "+0.1 s");
        assert_eq!(offset_label(-1.25), "−1.25 s");
        assert_eq!(offset_label(2.0), "+2.0 s");
    }

    #[test]
    fn package_handover() {
        let dir = std::env::temp_dir().join(format!("vivi-suboffset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("a.mkv");
        std::fs::write(&video, b"12345").unwrap();
        let changed = dir.join("b.mkv");
        std::fs::write(&changed, b"123").unwrap();
        let canon = std::fs::canonicalize(&video).unwrap();
        let kept = std::fs::canonicalize(dir.join("b.mkv")).unwrap();
        let into = dir.join("subtitle-offsets.json");
        // The player already has b.mkv: its value wins.
        std::fs::write(
            &into,
            serde_json::json!({ kept.to_string_lossy(): 1.0 }).to_string(),
        )
        .unwrap();
        let pkg = serde_json::json!({
            format!("{}|5", video.display()): 2.5,
            format!("{}|99", changed.display()): 3.0,
            "https://example.org/v.mkv|0": -0.4,
            "/gone/c.mkv|7": 0.0,
            "no-size": 1.0,
        });
        std::fs::write(dir.join("offsets.json"), pkg.to_string()).unwrap();
        assert_eq!(import_from(&dir, &into), Some(2));
        let m = load(&into);
        assert_eq!(m.get(canon.to_str().unwrap()), Some(&2.5));
        assert_eq!(m.get("https://example.org/v.mkv"), Some(&-0.4));
        assert_eq!(m.get(kept.to_str().unwrap()), Some(&1.0));
        assert_eq!(m.len(), 3);
        assert!(dir.join("offsets.imported").is_file());
        // Once only.
        assert_eq!(import_from(&dir, &into), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys() {
        assert_eq!(key_of(""), None);
        assert_eq!(
            key_of("https://example.org/v.mkv").as_deref(),
            Some("https://example.org/v.mkv")
        );
    }
}
