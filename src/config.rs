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
    /// Progress bar style: "Compact" | "Classic" | "Waveform" | "Plain" (see `BAR_STYLES`).
    pub bar_style: String,
    /// Volume control style: "Cone" | "Bar" | "Popup" | "Percent" (see `VOLUME_STYLES`).
    pub volume_style: String,
    /// Player top and bottom bar icons: "Two-tone" | "Single-tone" (see `BAR_ICONS`).
    pub bar_icons: String,
    /// Offer the Chapter Thumbnails package (seek-bar frame previews) above the seek bar;
    /// "Don't show again" turns it off.
    pub hint_chapter_thumbnails: bool,
    /// 0.11.0 settings, read once and folded into `toolbar` / `bar_style` by `migrate`.
    #[serde(skip_serializing)]
    pub bar_layout: Option<String>,
    #[serde(skip_serializing)]
    pub skip_buttons: Option<bool>,
    #[serde(skip_serializing)]
    pub waveform: Option<bool>,
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
    /// mpv `gpu-hwdec-interop`: "auto" lets vivi pick the one for the GPU it renders on
    /// (VA-API on Mesa, CUDA on NVIDIA); any other value is passed to mpv as is.
    pub hwdec_interop: String,
    /// Network read-ahead cache in MiB (mpv `demuxer-max-bytes`; mpv's own default is 150).
    pub cache_mib: i64,
    /// Already-played network data kept for seeking back, in MiB (`demuxer-max-back-bytes`).
    pub cache_back_mib: i64,
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
    /// Local control socket for `vivi-mcp` and other tools (see `src/ipc.rs`).
    pub remote_control: bool,
    /// Settings → About: query GitHub for a newer release on startup (off by default).
    pub check_updates: bool,
    /// Window close button: "Minimize to tray" (keeps playing; only while a system tray
    /// is available, see `src/tray.rs`) or "Quit".
    pub close_button: String,
}

/// Choices of `general.close_button`.
pub const CLOSE_BUTTON: [&str; 2] = ["Minimize to tray", "Quit"];

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
            remote_control: false,
            check_updates: false,
            close_button: CLOSE_BUTTON[0].into(),
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
            bar_style: "Compact".into(),
            volume_style: "Cone".into(),
            bar_icons: "Two-tone".into(),
            hint_chapter_thumbnails: true,
            bar_layout: None,
            skip_buttons: None,
            waveform: None,
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
            hwdec_interop: "auto".into(),
            cache_mib: 48,
            cache_back_mib: 16,
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
            Ok(s) => toml::from_str::<Self>(&s)
                .map(Self::migrated)
                .unwrap_or_else(|e| {
                    eprintln!("vivi: invalid {}: {e}; using defaults", p.display());
                    let _ = std::fs::copy(&p, p.with_extension("toml.bak"));
                    Self::default()
                }),
            Err(_) => Self::default(),
        }
    }

    /// Fold older bottom-bar settings into the item list and bar style.
    fn migrated(mut self) -> Self {
        let i = &mut self.interface;
        migrate_toolbar(i);
        self
    }

    /// Write config.toml owner-only (0600 on Unix): package settings in it can be API keys,
    /// tokens or session keys. Atomic (temp file + rename).
    pub fn save(&self) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::store::write_private(&path(), text.as_bytes())
    }
}

/// Normalized show name from a file name: "The_Wand_S01E05.mp4" → "the wand".
pub fn show_key(path: &str) -> String {
    let name = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Release tags in square brackets ("[SubGroup]", "[1080p]", yt-dlp's "[video id]") and
    // an episode number after " - " ("Show - 05", "Show - 12v2") are not the show's name.
    let normalize = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    ' '
                }
            })
            .collect()
    };
    let mut short = strip_square_tags(&name);
    if let Some(i) = dash_episode(&short) {
        short.truncate(i);
    }
    let mut norm = normalize(&short);
    if norm.trim().is_empty() {
        norm = normalize(&name);
    }
    let words: Vec<&str> = norm.split_whitespace().collect();
    // Cut at the first episode marker like s01e05, s01, e05, 1x05.
    let digits = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit());
    let is_marker = |i: usize, w: &str| {
        let b = w.as_bytes();
        // "Episode 5", "Ep 5", "Ep05".
        ((w == "episode" || w == "ep") && words.get(i + 1).is_some_and(|n| digits(n)))
            || w.strip_prefix("ep").is_some_and(digits)
            || (b.len() >= 2 && (b[0] == b's' || b[0] == b'e') && b[1].is_ascii_digit())
            || (w.contains('x')
                && w.split('x')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())))
    };
    let end = words
        .iter()
        .enumerate()
        .position(|(i, w)| is_marker(i, w))
        .unwrap_or(words.len());
    let key = words[..end].join(" ");
    if key.is_empty() {
        norm.trim().to_owned()
    } else {
        key
    }
}

