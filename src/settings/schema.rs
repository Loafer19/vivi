//! Settings schema: one entry per stored setting id with how its value is read from and
//! written to [`Config`]. The row builder reads values through [`get`] and the
//! `setting-changed` handler writes them through [`set`], so an id is mapped in one place.
//! Ids that need more than an assignment (file pickers, rescans, presets) stay in `settings.rs`.

use crate::config::{self, Config};
use crate::mouse;

pub(super) struct Field {
    pub id: &'static str,
    /// Display value for the settings row; `None` while that row still builds its own.
    pub get: Option<fn(&Config) -> String>,
    pub set: fn(&mut Config, &str),
}

fn on(v: &str) -> bool {
    v == "true"
}

/// A number from a slider value or typed text ("6", "6.0", "-3.5 dB").
fn num(v: &str) -> Option<f64> {
    v.split_whitespace()
        .next()
        .and_then(|n| n.replace('−', "-").replace('+', "").parse::<f64>().ok())
        .filter(|n| n.is_finite())
}

/// A gain in dB: half-dB steps within ±15 dB; unreadable input means 0.
fn db(v: &str) -> f64 {
    num(v).map_or(0.0, |n| (n.clamp(-15.0, 15.0) * 2.0).round() / 2.0)
}

/// "0.1×" for a speed step.
pub(super) fn speed_step_label(s: f64) -> String {
    format!("{s}×")
}

/// Stored value ↔ label of choices whose mpv values are not words people use.
pub(super) type Labels = [(&'static str, &'static str)];
pub(super) const AUTO_ALWAYS_OFF: &Labels =
    &[("auto", "Automatic"), ("yes", "Always"), ("no", "Off")];
pub(super) const SUB_AUTO: &Labels = &[
    ("no", "Off"),
    ("exact", "Same name as the video"),
    ("fuzzy", "Names containing the video's name"),
    ("all", "Every subtitle file in the folder"),
];

/// Settings → Interface → Appearance → Panel transparency (#95).
pub(super) const PANEL_GLASS: &Labels = &[
    ("solid", "Solid"),
    ("slight", "Slightly transparent"),
    ("clear", "Transparent"),
];
/// Settings → Interface → Auto-hide (#98).
pub(super) const AUTOHIDE_MODE: &Labels = &[
    ("delay", "After a delay"),
    ("fullscreen", "Only in fullscreen"),
    ("never", "Never"),
];
pub(super) const AUTOHIDE_WAKE: &Labels = &[
    ("move", "Any mouse movement"),
    ("nudge", "Mouse movement, ignoring small bumps"),
    ("edge", "Mouse near the top or bottom edge"),
];
pub(super) const AUTOHIDE_KEYS: &Labels = &[
    ("any", "Any key"),
    ("seek", "Seek and volume keys, briefly"),
    ("none", "No keys"),
];
pub(super) const AUTOHIDE_ANIM: &Labels =
    &[("instant", "Instant"), ("fade", "Fade"), ("slide", "Slide")];

pub(super) const HWDEC: &Labels = &[
    ("auto-safe", "Automatic (safe)"),
    ("auto", "Automatic (any method)"),
    ("vaapi", "VA-API (Linux)"),
    ("nvdec", "NVDEC (NVIDIA)"),
    ("vulkan", "Vulkan"),
    ("videotoolbox", "VideoToolbox (macOS)"),
    ("d3d11va", "Direct3D 11 (Windows)"),
    ("no", "Off (decode on the processor)"),
];
pub(super) const LOOP_FILTER: &Labels = &[
    ("default", "Off"),
    ("nonref", "Light (non-reference frames)"),
    ("bidir", "Medium (B-frames)"),
    ("nonkey", "Strong (all but keyframes)"),
    ("all", "Always (fastest, lowest quality)"),
];
pub(super) const VIDEO_SYNC: &Labels = &[
    ("audio", "Follow the audio"),
    ("display-resample", "Follow the display, adjust the audio"),
    ("display-vdrop", "Follow the display, drop frames"),
    ("desync", "Don't synchronise"),
];
pub(super) const SUB_ENCODING: &Labels = &[
    ("auto", "Detect automatically"),
    ("utf8", "Unicode (UTF-8)"),
    ("cp1251", "Cyrillic (Windows-1251)"),
    ("cp1252", "Western (Windows-1252)"),
    ("latin1", "Western (ISO-8859-1)"),
    ("koi8-u", "Ukrainian (KOI8-U)"),
    ("shift_jis", "Japanese (Shift JIS)"),
    ("gb18030", "Chinese (GB18030)"),
];
pub(super) const SNAPSHOT_FORMAT: &Labels = &[("png", "PNG"), ("jpg", "JPG"), ("webp", "WebP")];

