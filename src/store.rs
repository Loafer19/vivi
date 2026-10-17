//! Small on-disk state in XDG directories (no extra deps):
//! - `$XDG_STATE_HOME/vivi/recent.json` — Recent (files, folders, playlists, streams), newest first
//! - `$XDG_STATE_HOME/vivi/watch_later/` — mpv resume positions
//! - `$XDG_DATA_HOME/vivi/playlists/*.m3u` — saved playlists
//! - `$XDG_DATA_HOME/vivi/plugins/<id>/<version>/` — downloaded plugin packages
//! - `$XDG_CACHE_HOME/vivi/` — library database and thumbnails

use std::fs;
use std::path::{Path, PathBuf};

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

// ---- Recent ----
//
// `recent.json` (newest first) replaces the old one-path-per-line `recent.txt`, which is
// migrated on first read. Every entry has a target (path or URL), a type, and the title /
// secondary line when the opener knew them (station name, track title, ...).

const MAX_RECENT: usize = 50;

/// What a Recent entry opens; drives its icon and type label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecentKind {
    Video,
    Audio,
    Folder,
    Playlist,
    Stream,
    Web,
    Music,
}

impl RecentKind {
    /// Short type label shown on the entry.
    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Folder => "Folder",
            Self::Playlist => "Playlist",
            Self::Stream => "Stream",
            Self::Web => "Web video",
            Self::Music => "Free Music",
        }
    }
    /// Icon name in the context-menu icon set.
    pub fn menu_icon(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio | Self::Music => "audio",
            Self::Folder => "folder",
            Self::Playlist => "playlist",
            Self::Stream => "streaming",
            Self::Web => "stream",
        }
    }
    /// Type of a target when the opener did not say: folders, playlist files, yt-dlp
    /// sites, other URLs (streams / radio), then audio or video by extension.
    pub fn infer(target: &str) -> Self {
        if let Some(host) = url_host(target) {
            const WEB: [&str; 8] = [
                "youtube.com",
                "youtu.be",
                "vimeo.com",
                "twitch.tv",
                "soundcloud.com",
                "dailymotion.com",
                "bandcamp.com",
                "bilibili.com",
            ];
            return if WEB
                .iter()
                .any(|w| host == *w || host.ends_with(&format!(".{w}")))
            {
                Self::Web
            } else {
                Self::Stream
            };
        }
        let p = Path::new(target);
        if p.is_dir() {
            return Self::Folder;
        }
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "m3u" | "m3u8" | "pls" | "xspf" | "cue" => Self::Playlist,
            _ if crate::library::kind_of(p) == Some("audio") => Self::Audio,
            _ => Self::Video,
        }
    }
}

/// Host of a `scheme://host/...` URL (without `www.` and port), or None for paths.
pub fn url_host(target: &str) -> Option<String> {
    let (scheme, rest) = target.split_once("://")?;
    if scheme.is_empty() || scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string()).filter(|h| !h.is_empty())
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecentEntry {
    pub target: String,
    pub kind: RecentKind,
    /// Display title when known (station, track, page title); empty = derive from target.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// Secondary line when known (artist, station tags); empty = folder / host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub: String,
    /// Picture when known: station logo or cover URL, or a local image path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub image: String,
}

