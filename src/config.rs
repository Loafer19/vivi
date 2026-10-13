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
