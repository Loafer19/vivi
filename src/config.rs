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
    /// Per-show presets keyed by normalized show name (see [`show_key`]).
    pub presets: BTreeMap<String, Preset>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Playback {
    pub resume: bool,
    pub hwdec: String,
    pub speed: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Audio {
    /// Preferred languages, mpv `alang` syntax (e.g. "uk,en").
    pub language: String,
    pub volume_max: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Subtitles {
    pub language: String,
    pub scale: f64,
    /// mpv `sub-auto`: "no" | "exact" | "fuzzy" | "all"
    pub auto_load: String,
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
}

impl Default for Library {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
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
        }
    }
}
impl Default for Playback {
    fn default() -> Self {
        Self {
            resume: true,
            hwdec: "auto-safe".into(),
            speed: 1.0,
        }
    }
}
impl Default for Audio {
    fn default() -> Self {
        Self {
            language: String::new(),
            volume_max: 150,
        }
    }
}
impl Default for Subtitles {
    fn default() -> Self {
        Self {
            language: String::new(),
            scale: 1.0,
            auto_load: "fuzzy".into(),
        }
    }
}

pub fn path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".config")
        })
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