/// `name` without `[...]` parts (unbalanced brackets are kept).
fn strip_square_tags(name: &str) -> String {
    let mut out = String::new();
    let mut rest = name;
    while let Some(a) = rest.find('[') {
        let Some(b) = rest[a..].find(']') else { break };
        out.push_str(&rest[..a]);
        out.push(' ');
        rest = &rest[a + b + 1..];
    }
    out.push_str(rest);
    out
}

/// Byte index of " - <1-4 digits>" (optionally "v2") followed by nothing alphanumeric.
fn dash_episode(name: &str) -> Option<usize> {
    name.match_indices(" - ").find_map(|(i, _)| {
        let after = &name[i + 3..];
        let n = after.chars().take_while(|c| c.is_ascii_digit()).count();
        let tail = &after[n..];
        let tail = tail
            .strip_prefix('v')
            .map(|t| t.trim_start_matches(|c: char| c.is_ascii_digit()))
            .unwrap_or(tail);
        ((1..=4).contains(&n) && !tail.starts_with(char::is_alphanumeric)).then_some(i)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn show_keys() {
        assert_eq!(
            show_key("[SubsPlease] Frieren - 05 (1080p) [ABCD1234].mkv"),
            "frieren"
        );
        assert_eq!(show_key("Frieren - 12v2.mkv"), "frieren");
        assert_eq!(show_key("Show Episode 5.mkv"), "show");
        assert_eq!(show_key("Show Ep05.mkv"), "show");
        assert_eq!(
            show_key("Big Buck Bunny (chapters) [aqz-KE-bpKQ].mkv"),
            "big buck bunny chapters"
        );
        assert_eq!(show_key("The Wand - Pilot.mkv"), "the wand pilot");
        assert_eq!(show_key("[Only Tags].mkv"), "only tags");
        assert_eq!(show_key("Epic Movie.mkv"), "epic movie");
        assert_eq!(
            show_key("Spider-Man - Homecoming.mkv"),
            "spider man homecoming"
        );
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
pub const TOOL_ITEMS: [&str; 23] = [
    "prev",
    "back10",
    "play",
    "fwd10",
    "next",
    "time",
    "seek",
    "duration",
    "spacer",
    "queue",
    "subtitles",
    "audio",
    "volume",
    "video",
    "speed",
    "chapters",
    "bookmark",
    "stop",
    "snapshot",
    "repeat",
    "shuffle",
    "settings",
    "fullscreen",
];

/// Items that never fold into the "⋯" menu on narrow windows.
pub const CORE_ITEMS: [&str; 7] = ["prev", "play", "next", "time", "seek", "duration", "spacer"];

pub const BAR_STYLES: [&str; 4] = ["Compact", "Classic", "Waveform", "Plain"];

/// The canonical [`BAR_STYLES`] name for a config value, ignoring case
/// ("classic" → "Classic"); unknown values fall back to "Compact".
pub fn bar_style_name(s: &str) -> &'static str {
    BAR_STYLES
        .into_iter()
        .find(|b| b.eq_ignore_ascii_case(s.trim()))
        .unwrap_or("Compact")
}

/// Volume control styles: the cone wedge, a flat slider like the seek track, an icon
/// with a pop-up vertical slider, and an icon with the percentage.
pub const VOLUME_STYLES: [&str; 4] = ["Cone", "Bar", "Popup", "Percent"];

/// The canonical [`VOLUME_STYLES`] name, ignoring case; unknown values fall back to "Cone".
pub fn volume_style_name(s: &str) -> &'static str {
    VOLUME_STYLES
        .into_iter()
        .find(|b| b.eq_ignore_ascii_case(s.trim()))
        .unwrap_or("Cone")
}

/// Icon styles of the player's top and bottom bars (other icons are always two-tone).
pub const BAR_ICONS: [&str; 2] = ["Two-tone", "Single-tone"];

/// The canonical [`BAR_ICONS`] name, ignoring case; unknown values fall back to "Two-tone".
pub fn bar_icons_name(s: &str) -> &'static str {
    BAR_ICONS
        .into_iter()
        .find(|b| b.eq_ignore_ascii_case(s.trim()))
        .unwrap_or("Two-tone")
}

/// Volume style that goes with a built-in profile (all keep the cone for now).
pub fn profile_volume(_name: &str) -> &'static str {
    "Cone"
}