/// mpv's `auto` audio device; the others are shown by their mpv names.
pub(super) const AUDIO_DEVICE: &Labels = &[("auto", "System default")];

/// The labels of a choice, in order.
pub(super) fn labels(t: &Labels) -> Vec<&'static str> {
    t.iter().map(|(_, l)| *l).collect()
}

/// Label of a stored value (unknown values are shown as they are).
fn label(t: &Labels, v: &str) -> String {
    t.iter()
        .find(|(id, _)| *id == v)
        .map_or(v, |(_, l)| *l)
        .into()
}

/// Store a label or a stored value; anything else leaves the setting unchanged.
fn set_labeled(t: &Labels, slot: &mut String, v: &str) {
    if let Some((id, _)) = t.iter().find(|(id, l)| *l == v || *id == v) {
        *slot = (*id).into();
    }
}

fn action_label(id: &str) -> String {
    mouse::label_of(&mouse::ACTIONS, id).into()
}

/// Store a mouse action given by label or id; unknown values leave it unchanged.
fn set_action(slot: &mut String, v: &str) {
    if let Some(id) = mouse::id_of(&mouse::ACTIONS, v) {
        *slot = id.into();
    }
}

/// General category (log file path is handled with its file picker in `settings.rs`).
pub(super) const GENERAL: &[Field] = &[
    Field {
        id: "general.language",
        get: Some(|c| c.general.language.clone()),
        set: |c, v| c.general.language = v.into(),
    },
    Field {
        id: "general.remote_control",
        get: Some(|c| c.general.remote_control.to_string()),
        set: |c, v| c.general.remote_control = on(v),
    },
    Field {
        id: "general.close_button",
        get: Some(|c| c.general.close_button.clone()),
        set: |c, v| c.general.close_button = v.into(),
    },
    Field {
        id: "general.save_log",
        get: Some(|c| c.general.save_log.to_string()),
        set: |c, v| c.general.save_log = on(v),
    },
    Field {
        id: "general.check_updates",
        get: Some(|c| c.general.check_updates.to_string()),
        set: |c, v| c.general.check_updates = on(v),
    },
    Field {
        id: "general.log_level",
        get: Some(|c| c.general.log_level.clone()),
        set: |c, v| c.general.log_level = v.into(),
    },
];

