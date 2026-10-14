//! Small on-disk state in XDG directories (no extra deps):
//! - `$XDG_STATE_HOME/vivi/recent.txt`  — recently played files, newest first
//! - `$XDG_STATE_HOME/vivi/watch_later/` — mpv resume positions
//! - `$XDG_DATA_HOME/vivi/playlists/*.m3u` — saved playlists
//! - `$XDG_DATA_HOME/vivi/plugins/<id>/<version>/` — downloaded plugin packages
//! - `$XDG_CACHE_HOME/vivi/` — library database and thumbnails

use std::fs;
use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 12;

/// Home directory (`USERPROFILE` on Windows).
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| ".".into())
}
fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
        .join("vivi")
}
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}
pub fn watch_later_dir() -> PathBuf {
    state_dir().join("watch_later")
}
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}
pub fn playlists_dir() -> PathBuf {
    data_dir().join("playlists")
}

/// Write via temp file + rename so a crash never leaves a truncated file.
pub fn write_atomic(path: &Path, data: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)
}

pub fn recent() -> Vec<String> {
    fs::read_to_string(state_dir().join("recent.txt"))
        .map(|s| {
            s.lines()
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

pub fn push_recent(path: &str) -> Vec<String> {
    let mut list = recent();
    list.retain(|p| p != path);
    list.insert(0, path.to_owned());
    list.truncate(MAX_RECENT);
    let _ = write_atomic(&state_dir().join("recent.txt"), &(list.join("\n") + "\n"));
    list
}

pub fn clear_recent() {
    let _ = fs::remove_file(state_dir().join("recent.txt"));
}

/// Saved playlists: (name, path).
pub fn playlists() -> Vec<(String, PathBuf)> {
    let mut v: Vec<_> = fs::read_dir(playlists_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "m3u"))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect();
    v.sort();
    v
}

pub fn save_playlist(name: &str, files: &[String]) -> std::io::Result<PathBuf> {
    let safe: String = name
        .chars()
        .map(|c| if c == '/' || c == '\0' { '_' } else { c })
        .collect();
    let safe = if safe.trim().is_empty() {
        "Playlist".into()
    } else {
        safe.trim().to_owned()
    };
    let path = playlists_dir().join(format!("{safe}.m3u"));
    write_atomic(&path, &format!("#EXTM3U\n{}\n", files.join("\n")))?;
    Ok(path)
}

pub fn delete_playlist(path: &Path) {
    let _ = fs::remove_file(path);
}

/// File name for display.
pub fn display_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

/// Saved resume position for `path` (mpv names watch-later files by the MD5 of the path).
pub fn resume_position(path: &str) -> Option<f64> {
    let name = format!("{:X}", md5::compute(path.as_bytes()));
    let text = fs::read_to_string(watch_later_dir().join(name)).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("start="))
        .and_then(|v| v.trim().parse().ok())
}

/// Forget the resume position for `path`.
pub fn forget_position(path: &str) {
    let name = format!("{:X}", md5::compute(path.as_bytes()));
    let _ = fs::remove_file(watch_later_dir().join(name));
}
