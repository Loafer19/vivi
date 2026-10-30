//! Applying the config: theme, mpv options, presets, toolbar and the panel sync.

use super::*;

/// Arrow-key steps (read by the key handler in main.rs).
pub static SHORT_SKIP: AtomicU32 = AtomicU32::new(5);
pub static LONG_SKIP: AtomicU32 = AtomicU32::new(60);
pub static VOLUME_STEP: AtomicU32 = AtomicU32::new(5);

/// "system" theme: follow GTK_THEME or the GNOME color-scheme when available.
pub(crate) fn system_dark() -> bool {
    if let Ok(t) = std::env::var("GTK_THEME") {
        return !t.to_lowercase().contains("light") || t.to_lowercase().contains("dark");
    }
    #[cfg(target_os = "linux")]
    if let Ok(o) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        let s = String::from_utf8_lossy(&o.stdout);
        if s.contains("light") {
            return false;
        }
    }
    true
}

pub fn apply_theme(a: &AppWindow, c: &Config) {
    let t = a.global::<Theme>();
    t.set_dark(match c.interface.theme.as_str() {
        "light" => false,
        "system" => system_dark(),
        _ => true,
    });
    t.set_accent(hex(&c.interface.accent));
    t.set_bar_duo(config::bar_icons_name(&c.interface.bar_icons) == "Two-tone");
    a.global::<crate::Settings>()
        .set_accent_name(c.interface.accent.clone().into());
    crate::app::wiring::autohide::configure(a, &c.interface);
    t.set_panel_glass(match c.interface.panel_glass.as_str() {
        "solid" => 0,
        "clear" => 2,
        _ => 1,
    });
    SHORT_SKIP.store(c.controls.short_skip.max(1), Ordering::Relaxed);
    LONG_SKIP.store(c.controls.long_skip.max(1), Ordering::Relaxed);
    VOLUME_STEP.store(c.audio.volume_step.clamp(1, 50) as u32, Ordering::Relaxed);
    crate::mouse::configure(&c.controls);
    a.global::<crate::Panel>()
        .set_pv_speed_step(c.controls.speed_step as f32);
}

pub(crate) fn yes(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// `#RRGGBB` + opacity % → mpv `#AARRGGBB`.
pub(crate) fn mpv_color(rgb: &str, opacity: i64) -> String {
    let h = rgb.trim().trim_start_matches('#');
    let h = if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        h
    } else {
        "FFFFFF"
    };
    format!("#{:02X}{h}", (opacity.clamp(0, 100) * 255 / 100) as u8)
}

pub(crate) fn ytdl_format(q: &str) -> String {
    match q.trim_end_matches('p').parse::<u32>() {
        Ok(h) => format!("bestvideo[height<=?{h}]+bestaudio/best[height<=?{h}]/best"),
        Err(_) if q == "Audio only" => "bestaudio/best".into(),
        Err(_) => "bestvideo+bestaudio/best".into(),
    }
}

/// mpv `replaygain` and `replaygain-fallback` for the audio settings.
pub(crate) fn replaygain(a: &config::Audio) -> (&'static str, f64) {
    match a.replaygain.as_str() {
        "track" => ("track", a.untagged_gain),
        "album" => ("album", a.untagged_gain),
        _ => ("no", 0.0),
    }
}