impl RecentEntry {
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_owned(),
            kind: RecentKind::infer(target),
            title: String::new(),
            sub: String::new(),
            image: String::new(),
        }
    }
    /// Title to show: the known one, else the file / folder name, else the URL's last
    /// path segment or host.
    pub fn display_title(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        if url_host(&self.target).is_some() {
            let last = self
                .target
                .split(['?', '#'])
                .next()
                .unwrap_or("")
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("");
            let host = url_host(&self.target).unwrap_or_default();
            // "watch?v=…": a bare page name is not a title without its query.
            let query = self
                .target
                .split_once('?')
                .map(|(_, q)| q.split('#').next().unwrap_or(""));
            return if last.is_empty() || last.contains(&host) || last == host {
                host
            } else if !last.contains('.') && query.is_some_and(|q| !q.is_empty()) {
                format!("{last}?{}", query.unwrap_or_default())
            } else {
                last.to_string()
            };
        }
        display_name(&self.target)
    }
    /// Secondary line: the known one, else the host for URLs, else the parent folder.
    pub fn display_sub(&self) -> String {
        if !self.sub.is_empty() {
            return self.sub.clone();
        }
        if let Some(h) = url_host(&self.target) {
            return h;
        }
        Path::new(&self.target)
            .parent()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn recent_json() -> PathBuf {
    state_dir().join("recent.json")
}

fn parse_recent(json: &str) -> Vec<RecentEntry> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Old `recent.txt`: one target per line, newest first.
fn migrate_recent_txt(txt: &str) -> Vec<RecentEntry> {
    let mut v: Vec<RecentEntry> = Vec::new();
    for l in txt.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !v.iter().any(|e| e.target == l) {
            v.push(RecentEntry::new(l));
        }
    }
    v.truncate(MAX_RECENT);
    v
}

/// Recent entries, newest first (migrates `recent.txt` the first time).
pub fn recent_entries() -> Vec<RecentEntry> {
    if let Ok(s) = fs::read_to_string(recent_json()) {
        return parse_recent(&s);
    }
    let old = state_dir().join("recent.txt");
    let Ok(txt) = fs::read_to_string(&old) else {
        return Vec::new();
    };
    let v = migrate_recent_txt(&txt);
    if write_recent(&v).is_ok() {
        let _ = fs::remove_file(old);
    }
    v
}

fn write_recent(v: &[RecentEntry]) -> std::io::Result<()> {
    let s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "[]".into());
    write_atomic(&recent_json(), &s)
}

/// Recent targets (paths / URLs), newest first.
pub fn recent() -> Vec<String> {
    recent_entries().into_iter().map(|e| e.target).collect()
}

/// Put `e` at the top; a reopened target moves up and keeps a title it learned before.
fn push_into(list: &mut Vec<RecentEntry>, mut e: RecentEntry) {
    if let Some(i) = list.iter().position(|x| x.target == e.target) {
        let old = list.remove(i);
        if e.title.is_empty() {
            e.title = old.title;
        }
        if e.sub.is_empty() {
            e.sub = old.sub;
        }
        if e.image.is_empty() {
            e.image = old.image;
        }
    }
    list.insert(0, e);
    list.truncate(MAX_RECENT);
}

/// Title / type an opener knows for a target it is about to load (Free Music track,
/// radio station, ...); `on_file_loaded` takes it when that target loads.
static HINTS: std::sync::Mutex<Vec<RecentEntry>> = std::sync::Mutex::new(Vec::new());

pub fn recent_hint(e: RecentEntry) {
    if let Ok(mut h) = HINTS.lock() {
        h.retain(|x| x.target != e.target);
        h.push(e);
        let n = h.len();
        if n > 64 {
            h.drain(..n - 64);
        }
    }
}

pub fn take_recent_hint(target: &str) -> Option<RecentEntry> {
    let mut h = HINTS.lock().ok()?;
    let i = h.iter().position(|x| x.target == target)?;
    Some(h.remove(i))
}

pub fn push_recent_entry(e: RecentEntry) {
    let mut list = recent_entries();
    push_into(&mut list, e);
    let _ = write_recent(&list);
}

/// Drop one entry.
pub fn remove_recent(target: &str) {
    let mut list = recent_entries();
    list.retain(|e| e.target != target);
    let _ = write_recent(&list);
}

pub fn clear_recent() {
    let _ = fs::remove_file(recent_json());
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
    if let Some(c) = playlist_cover(path) {
        let _ = fs::remove_file(c);
    }
}