/// The other categories: setters so far; their rows still format the displayed value.
pub(super) const OTHER: &[Field] = &[
    Field {
        id: "interface.accent",
        get: None,
        set: |c, v| c.interface.accent = v.into(),
    },
    Field {
        id: "interface.bar_icons",
        get: Some(|c| crate::config::bar_icons_name(&c.interface.bar_icons).into()),
        set: |c, v| c.interface.bar_icons = crate::config::bar_icons_name(v).into(),
    },
    Field {
        id: "interface.panel_glass",
        get: Some(|c| label(PANEL_GLASS, &c.interface.panel_glass)),
        set: |c, v| set_labeled(PANEL_GLASS, &mut c.interface.panel_glass, v),
    },
    Field {
        id: "interface.autohide",
        get: Some(|c| {
            crate::app::wiring::autohide::delay_secs(c.interface.autohide_secs).to_string()
        }),
        set: |c, v| {
            if let Some(n) = num(v) {
                c.interface.autohide_secs = crate::app::wiring::autohide::delay_secs(n)
            }
        },
    },
    Field {
        id: "interface.autohide_mode",
        get: Some(|c| label(AUTOHIDE_MODE, &c.interface.autohide_mode)),
        set: |c, v| set_labeled(AUTOHIDE_MODE, &mut c.interface.autohide_mode, v),
    },
    Field {
        id: "interface.autohide_top",
        get: Some(|c| c.interface.autohide_top.to_string()),
        set: |c, v| c.interface.autohide_top = on(v),
    },
    Field {
        id: "interface.autohide_bottom",
        get: Some(|c| c.interface.autohide_bottom.to_string()),
        set: |c, v| c.interface.autohide_bottom = on(v),
    },
    Field {
        id: "interface.autohide_cursor",
        get: Some(|c| c.interface.autohide_cursor.to_string()),
        set: |c, v| c.interface.autohide_cursor = on(v),
    },
    Field {
        id: "interface.autohide_keep_paused",
        get: Some(|c| c.interface.autohide_keep_paused.to_string()),
        set: |c, v| c.interface.autohide_keep_paused = on(v),
    },
    Field {
        id: "interface.autohide_keep_panel",
        get: Some(|c| c.interface.autohide_keep_panel.to_string()),
        set: |c, v| c.interface.autohide_keep_panel = on(v),
    },
    Field {
        id: "interface.autohide_keep_hover",
        get: Some(|c| c.interface.autohide_keep_hover.to_string()),
        set: |c, v| c.interface.autohide_keep_hover = on(v),
    },
    Field {
        id: "interface.autohide_keep_audio",
        get: Some(|c| c.interface.autohide_keep_audio.to_string()),
        set: |c, v| c.interface.autohide_keep_audio = on(v),
    },
    Field {
        id: "interface.autohide_wake",
        get: Some(|c| label(AUTOHIDE_WAKE, &c.interface.autohide_wake)),
        set: |c, v| set_labeled(AUTOHIDE_WAKE, &mut c.interface.autohide_wake, v),
    },
    Field {
        id: "interface.autohide_keys",
        get: Some(|c| label(AUTOHIDE_KEYS, &c.interface.autohide_keys)),
        set: |c, v| set_labeled(AUTOHIDE_KEYS, &mut c.interface.autohide_keys, v),
    },
    Field {
        id: "interface.autohide_anim",
        get: Some(|c| label(AUTOHIDE_ANIM, &c.interface.autohide_anim)),
        set: |c, v| set_labeled(AUTOHIDE_ANIM, &mut c.interface.autohide_anim, v),
    },
    Field {
        id: "playback.resume",
        get: None,
        set: |c, v| c.playback.resume = v == "true",
    },
    Field {
        id: "playback.speed",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.playback.speed = (num * 100.0).round() / 100.0
        },
    },
    Field {
        id: "playback.hwdec",
        get: Some(|c| label(HWDEC, &c.playback.hwdec)),
        set: |c, v| set_labeled(HWDEC, &mut c.playback.hwdec, v),
    },
    Field {
        id: "audio.language",
        get: None,
        set: |c, v| c.audio.language = v.trim().into(),
    },
    Field {
        id: "audio.volume_max",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.audio.volume_max = num as i64
        },
    },
    Field {
        id: "subtitles.language",
        get: None,
        set: |c, v| c.subtitles.language = v.trim().into(),
    },
    Field {
        id: "subtitles.scale",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.subtitles.scale = (num * 10.0).round() / 10.0
        },
    },
    Field {
        id: "subtitles.auto_load",
        get: Some(|c| label(SUB_AUTO, &c.subtitles.auto_load)),
        set: |c, v| set_labeled(SUB_AUTO, &mut c.subtitles.auto_load, v),
    },
    Field {
        id: "subtitles.enabled",
        get: None,
        set: |c, v| c.subtitles.enabled = v == "true",
    },
    Field {
        id: "subtitles.encoding",
        get: Some(|c| label(SUB_ENCODING, &c.subtitles.encoding)),
        set: |c, v| set_labeled(SUB_ENCODING, &mut c.subtitles.encoding, v),
    },
    Field {
        id: "subtitles.font",
        get: None,
        set: |c, v| c.subtitles.font = v.into(),
    },
    Field {
        id: "subtitles.bold",
        get: None,
        set: |c, v| c.subtitles.bold = v == "true",
    },
    Field {
        id: "subtitles.color",
        get: None,
        set: |c, v| c.subtitles.color = v.trim().into(),
    },
    Field {
        id: "subtitles.opacity",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.subtitles.opacity = num as i64
        },
    },
    Field {
        id: "subtitles.outline_color",
        get: None,
        set: |c, v| c.subtitles.outline_color = v.trim().into(),
    },
    Field {
        id: "subtitles.shadow",
        get: None,
        set: |c, v| c.subtitles.shadow = v == "true",
    },
    Field {
        id: "subtitles.position",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.subtitles.position = num as i64
        },
    },
    Field {
        id: "playback.repeat",
        get: None,
        set: |c, v| c.playback.repeat = v.into(),
    },
    Field {
        id: "playback.precise_seek",
        get: None,
        set: |c, v| c.playback.precise_seek = v == "true",
    },
    Field {
        id: "playback.sleep_fade",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(30.0);
            c.playback.sleep_fade = (num.round() as i64).clamp(0, 120)
        },
    },
    Field {
        id: "playback.quality",
        get: None,
        set: |c, v| c.playback.quality = v.into(),
    },
    Field {
        id: "controls.short_skip",
        get: None,
        set: |c, v| c.controls.short_skip = super::lead_num(v),
    },
    Field {
        id: "controls.long_skip",
        get: None,
        set: |c, v| c.controls.long_skip = super::lead_num(v),
    },
    Field {
        id: "controls.speed_step",
        get: Some(|c| speed_step_label(c.controls.speed_step)),
        set: |c, v| {
            let n = v
                .trim()
                .trim_end_matches('×')
                .trim()
                .parse::<f64>()
                .unwrap_or(0.0);
            if let Some(s) = crate::config::SPEED_STEPS
                .into_iter()
                .find(|s| (s - n).abs() < 1e-9)
            {
                c.controls.speed_step = s;
            }
        },
    },
    Field {
        id: "controls.mouse_click",
        get: Some(|c| action_label(&c.controls.mouse_click)),
        set: |c, v| set_action(&mut c.controls.mouse_click, v),
    },
    Field {
        id: "controls.mouse_double_click",
        get: Some(|c| action_label(&c.controls.mouse_double_click)),
        set: |c, v| set_action(&mut c.controls.mouse_double_click, v),
    },
    Field {
        id: "controls.mouse_middle",
        get: Some(|c| action_label(&c.controls.mouse_middle)),
        set: |c, v| set_action(&mut c.controls.mouse_middle, v),
    },
    Field {
        id: "controls.mouse_back",
        get: Some(|c| action_label(&c.controls.mouse_back)),
        set: |c, v| set_action(&mut c.controls.mouse_back, v),
    },
    Field {
        id: "controls.mouse_forward",
        get: Some(|c| action_label(&c.controls.mouse_forward)),
        set: |c, v| set_action(&mut c.controls.mouse_forward, v),
    },
    Field {
        id: "controls.wheel",
        get: Some(|c| mouse::label_of(&mouse::WHEEL, &c.controls.wheel).into()),
        set: |c, v| {
            if let Some(id) = mouse::id_of(&mouse::WHEEL, v) {
                c.controls.wheel = id.into();
            }
        },
    },
    Field {
        id: "controls.wheel_reverse",
        get: Some(|c| c.controls.wheel_reverse.to_string()),
        set: |c, v| c.controls.wheel_reverse = on(v),
    },
    Field {
        id: "audio.volume_step",
        get: None,
        set: |c, v| c.audio.volume_step = super::lead_num(v) as i64,
    },
    Field {
        id: "audio.enabled",
        get: None,
        set: |c, v| c.audio.enabled = v == "true",
    },
    Field {
        id: "audio.device",
        get: Some(|c| label(AUDIO_DEVICE, &c.audio.device)),
        set: |c, v| c.audio.device = if v == AUDIO_DEVICE[0].1 { "auto" } else { v }.into(),
    },
    Field {
        id: "audio.startup_volume",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.audio.startup_volume = num as i64
        },
    },
    Field {
        id: "audio.normalize",
        get: None,
        set: |c, v| c.audio.normalize = v == "true",
    },
    Field {
        id: "audio.replaygain",
        get: Some(|c| {
            config::REPLAYGAIN
                .iter()
                .find(|(id, _)| *id == c.audio.replaygain)
                .map_or("Off", |(_, l)| *l)
                .into()
        }),
        set: |c, v| {
            if let Some(id) = config::replaygain_id(v) {
                c.audio.replaygain = id.into();
            }
        },
    },
    Field {
        id: "audio.preamp",
        get: Some(|c| c.audio.preamp.to_string()),
        set: |c, v| c.audio.preamp = db(v),
    },
    Field {
        id: "audio.prevent_clipping",
        get: Some(|c| c.audio.prevent_clipping.to_string()),
        set: |c, v| c.audio.prevent_clipping = on(v),
    },
    Field {
        id: "audio.untagged_gain",
        get: Some(|c| c.audio.untagged_gain.to_string()),
        set: |c, v| c.audio.untagged_gain = db(v),
    },
    Field {
        id: "audio.gapless",
        get: Some(|c| c.audio.gapless.to_string()),
        set: |c, v| c.audio.gapless = on(v),
    },
    Field {
        id: "audio.crossfade_secs",
        get: Some(|c| c.audio.crossfade_secs.to_string()),
        set: |c, v| {
            let n = num(v).unwrap_or(0.0).round() as i64;
            c.audio.crossfade_secs = n.clamp(0, config::CROSSFADE_MAX)
        },
    },
    Field {
        id: "audio.delay",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.audio.delay = (num * 100.0).round() / 100.0
        },
    },
    Field {
        id: "video.enabled",
        get: None,
        set: |c, v| c.video.enabled = v == "true",
    },
    Field {
        id: "video.stop_screensaver",
        get: None,
        set: |c, v| c.video.stop_screensaver = v == "true",
    },
    Field {
        id: "video.hue",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.video.hue = num as i64
        },
    },
    Field {
        id: "video.brightness",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.video.brightness = num as i64
        },
    },
    Field {
        id: "video.contrast",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.video.contrast = num as i64
        },
    },
    Field {
        id: "video.saturation",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.video.saturation = num as i64
        },
    },
    Field {
        id: "video.gamma",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.video.gamma = num as i64
        },
    },
    Field {
        id: "video.deinterlace",
        get: Some(|c| label(AUTO_ALWAYS_OFF, &c.video.deinterlace)),
        set: |c, v| set_labeled(AUTO_ALWAYS_OFF, &mut c.video.deinterlace, v),
    },
    Field {
        id: "video.screenshot_format",
        get: Some(|c| label(SNAPSHOT_FORMAT, &c.video.screenshot_format)),
        set: |c, v| set_labeled(SNAPSHOT_FORMAT, &mut c.video.screenshot_format, v),
    },
    Field {
        id: "network.buffering",
        get: None,
        set: |c, v| c.network.buffering = v.into(),
    },
    Field {
        id: "network.cache",
        get: Some(|c| label(AUTO_ALWAYS_OFF, &c.network.cache)),
        set: |c, v| set_labeled(AUTO_ALWAYS_OFF, &mut c.network.cache, v),
    },
    Field {
        id: "network.cache_secs",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.network.cache_secs = num as i64
        },
    },
    Field {
        id: "network.http_proxy",
        get: None,
        set: |c, v| c.network.http_proxy = v.trim().into(),
    },
    Field {
        id: "network.tls_verify",
        get: None,
        set: |c, v| c.network.tls_verify = v == "true",
    },
    Field {
        id: "network.user_agent",
        get: None,
        set: |c, v| c.network.user_agent = v.into(),
    },
    Field {
        id: "performance.skip_loop_filter",
        get: Some(|c| label(LOOP_FILTER, &c.performance.skip_loop_filter)),
        set: |c, v| set_labeled(LOOP_FILTER, &mut c.performance.skip_loop_filter, v),
    },
    Field {
        id: "performance.threads",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.performance.threads = num as i64
        },
    },
    Field {
        id: "performance.framedrop",
        get: None,
        set: |c, v| c.performance.framedrop = v == "true",
    },
    Field {
        id: "performance.video_sync",
        get: Some(|c| label(VIDEO_SYNC, &c.performance.video_sync)),
        set: |c, v| set_labeled(VIDEO_SYNC, &mut c.performance.video_sync, v),
    },
    Field {
        id: "interface.hint_chapter_thumbnails",
        get: None,
        set: |c, v| c.interface.hint_chapter_thumbnails = v == "true",
    },
    Field {
        id: "library.thumbnails",
        get: None,
        set: |c, v| c.library.thumbnails = v == "true",
    },
];

