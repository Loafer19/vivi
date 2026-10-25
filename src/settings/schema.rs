//! Settings schema: one entry per stored setting id with how its value is read from and
//! written to [`Config`]. The row builder reads values through [`get`] and the
//! `setting-changed` handler writes them through [`set`], so an id is mapped in one place.
//! Ids that need more than an assignment (file pickers, rescans, presets) stay in `settings.rs`.

use crate::config::Config;

pub(super) struct Field {
    pub id: &'static str,
    /// Display value for the settings row; `None` while that row still builds its own.
    pub get: Option<fn(&Config) -> String>,
    pub set: fn(&mut Config, &str),
}

fn on(v: &str) -> bool {
    v == "true"
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
        id: "interface.autohide",
        get: None,
        set: |c, v| {
            let num = v.parse::<f64>().unwrap_or(0.0);
            c.interface.autohide_secs = num as u32
        },
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
        get: None,
        set: |c, v| c.playback.hwdec = v.into(),
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
        get: None,
        set: |c, v| c.subtitles.auto_load = v.into(),
    },
    Field {
        id: "subtitles.enabled",
        get: None,
        set: |c, v| c.subtitles.enabled = v == "true",
    },
    Field {
        id: "subtitles.encoding",
        get: None,
        set: |c, v| c.subtitles.encoding = v.into(),
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
        get: None,
        set: |c, v| c.audio.device = v.into(),
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
        get: None,
        set: |c, v| c.video.deinterlace = v.into(),
    },
    Field {
        id: "video.screenshot_format",
        get: None,
        set: |c, v| c.video.screenshot_format = v.into(),
    },
    Field {
        id: "network.buffering",
        get: None,
        set: |c, v| c.network.buffering = v.into(),
    },
    Field {
        id: "network.cache",
        get: None,
        set: |c, v| c.network.cache = v.into(),
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
        get: None,
        set: |c, v| c.performance.skip_loop_filter = v.into(),
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
        get: None,
        set: |c, v| c.performance.video_sync = v.into(),
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
        ] {
            assert!(set(&mut c, id, v), "{id}");
            assert_eq!(get(&c, id), v, "{id}");
        }
        assert!(c.general.remote_control && c.general.save_log);
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
        set(&mut c, "subtitles.language", " en ");
        assert_eq!(c.subtitles.language, "en");
    }
}