/// mpv options driven by settings (applied at startup and on change).
pub fn apply_player(p: &Player, c: &Config) {
    // Playback
    crate::player::AUTOPLAY.store(
        c.stub(config::AUTOPLAY),
        std::sync::atomic::Ordering::Relaxed,
    );
    p.set("hwdec", &c.playback.hwdec);
    p.set("speed", &c.playback.speed.to_string());
    p.set("resume-playback", yes(c.playback.resume));
    p.set("save-position-on-quit", yes(c.playback.resume));
    p.set(
        "hr-seek",
        if c.playback.precise_seek {
            "yes"
        } else {
            "default"
        },
    );
    p.set("ytdl-format", &ytdl_format(&c.playback.quality));
    // Network cache: mpv's 150 + 50 MiB defaults are far more than streams need.
    p.set(
        "demuxer-max-bytes",
        &format!("{}MiB", c.playback.cache_mib.clamp(4, 2048)),
    );
    p.set(
        "demuxer-max-back-bytes",
        &format!("{}MiB", c.playback.cache_back_mib.clamp(0, 2048)),
    );
    let (file, list) = match c.playback.repeat.as_str() {
        "One" => ("inf", "no"),
        "All" => ("no", "inf"),
        _ => ("no", "no"),
    };
    p.set("loop-file", file);
    p.set("loop-playlist", list);
    // Video
    p.set("vid", if c.video.enabled { "auto" } else { "no" });
    p.set("stop-screensaver", yes(c.video.stop_screensaver));
    p.set("hue", &c.video.hue.to_string());
    p.set("brightness", &c.video.brightness.to_string());
    p.set("contrast", &c.video.contrast.to_string());
    p.set("saturation", &c.video.saturation.to_string());
    p.set("gamma", &c.video.gamma.to_string());
    p.set("deinterlace", &c.video.deinterlace);
    p.set("video-aspect-override", &c.video.aspect);
    if !c.video.screenshot_dir.is_empty() {
        p.set("screenshot-directory", &c.video.screenshot_dir);
    }
    p.set("screenshot-format", &c.video.screenshot_format);
    // Audio
    p.set("aid", if c.audio.enabled { "auto" } else { "no" });
    p.set("audio-device", &c.audio.device);
    p.set("alang", &c.audio.language);
    p.set("volume-max", &c.audio.volume_max.to_string());
    p.set("audio-delay", &c.audio.delay.to_string());
    // Labelled filter, so other audio filters are left alone.
    let has_norm = p.get("af").is_some_and(|f| f.contains("vivi-norm"));
    if has_norm && !c.audio.normalize {
        p.cmd("af", &["remove", "@vivi-norm"]);
    }
    if c.audio.normalize && !has_norm {
        p.cmd("af", &["add", "@vivi-norm:lavfi=[dynaudnorm]"]);
    }
    // Volume leveling (#88). mpv applies replaygain-fallback even with replaygain=no,
    // so it is only set while ReplayGain is on.
    let (rg, fallback) = replaygain(&c.audio);
    p.set("replaygain", rg);
    p.set("replaygain-preamp", &c.audio.preamp.to_string());
    p.set("replaygain-clip", yes(!c.audio.prevent_clipping));
    p.set("replaygain-fallback", &fallback.to_string());
    crate::transitions::apply(p, c);
    // Subtitles
    p.set("sub-visibility", yes(c.subtitles.enabled));
    p.set("slang", &c.subtitles.language);
    p.set("sub-scale", &c.subtitles.scale.to_string());
    p.set("sub-auto", &c.subtitles.auto_load);
    p.set("sub-codepage", &c.subtitles.encoding);
    if !c.subtitles.font.trim().is_empty() {
        p.set("sub-font", c.subtitles.font.trim());
    }
    p.set("sub-bold", yes(c.subtitles.bold));
    p.set(
        "sub-color",
        &mpv_color(&c.subtitles.color, c.subtitles.opacity),
    );
    p.set("sub-border-size", &c.subtitles.outline.to_string());
    p.set(
        "sub-border-color",
        &mpv_color(&c.subtitles.outline_color, 100),
    );
    p.set(
        "sub-shadow-offset",
        if c.subtitles.shadow { "2" } else { "0" },
    );
    p.set("sub-pos", &c.subtitles.position.to_string());
    // Network
    let bytes = match c.network.buffering.as_str() {
        "Low" => "32MiB",
        "High" => "512MiB",
        _ => "150MiB",
    };
    p.set("demuxer-max-bytes", bytes);
    p.set("cache", &c.network.cache);
    p.set("cache-secs", &c.network.cache_secs.to_string());
    p.set("http-proxy", c.network.http_proxy.trim());
    p.set("tls-verify", yes(c.network.tls_verify));
    if !c.network.user_agent.trim().is_empty() {
        p.set("user-agent", c.network.user_agent.trim());
    }
    // Performance
    p.set("vd-lavc-threads", &c.performance.threads.to_string());
    p.set("vd-lavc-skiploopfilter", &c.performance.skip_loop_filter);
    p.set(
        "framedrop",
        if c.performance.framedrop { "vo" } else { "no" },
    );
    p.set("video-sync", &c.performance.video_sync);
    // Diagnostics
    if c.general.save_log && !c.general.log_file.is_empty() {
        let lvl = match c.general.log_level.as_str() {
            "Warnings" => "all=warn",
            "Everything" => "all=v",
            _ => "all=error",
        };
        p.set("log-file", &c.general.log_file);
        p.set("msg-level", lvl);
    } else {
        p.set("log-file", "");
    }
}