fn field(id: &str) -> Option<&'static Field> {
    GENERAL.iter().chain(OTHER).find(|f| f.id == id)
}

/// Current value of a schema setting (empty for ids not in the schema).
pub(super) fn get(c: &Config, id: &str) -> String {
    field(id)
        .and_then(|f| f.get)
        .map(|g| g(c))
        .unwrap_or_default()
}

/// Whether `id` is stored through the schema.
pub(super) fn has(id: &str) -> bool {
    field(id).is_some()
}

/// Stores `v` for a schema setting; returns false for ids not in the schema.
pub(super) fn set(c: &mut Config, id: &str, v: &str) -> bool {
    field(id).map(|f| (f.set)(c, v)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        let mut c = Config::default();
        for (id, v) in [
            ("general.language", "English"),
            ("general.remote_control", "true"),
            ("general.save_log", "true"),
            ("general.log_level", "Everything"),
            ("general.check_updates", "true"),
        ] {
            assert!(set(&mut c, id, v), "{id}");
            assert_eq!(get(&c, id), v, "{id}");
        }
        assert!(c.general.remote_control && c.general.save_log && c.general.check_updates);
        assert!(!set(&mut c, "general.nope", "x"));
        assert!(GENERAL.iter().all(|f| has(f.id)));
    }

    #[test]
    fn ids_are_unique_and_setters_accept_input() {
        let mut seen = std::collections::HashSet::new();
        let mut c = Config::default();
        for f in GENERAL.iter().chain(OTHER) {
            assert!(seen.insert(f.id), "duplicate {}", f.id);
            for v in ["true", "42", "2.5", " x ", ""] {
                (f.set)(&mut c, v);
            }
        }
        set(&mut c, "audio.volume_max", "150");
        assert_eq!(c.audio.volume_max, 150);
        set(&mut c, "playback.speed", "1.234");
        assert_eq!(c.playback.speed, 1.23);
        let mut c = Config::default();
        for (id, v) in [
            ("controls.speed_step", "0.25×"),
            ("controls.mouse_click", "Mute"),
            ("controls.mouse_double_click", "Nothing"),
            ("controls.mouse_middle", "Play / pause"),
            ("controls.mouse_back", "Short skip backward"),
            ("controls.mouse_forward", "Next in queue"),
            ("controls.wheel", "Speed"),
            ("controls.wheel_reverse", "true"),
            ("audio.replaygain", "Album"),
            ("audio.preamp", "-3.5"),
            ("audio.prevent_clipping", "false"),
            ("audio.untagged_gain", "-6"),
            ("audio.gapless", "false"),
            ("audio.crossfade_secs", "6"),
            ("interface.panel_glass", "Solid"),
            ("interface.autohide", "0.5"),
            ("interface.autohide_mode", "Only in fullscreen"),
            ("interface.autohide_top", "false"),
            ("interface.autohide_keep_paused", "false"),
            ("interface.autohide_keep_hover", "true"),
            (
                "interface.autohide_wake",
                "Mouse near the top or bottom edge",
            ),
            ("interface.autohide_keys", "Seek and volume keys, briefly"),
            ("interface.autohide_anim", "Fade"),
        ] {
            assert!(set(&mut c, id, v), "{id}");
            assert_eq!(get(&c, id), v, "{id}");
        }
        let k = &c.controls;
        assert_eq!(k.speed_step, 0.25);
        assert_eq!(
            [
                &k.mouse_click,
                &k.mouse_double_click,
                &k.mouse_middle,
                &k.mouse_back,
                &k.mouse_forward
            ],
            ["mute", "none", "play-pause", "skip-back", "next"]
        );
        assert_eq!(k.wheel, "speed");
        // Values that are not choices leave the setting as it was.
        set(&mut c, "controls.speed_step", "0.3");
        set(&mut c, "controls.wheel", "Zoom");
        set(&mut c, "controls.mouse_click", "Explode");
        assert_eq!(
            (c.controls.speed_step, c.controls.wheel.as_str()),
            (0.25, "speed")
        );
        assert_eq!(c.controls.mouse_click, "mute");
        assert_eq!(c.audio.replaygain, "album");
        set(&mut c, "audio.replaygain", "louder");
        set(&mut c, "audio.preamp", "40");
        set(&mut c, "audio.crossfade_secs", "6.4");
        assert_eq!(
            (
                c.audio.replaygain.as_str(),
                c.audio.preamp,
                c.audio.crossfade_secs
            ),
            ("album", 15.0, 6)
        );
        set(&mut c, "audio.crossfade_secs", "99");
        assert_eq!(c.audio.crossfade_secs, config::CROSSFADE_MAX);
        set(&mut c, "subtitles.language", " en ");
        assert_eq!(c.subtitles.language, "en");
        // Choices shown with words store mpv's values; old stored values still work.
        for (id, label, stored) in [
            ("video.deinterlace", "Always", "yes"),
            ("network.cache", "Off", "no"),
            (
                "subtitles.auto_load",
                "Names containing the video's name",
                "fuzzy",
            ),
            ("playback.hwdec", "Off (decode on the processor)", "no"),
            ("performance.skip_loop_filter", "Medium (B-frames)", "bidir"),
            (
                "performance.video_sync",
                "Follow the display, drop frames",
                "display-vdrop",
            ),
            ("subtitles.encoding", "Cyrillic (Windows-1251)", "cp1251"),
            ("video.screenshot_format", "WebP", "webp"),
        ] {
            assert!(set(&mut c, id, label), "{id}");
            assert_eq!(get(&c, id), label, "{id}");
            set(&mut c, id, "bogus");
            assert_eq!(get(&c, id), label, "{id}");
            set(&mut c, id, stored);
            assert_eq!(get(&c, id), label, "{id}");
        }
        assert_eq!(
            (c.video.deinterlace.as_str(), c.network.cache.as_str()),
            ("yes", "no")
        );
        assert_eq!(
            [
                c.playback.hwdec.as_str(),
                c.performance.skip_loop_filter.as_str(),
                c.performance.video_sync.as_str(),
                c.subtitles.encoding.as_str(),
                c.video.screenshot_format.as_str(),
            ],
            ["no", "bidir", "display-vdrop", "cp1251", "webp"]
        );
        set(&mut c, "audio.device", "pulse/hdmi");
        assert_eq!(get(&c, "audio.device"), "pulse/hdmi");
        set(&mut c, "audio.device", "System default");
        assert_eq!(
            (c.audio.device.as_str(), get(&c, "audio.device").as_str()),
            ("auto", "System default")
        );
    }
}