pub const PROFILES: [&str; 3] = ["Minimal", "Default", "Power"];

/// Built-in toolbar layouts.
pub fn profile_toolbar(name: &str) -> Option<Vec<String>> {
    let v: &[&str] = match name {
        "Minimal" => &["play", "time", "seek", "duration", "volume", "fullscreen"],
        // The compact reference layout.
        "Default" => &[
            "prev",
            "play",
            "next",
            "time",
            "seek",
            "duration",
            "volume",
            "audio",
            "subtitles",
            "settings",
            "queue",
            "fullscreen",
        ],
        "Power" => &[
            "queue",
            "prev",
            "back10",
            "play",
            "fwd10",
            "next",
            "time",
            "seek",
            "duration",
            "chapters",
            "bookmark",
            "snapshot",
            "repeat",
            "shuffle",
            "speed",
            "subtitles",
            "audio",
            "video",
            "volume",
            "settings",
            "fullscreen",
        ],
        _ => return None,
    };
    Some(v.iter().map(|s| s.to_string()).collect())
}

/// Bar style that goes with a built-in profile.
pub fn profile_style(name: &str) -> &'static str {
    match name {
        "Minimal" => "Plain",
        "Power" => "Waveform",
        _ => "Compact",
    }
}

/// Prefix of a plugin's bottom-bar button (`plug:<plugin id>|<command>`, `controlbar.button`).
pub const PLUGIN_ITEM: &str = "plug:";

/// Known items (and plugin buttons) in order, no duplicates (except spacers).
pub fn sanitize_toolbar(v: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        let ok = TOOL_ITEMS.contains(&s.as_str())
            || s.strip_prefix(PLUGIN_ITEM).is_some_and(|r| r.contains('|'));
        if ok && (s == "spacer" || !out.contains(s)) {
            out.push(s.clone());
        }
    }
    out
}