/// Apply a saved preset for the current file's show, if any.
pub fn apply_preset(p: &Player, cfg: &Config, log: bool) -> Option<String> {
    let path = p.get("path")?;
    let key = config::show_key(&path);
    let pr = cfg.presets.get(&key)?;
    p.set("sub-delay", &pr.sub_delay.to_string());
    p.set("audio-delay", &pr.audio_delay.to_string());
    if pr.sub_scale > 0.0 {
        p.set("sub-scale", &pr.sub_scale.to_string());
    }
    p.set("sub-pos", &pr.sub_pos.to_string());
    pick_lang(p, "sub", "sid", &pr.sub_lang);
    pick_lang(p, "audio", "aid", &pr.audio_lang);
    if log {
        eprintln!("preset applied: {key}");
    }
    Some(key)
}

pub(crate) fn pick_lang(p: &Player, kind: &str, prop: &str, lang: &str) {
    if lang.is_empty() {
        return;
    }
    let n: i64 = p.mpv.get_property("track-list/count").unwrap_or(0);
    for i in 0..n {
        let g = |f: &str| p.get(&format!("track-list/{i}/{f}")).unwrap_or_default();
        if g("type") == kind && g("lang") == lang {
            p.set(prop, &g("id"));
            return;
        }
    }
}

pub(crate) fn current_preset(p: &Player) -> Preset {
    let f = |k: &str| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let lang = |prop: &str| {
        p.get(&format!("current-tracks/{prop}/lang"))
            .unwrap_or_default()
    };
    Preset {
        sub_delay: f("sub-delay"),
        audio_delay: f("audio-delay"),
        sub_scale: f("sub-scale"),
        sub_pos: f("sub-pos") as i64,
        sub_lang: lang("sub"),
        audio_lang: lang("audio"),
    }
}

pub(crate) use crate::util::strings;

/// Push the configured bottom-bar layout to the UI.
pub fn apply_toolbar(a: &AppWindow, c: &Config) {
    let buttons = plugin_buttons();
    let t = visible_toolbar(&c.interface.toolbar, &buttons);
    // Cumulative width (in ~32 px slots) of the items that may fold into "⋯"; -1 = always shown.
    let mut used = 0;
    let cum: Vec<i32> = t
        .iter()
        .map(|id| {
            if config::CORE_ITEMS.contains(&id.as_str()) {
                -1
            } else {
                used += if id == "volume" { 3 } else { 1 };
                used
            }
        })
        .collect();
    let meta = |id: &str| {
        buttons
            .iter()
            .find(|(k, _)| k == id)
            .map_or_else(crate::ToolMeta::default, |(_, m)| m.clone())
    };
    let metas: Vec<crate::ToolMeta> = t.iter().map(|id| meta(id)).collect();
    a.global::<crate::Customize>()
        .set_bar_meta(ModelRc::new(VecModel::from(metas)));
    a.global::<crate::Customize>()
        .set_bar_items(strings(t.iter().cloned()));
    a.global::<crate::Customize>()
        .set_bar_cum(ModelRc::new(VecModel::from(cum)));
    a.global::<crate::Customize>().set_bar_slots(used);
    a.global::<crate::Customize>()
        .set_bar_style(bar_style(&c.interface.bar_style));
    a.global::<crate::Customize>()
        .set_bar_styles(strings(config::BAR_STYLES));
    a.global::<crate::Customize>()
        .set_vol_style(volume_style(&c.interface.volume_style));
    a.global::<crate::Customize>()
        .set_vol_styles(strings(config::VOLUME_STYLES));
    a.global::<crate::Customize>()
        .set_profile(c.interface.profile.clone().into());
    // Built-in items, then plugin buttons (off until added here).
    let avail: Vec<String> = config::TOOL_ITEMS
        .iter()
        .filter(|i| **i == "spacer" || !t.iter().any(|s| s == *i))
        .map(|s| s.to_string())
        .chain(
            buttons
                .iter()
                .filter(|(k, _)| !t.contains(k))
                .map(|(k, _)| k.clone()),
        )
        .collect();
    let avail_meta: Vec<crate::ToolMeta> = avail.iter().map(|id| meta(id)).collect();
    a.global::<crate::Customize>()
        .set_available_meta(ModelRc::new(VecModel::from(avail_meta)));
    a.global::<crate::Customize>()
        .set_available(strings(avail.iter().cloned()));
    a.global::<crate::Playback>()
        .set_volume_max(c.audio.volume_max as f32);
}