const COVER_EXT: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// Thumbnail saved next to a playlist (`Name.m3u` → `Name.jpg`/`.png`/…).
pub fn playlist_cover(playlist: &Path) -> Option<PathBuf> {
    COVER_EXT
        .iter()
        .map(|e| playlist.with_extension(e))
        .find(|p| p.is_file())
}

/// Copy `image` next to the playlist as its cover (replacing an older one).
pub fn set_playlist_cover(playlist: &Path, image: &Path) -> std::io::Result<PathBuf> {
    let ext = image
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .filter(|e| COVER_EXT.contains(&e.as_str()))
        .ok_or_else(|| std::io::Error::other("unsupported image type"))?;
    if let Some(old) = playlist_cover(playlist) {
        let _ = fs::remove_file(old);
    }
    let dst = playlist.with_extension(ext);
    fs::copy(image, &dst)?;
    Ok(dst)
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

#[cfg(test)]
mod recent_tests {
    use super::*;

    #[test]
    fn infers_kinds() {
        assert_eq!(
            RecentKind::infer("https://www.youtube.com/watch?v=x"),
            RecentKind::Web
        );
        assert_eq!(
            RecentKind::infer("https://m.youtube.com/watch?v=x"),
            RecentKind::Web
        );
        assert_eq!(
            RecentKind::infer("http://ice.example.org:8000/live.mp3"),
            RecentKind::Stream
        );
        assert_eq!(RecentKind::infer("/no/such/song.flac"), RecentKind::Audio);
        assert_eq!(RecentKind::infer("/no/such/film.mkv"), RecentKind::Video);
        assert_eq!(RecentKind::infer("/no/such/list.m3u"), RecentKind::Playlist);
        assert_eq!(
            RecentKind::infer(env!("CARGO_MANIFEST_DIR")),
            RecentKind::Folder
        );
    }

    #[test]
    fn hosts_and_titles() {
        assert_eq!(
            url_host("http://user@WWW.Radio.example:8000/a").as_deref(),
            Some("radio.example")
        );
        assert_eq!(url_host("/home/a.mp3"), None);
        assert_eq!(url_host("file:///home/a.mp3"), None);
        let e = RecentEntry::new("http://ice.example.org:8000/live.mp3");
        assert_eq!(e.display_title(), "live.mp3");
        assert_eq!(e.display_sub(), "ice.example.org");
        let e = RecentEntry::new("https://www.youtube.com/watch?v=abc");
        assert_eq!(e.display_title(), "watch?v=abc");
        assert_eq!(e.display_sub(), "youtube.com");
        let e = RecentEntry::new("https://radio.example/");
        assert_eq!(e.display_title(), "radio.example");
        let e = RecentEntry::new("/music/Artist/song.flac");
        assert_eq!(e.display_title(), "song.flac");
        assert_eq!(e.display_sub(), "/music/Artist");
    }

    #[test]
    fn migrates_old_list() {
        let v = migrate_recent_txt("/a.mkv\n\nhttp://x.example/s\n/a.mkv\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].kind, RecentKind::Video);
        assert_eq!(v[1].kind, RecentKind::Stream);
        // Round trip through the JSON format.
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(parse_recent(&s), v);
        assert!(parse_recent("not json").is_empty());
    }

    #[test]
    fn dedupes_and_caps() {
        let mut l = Vec::new();
        let mut e = RecentEntry::new("http://x.example/s");
        e.title = "Station X".into();
        push_into(&mut l, e);
        push_into(&mut l, RecentEntry::new("/b.mkv"));
        // Reopened without a title: moves to the top and keeps "Station X".
        push_into(&mut l, RecentEntry::new("http://x.example/s"));
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].title, "Station X");
        for i in 0..80 {
            push_into(&mut l, RecentEntry::new(&format!("/f{i}.mkv")));
        }
        assert_eq!(l.len(), MAX_RECENT);
        assert_eq!(l[0].target, "/f79.mkv");
    }
}
