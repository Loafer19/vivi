//! User settings persisted to `$XDG_CONFIG_HOME/vivi/config.toml` (atomic writes).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub interface: Interface,
    pub playback: Playback,
    pub audio: Audio,
    pub subtitles: Subtitles,
    pub library: Library,
    pub general: General,
    pub video: Video,
    pub controls: Controls,
    pub network: Network,
    pub performance: Performance,
    pub recording: Recording,
    /// Values of settings that are shown in the UI but not implemented yet
    /// (casting, streaming, extensions, …), keyed by setting id.
    pub stubs: BTreeMap<String, String>,
    /// Per-show presets keyed by normalized show name (see [`show_key`]).
    pub presets: BTreeMap<String, Preset>,
    /// Plugin state keyed by plugin id (see `src/plugins`).
    pub plugins: BTreeMap<String, PluginState>,
}

/// Enable flag and settings of one plugin. `enabled = None` uses the plugin's default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PluginState {
    pub enabled: Option<bool>,
    pub settings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Interface {
    /// "dark" | "light"
    pub theme: String,
    /// Accent color as `#RRGGBB`.
    pub accent: String,
    /// Seconds of mouse inactivity before controls hide.
    pub autohide_secs: u32,
    /// Toolbar profile: "Minimal" | "Default" | "Power" | "Custom".
    pub profile: String,
    /// Bottom-bar items; `|` marks the play controls (left of it = left group).
    pub toolbar: Vec<String>,
    /// First-run onboarding completed or skipped.
    pub onboarded: bool,
    /// Miniplayer geometry `[x, y, width, height]` (height without the queue view).
    pub mini_window: Option<[i32; 4]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Playback {
    pub resume: bool,
    pub hwdec: String,
    pub speed: f64,
    /// "Off" | "One" | "All" (mpv `loop-file` / `loop-playlist`).
    pub repeat: String,
    /// Precise seeking (mpv `hr-seek`).
    pub precise_seek: bool,
    /// Preferred stream quality for yt-dlp (mpv `ytdl-format`).
    pub quality: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Audio {
    /// Preferred languages, mpv `alang` syntax (e.g. "uk,en").
    pub language: String,
    pub volume_max: i64,
    pub enabled: bool,
    /// mpv `audio-device` name ("auto" = system default).
    pub device: String,
    /// Volume at startup (%).
    pub startup_volume: i64,
    /// Arrow-key volume step (%).
    pub volume_step: i64,
    /// Dynamic loudness normalization (lavfi dynaudnorm filter).
    pub normalize: bool,
    /// Default audio delay in seconds.
    pub delay: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Subtitles {
    pub language: String,
    pub scale: f64,
    /// mpv `sub-auto`: "no" | "exact" | "fuzzy" | "all"
    pub auto_load: String,
    pub enabled: bool,
    /// mpv `sub-codepage` ("auto", "utf8", "cp1251", …).
    pub encoding: String,
    pub font: String,
    pub bold: bool,
    /// Text color `#RRGGBB`.
    pub color: String,
    /// Text opacity 0–100 %.
    pub opacity: i64,
    /// mpv `sub-border-size` (0 = none).
    pub outline: f64,
    pub outline_color: String,
    pub shadow: bool,
    /// Vertical position, mpv `sub-pos` (100 = bottom).
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// Interface language (only English is available for now).
    pub language: String,
    /// Write an mpv log file.
    pub save_log: bool,
    pub log_file: String,
    /// "Errors only" | "Warnings" | "Everything"
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Video {
    pub enabled: bool,
    pub stop_screensaver: bool,
    pub hue: i64,
    pub brightness: i64,
    pub contrast: i64,
    pub saturation: i64,
    pub gamma: i64,
    /// mpv `deinterlace`: "auto" | "yes" | "no"
    pub deinterlace: String,
    /// mpv `video-aspect-override` ("no" = automatic).
    pub aspect: String,
    pub screenshot_dir: String,
    /// mpv `screenshot-format`: "png" | "jpg" | "webp"
    pub screenshot_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Controls {
    /// Arrow-key seek step in seconds.
    pub short_skip: u32,
    /// Shift+arrow seek step in seconds.
    pub long_skip: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Network {
    /// "Low" | "Normal" | "High" (demuxer cache size).
    pub buffering: String,
    /// mpv `cache`: "auto" | "yes" | "no"
    pub cache: String,
    /// Seconds to read ahead (mpv `cache-secs`).
    pub cache_secs: i64,
    pub http_proxy: String,
    pub tls_verify: bool,
    pub user_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Performance {
    /// mpv `vd-lavc-threads` (0 = automatic).
    pub threads: i64,
    /// mpv `vd-lavc-skiploopfilter`.
    pub skip_loop_filter: String,
    /// mpv `framedrop` (vo) when true.
    pub framedrop: bool,
    /// mpv `video-sync`.
    pub video_sync: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Recording {
    pub dir: String,
}

impl Default for General {
    fn default() -> Self {
        Self {
            language: "English".into(),
            save_log: false,
            log_file: crate::store::cache_dir()
                .join("vivi.log")
                .to_string_lossy()
                .into_owned(),
            log_level: "Errors only".into(),
        }
    }
}
impl Default for Video {
    fn default() -> Self {
        Self {
            enabled: true,
            stop_screensaver: true,
            hue: 0,
            brightness: 0,
            contrast: 0,
            saturation: 0,
            gamma: 0,
            deinterlace: "auto".into(),
            aspect: "no".into(),
            screenshot_dir: crate::store::home()
                .join("Pictures")
                .to_string_lossy()
                .into_owned(),
            screenshot_format: "png".into(),
        }
    }
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            short_skip: 5,
            long_skip: 60,
        }
    }
}
impl Default for Network {
    fn default() -> Self {
        Self {
            buffering: "Normal".into(),
            cache: "auto".into(),
            cache_secs: 60,
            http_proxy: String::new(),
            tls_verify: true,
            user_agent: String::new(),
        }
    }
}
impl Default for Performance {
    fn default() -> Self {
        Self {
            threads: 0,
            skip_loop_filter: "default".into(),
            framedrop: true,
            video_sync: "audio".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Library {
    /// Folders scanned for media.
    pub folders: Vec<String>,
    /// Files and folders pinned to Home.
    pub pinned: Vec<String>,
    /// "grid" | "list"
    pub view: String,
    /// "name" | "date" | "size" | "watched"
    pub sort: String,
    /// Generate thumbnails with ffmpeg (if installed).
    pub thumbnails: bool,
    /// Files marked as watched from the context menu.
    pub watched: Vec<String>,
}

impl Default for Library {
    fn default() -> Self {
        let home = crate::store::home().to_string_lossy().replace('\\', "/");
        let folders = ["Videos", "Music"]
            .iter()
            .map(|d| format!("{home}/{d}"))
            .filter(|p| std::path::Path::new(p).is_dir())
            .collect();
        Self {
            folders,
            pinned: Vec::new(),
            view: "grid".into(),
            sort: "name".into(),
            watched: Vec::new(),
            thumbnails: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Preset {
    pub sub_delay: f64,
    pub audio_delay: f64,
    pub sub_scale: f64,
    pub sub_pos: i64,
    pub sub_lang: String,
    pub audio_lang: String,
}

impl Default for Interface {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            accent: "#D37918".into(),
            autohide_secs: 2,
            profile: "Default".into(),
            toolbar: profile_toolbar("Default").unwrap_or_default(),
            onboarded: false,
            mini_window: None,
        }
    }
}
impl Default for Playback {
    fn default() -> Self {
        Self {
            resume: true,
            hwdec: "auto-safe".into(),
            speed: 1.0,
            repeat: "Off".into(),
            precise_seek: false,
            quality: "Best available".into(),
        }
    }
}
impl Default for Audio {
    fn default() -> Self {
        Self {
            language: String::new(),
            volume_max: 150,
            enabled: true,
            device: "auto".into(),
            startup_volume: 100,
            volume_step: 5,
            normalize: false,
            delay: 0.0,
        }
    }
}
impl Default for Subtitles {
    fn default() -> Self {
        Self {
            language: String::new(),
            scale: 1.0,
            auto_load: "fuzzy".into(),
            enabled: true,
            encoding: "auto".into(),
            font: "sans-serif".into(),
            bold: false,
            color: "#FFFFFF".into(),
            opacity: 100,
            outline: 3.0,
            outline_color: "#000000".into(),
            shadow: false,
            position: 100,
        }
    }
}

/// Directory with mpv user scripts loaded as external plugins (`~/.config/vivi/scripts`).
pub fn scripts_dir() -> PathBuf {
    path().with_file_name("scripts")
}

pub fn path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| crate::store::home().join(".config"))
        .join("vivi")
        .join("config.toml")
}

impl Config {
    /// Load the config; a missing file gives defaults, a broken one is kept as `.bak`.
    pub fn load() -> Self {
        let p = path();
        match std::fs::read_to_string(&p) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!("vivi: invalid {}: {e}; using defaults", p.display());
                let _ = std::fs::copy(&p, p.with_extension("toml.bak"));
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(
            &tmp,
            toml::to_string_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, p)
    }
}

/// Normalized show name from a file name: "The_Wand_S01E05.mp4" → "the wand".
pub fn show_key(path: &str) -> String {
    let name = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let norm: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = norm.split_whitespace().collect();
    // Cut at the first episode marker like s01e05, s01, e05, 1x05.
    let is_marker = |w: &str| {
        let b = w.as_bytes();
        (b.len() >= 2 && (b[0] == b's' || b[0] == b'e') && b[1].is_ascii_digit())
            || (w.contains('x')
                && w.split('x')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
    };
    let end = words
        .iter()
        .position(|w| is_marker(w))
        .unwrap_or(words.len());
    let key = words[..end].join(" ");
    if key.is_empty() {
        norm.trim().to_owned()
    } else {
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn show_keys() {
        assert_eq!(show_key("/v/The_Wand_S01E05.mp4"), "the wand");
        assert_eq!(show_key("The.Wand.s02e11.1080p.mkv"), "the wand");
        assert_eq!(show_key("Show 1x05.mkv"), "show");
        assert_eq!(show_key("movie.mkv"), "movie");
    }
    #[test]
    fn roundtrip() {
        let mut c = Config::default();
        c.presets.insert(
            "x".into(),
            Preset {
                sub_delay: 0.5,
                ..Default::default()
            },
        );
        let s = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&s).unwrap(), c);
    }
}

/// Every item that can be placed in the bottom bar.
pub const TOOL_ITEMS: [&str; 16] = [
    "queue",
    "subtitles",
    "audio",
    "volume",
    "video",
    "speed",
    "chapters",
    "bookmark",
    "back10",
    "fwd10",
    "stop",
    "snapshot",
    "repeat",
    "shuffle",
    "settings",
    "fullscreen",
];

pub const PROFILES: [&str; 3] = ["Minimal", "Default", "Power"];

/// Built-in toolbar layouts.
pub fn profile_toolbar(name: &str) -> Option<Vec<String>> {
    let v: &[&str] = match name {
        "Minimal" => &["|", "volume", "fullscreen"],
        "Default" => &[
            "queue",
            "|",
            "subtitles",
            "audio",
            "volume",
            "video",
            "fullscreen",
        ],
        "Power" => &[
            "queue",
            "chapters",
            "bookmark",
            "back10",
            "fwd10",
            "|",
            "repeat",
            "shuffle",
            "snapshot",
            "subtitles",
            "audio",
            "volume",
            "speed",
            "video",
            "fullscreen",
        ],
        _ => return None,
    };
    Some(v.iter().map(|s| s.to_string()).collect())
}

/// Drop unknown/duplicate ids and make sure exactly one `|` exists.
pub fn sanitize_toolbar(v: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        let ok = s == "|" || TOOL_ITEMS.contains(&s.as_str());
        if ok && !out.contains(s) {
            out.push(s.clone());
        }
    }
    if !out.iter().any(|s| s == "|") {
        out.insert(0, "|".into());
    }
    out
}

#[cfg(test)]
mod toolbar_tests {
    use super::*;
    #[test]
    fn profiles_are_valid() {
        for p in PROFILES {
            let t = profile_toolbar(p).unwrap();
            assert_eq!(sanitize_toolbar(&t), t, "{p}");
        }
        let bad = vec!["nope".to_string(), "volume".into(), "volume".into()];
        assert_eq!(
            sanitize_toolbar(&bad),
            vec!["|".to_string(), "volume".into()]
        );
    }
}