/// Bottom-bar buttons enabled plugins contribute (`controlbar.button`): item id and
/// its label / icon key.
pub fn plugin_buttons() -> Vec<(String, crate::ToolMeta)> {
    crate::plugins::contributions(crate::plugins::contrib::CONTROLBAR_BUTTON)
        .into_iter()
        .map(|e| {
            (
                format!("{}{}|{}", config::PLUGIN_ITEM, e.id, e.command),
                crate::ToolMeta {
                    label: e.label.into(),
                    icon: e.icon.into(),
                },
            )
        })
        .collect()
}

/// The configured bar without buttons of plugins that are off or gone.
pub fn visible_toolbar(t: &[String], buttons: &[(String, crate::ToolMeta)]) -> Vec<String> {
    config::sanitize_toolbar(t)
        .into_iter()
        .filter(|s| !s.starts_with(config::PLUGIN_ITEM) || buttons.iter().any(|(k, _)| k == s))
        .collect()
}

/// Name of the built-in profile matching the bar, else "Custom".
pub fn profile_of(i: &config::Interface) -> String {
    config::PROFILES
        .iter()
        .find(|p| {
            config::profile_toolbar(p).as_ref() == Some(&i.toolbar)
                && config::profile_style(p) == i.bar_style
                && config::profile_volume(p) == i.volume_style
        })
        .map_or("Custom".into(), |p| p.to_string())
}

/// Mirror mpv's current values into the side-panel sliders.
pub fn sync_panel(a: &AppWindow, p: &Player) {
    let f = |k: &str, d: f64| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d) as f32;
    a.global::<crate::Panel>()
        .set_pv_sub_delay(f("sub-delay", 0.0));
    a.global::<crate::Panel>()
        .set_pv_sub_scale(f("sub-scale", 1.0));
    a.global::<crate::Panel>()
        .set_pv_sub_pos(f("sub-pos", 100.0));
    a.global::<crate::Panel>()
        .set_pv_audio_delay(f("audio-delay", 0.0));
    a.global::<crate::Panel>().set_pv_boost(f("volume", 100.0));
    a.global::<crate::Panel>()
        .set_pv_rotate(f("video-rotate", 0.0) as i32);
    a.global::<crate::Panel>().set_pv_zoom(f("video-zoom", 0.0));
    a.global::<crate::Panel>()
        .set_pv_panscan(f("panscan", 0.0) * 100.0);
    a.global::<crate::Panel>()
        .set_pv_brightness(f("brightness", 0.0));
    a.global::<crate::Panel>()
        .set_pv_contrast(f("contrast", 0.0));
    a.global::<crate::Panel>()
        .set_pv_saturation(f("saturation", 0.0));
    a.global::<crate::Panel>().set_pv_gamma(f("gamma", 0.0));
    a.global::<crate::Panel>().set_pv_speed(f("speed", 1.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaygain_fallback_only_while_on() {
        let mut a = config::Audio {
            untagged_gain: -6.0,
            ..Default::default()
        };
        // mpv applies replaygain-fallback even with replaygain=no.
        assert_eq!(replaygain(&a), ("no", 0.0));
        a.replaygain = "album".into();
        assert_eq!(replaygain(&a), ("album", -6.0));
        a.replaygain = "track".into();
        assert_eq!(replaygain(&a), ("track", -6.0));
    }
}