/// 0.11.0 and older: `|` stood for the fixed centre transport, and the bar layout,
/// skip buttons and waveform were separate settings.
pub fn migrate_toolbar(i: &mut Interface) {
    if i.toolbar.iter().any(|s| s == "|") {
        let skip = i.skip_buttons.unwrap_or(true);
        let had = |id: &str| i.toolbar.iter().any(|s| s == id);
        let (b10, f10) = (skip || had("back10"), skip || had("fwd10"));
        let mut core: Vec<String> = vec!["prev".into()];
        if b10 {
            core.push("back10".into());
        }
        core.push("play".into());
        if f10 {
            core.push("fwd10".into());
        }
        core.extend(["next", "time", "seek", "duration"].map(String::from));
        let mut t: Vec<String> = Vec::new();
        for s in &i.toolbar {
            if s == "|" {
                t.extend(core.iter().cloned());
            } else if !(s == "back10" || s == "fwd10") {
                t.push(s.clone());
            }
        }
        i.toolbar = sanitize_toolbar(&t);
        if i.profile != "Custom" {
            if let Some(p) = profile_toolbar(&i.profile) {
                i.toolbar = p;
            }
        }
    }
    if let Some(l) = i.bar_layout.take() {
        i.bar_style = if l == "Classic" {
            "Classic".into()
        } else if i.waveform == Some(false) {
            "Compact".into()
        } else {
            "Waveform".into()
        };
    }
    i.skip_buttons = None;
    i.waveform = None;
    i.bar_style = bar_style_name(&i.bar_style).into();
    i.volume_style = volume_style_name(&i.volume_style).into();
    i.bar_icons = bar_icons_name(&i.bar_icons).into();
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
        assert_eq!(sanitize_toolbar(&bad), vec!["volume".to_string()]);
        // Plugin buttons (`controlbar.button`) are kept once; a malformed one is dropped.
        let plug = vec![
            "plug:lyrics|karaoke".to_string(),
            "plug:lyrics|karaoke".into(),
            "plug:broken".into(),
        ];
        assert_eq!(
            sanitize_toolbar(&plug),
            vec!["plug:lyrics|karaoke".to_string()]
        );
    }
    #[test]
    fn migrates_old_bar() {
        let mut i = Interface {
            profile: "Custom".into(),
            toolbar: ["queue", "|", "volume"].map(String::from).to_vec(),
            bar_layout: Some("Classic".into()),
            skip_buttons: Some(false),
            ..Default::default()
        };
        migrate_toolbar(&mut i);
        assert_eq!(
            i.toolbar,
            ["queue", "prev", "play", "next", "time", "seek", "duration", "volume"]
                .map(String::from)
                .to_vec()
        );
        assert_eq!(i.bar_style, "Classic");
        let mut d = Interface {
            toolbar: vec!["|".into()],
            bar_layout: Some("Compact".into()),
            ..Default::default()
        };
        migrate_toolbar(&mut d);
        assert_eq!(d.toolbar, profile_toolbar("Default").unwrap());
        assert_eq!(d.bar_style, "Waveform");
    }

    #[test]
    fn volume_style_names_are_case_insensitive() {
        assert_eq!(volume_style_name("popup"), "Popup");
        assert_eq!(volume_style_name(" PERCENT "), "Percent");
        assert_eq!(volume_style_name("bogus"), "Cone");
        assert_eq!(volume_style_name(""), "Cone");
        let mut i = Interface {
            volume_style: "bar".into(),
            ..Default::default()
        };
        migrate_toolbar(&mut i);
        assert_eq!(i.volume_style, "Bar");
        // Older configs have no volume_style: serde default → Cone.
        let i: Interface = toml::from_str("bar_style = \"Plain\"").unwrap();
        assert_eq!(i.volume_style, "Cone");
    }

    #[test]
    fn chapter_thumbnails_hint_defaults_on() {
        // Older configs have no key: the hint is offered.
        let i: Interface = toml::from_str("theme = \"dark\"").unwrap();
        assert!(i.hint_chapter_thumbnails);
        let i: Interface = toml::from_str("hint_chapter_thumbnails = false").unwrap();
        assert!(!i.hint_chapter_thumbnails);
    }

    #[test]
    fn bar_icons_default_and_names() {
        assert_eq!(Interface::default().bar_icons, "Two-tone");
        assert_eq!(bar_icons_name("single-tone"), "Single-tone");
        assert_eq!(bar_icons_name("bogus"), "Two-tone");
        // Older configs have no bar_icons: serde default → Two-tone.
        let i: Interface = toml::from_str("bar_style = \"Plain\"").unwrap();
        assert_eq!(i.bar_icons, "Two-tone");
        let mut i = Interface {
            bar_icons: "SINGLE-TONE".into(),
            ..Interface::default()
        };
        migrate_toolbar(&mut i);
        assert_eq!(i.bar_icons, "Single-tone");
    }
    #[test]
    fn bar_style_names_are_case_insensitive() {
        assert_eq!(bar_style_name("classic"), "Classic");
        assert_eq!(bar_style_name(" PLAIN "), "Plain");
        assert_eq!(bar_style_name("Waveform"), "Waveform");
        assert_eq!(bar_style_name("bogus"), "Compact");
        let mut i = Interface {
            bar_style: "plain".into(),
            ..Default::default()
        };
        migrate_toolbar(&mut i);
        assert_eq!(i.bar_style, "Plain");
    }
}
