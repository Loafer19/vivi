//! Settings dialog model, live application of settings and per-show presets (#16).

use crate::config::{self, Config, Preset};
use crate::player::Player;
use crate::{store, AppWindow, SettingRow, Theme};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Seconds of inactivity before the controls hide (read by the auto-hide timer).
pub static AUTOHIDE_SECS: AtomicU32 = AtomicU32::new(2);

const CATEGORIES: [&str; 7] = [
    "Interface",
    "Playback",
    "Audio",
    "Subtitles",
    "Keyboard Shortcuts",
    "Presets",
    "Privacy",
];
const ACCENTS: [&str; 6] = [
    "#D37918", "#E5484D", "#30A46C", "#0090FF", "#8E4EC6", "#E54666",
];

pub const SHORTCUTS: [(&str, &str); 13] = [
    ("Play / pause", "Space  K"),
    ("Seek ±5 s", "←  →"),
    ("Seek ±60 s", "Shift ← →"),
    ("Volume ±5", "↑  ↓"),
    ("Mute", "M"),
    ("Fullscreen", "F  Double-click"),
    ("Leave fullscreen", "Esc"),
    ("Open file", "Ctrl O"),
    ("Add bookmark", "Ctrl B"),
    ("Settings", "Ctrl ,"),
    ("Previous / next in queue", "Buttons"),
    ("Drop files", "Open or enqueue"),
    ("Side panel", "Panel button"),
];

fn hex(s: &str) -> Color {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0xD37918);
    Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

#[allow(clippy::too_many_arguments)]
fn row(id: &str, cat: &str, title: &str, info: &str, kind: &str, value: String) -> SettingRow {
    SettingRow {
        id: id.into(),
        category: cat.into(),
        title: title.into(),
        info: info.into(),
        kind: kind.into(),
        value: value.into(),
        options: ModelRc::default(),
        swatches: ModelRc::default(),
        minimum: 0.0,
        maximum: 1.0,
        step: 1.0,
    }
}
fn opts(mut r: SettingRow, o: &[&str]) -> SettingRow {
    r.options = ModelRc::new(VecModel::from(
        o.iter().map(|s| SharedString::from(*s)).collect::<Vec<_>>(),
    ));
    r
}
fn range(mut r: SettingRow, min: f32, max: f32, step: f32) -> SettingRow {
    r.minimum = min;
    r.maximum = max;
    r.step = step;
    r
}

/// All rows; the bool marks "advanced" rows hidden in basic mode.
fn all_rows(c: &Config) -> Vec<(SettingRow, bool)> {
    let b = |v: bool| v.to_string();
    let mut accent = opts(
        row(
            "interface.accent",
            "Interface",
            "Accent color",
            "Highlight color for sliders, buttons and selection.",
            "color",
            c.interface.accent.clone(),
        ),
        &ACCENTS,
    );
    accent.swatches = ModelRc::new(VecModel::from(
        ACCENTS.iter().map(|a| hex(a)).collect::<Vec<_>>(),
    ));
    let mut v = vec![
        (row("", "Interface", "Appearance", "", "header", String::new()), false),
        (opts(row("interface.theme", "Interface", "Theme", "Dark or light interface colors (Figma light tokens).", "choice", c.interface.theme.clone()), &["dark", "light"]), false),
        (accent, false),
        (range(row("interface.autohide", "Interface", "Hide controls after (s)", "Seconds without mouse movement before bars and cursor hide during playback.", "slider", c.interface.autohide_secs.to_string()), 1.0, 10.0, 1.0), true),
        (row("", "Playback", "Playback", "", "header", String::new()), false),
        (row("playback.resume", "Playback", "Resume where I left off", "Remember the position of every file and continue from it next time.", "toggle", b(c.playback.resume)), false),
        (range(row("playback.speed", "Playback", "Default speed", "Speed applied to every new file.", "slider", format!("{}", c.playback.speed)), 0.25, 3.0, 0.05), false),
        (opts(row("playback.hwdec", "Playback", "Hardware decoding", "auto-safe uses the GPU when it is known to work; no forces software decoding.", "choice", c.playback.hwdec.clone()), &["auto-safe", "auto", "vaapi", "nvdec", "no"]), true),
        (row("", "Audio", "Audio", "", "header", String::new()), false),
        (row("audio.language", "Audio", "Preferred audio language", "Comma-separated language codes in priority order, e.g. uk,en.", "text", c.audio.language.clone()), false),
        (range(row("audio.volume_max", "Audio", "Volume boost cap (%)", "Maximum volume; above 100% amplifies and may distort.", "slider", c.audio.volume_max.to_string()), 100.0, 300.0, 10.0), true),
        (row("", "Subtitles", "Subtitles", "", "header", String::new()), false),
        (row("subtitles.language", "Subtitles", "Preferred subtitle language", "Comma-separated language codes in priority order, e.g. uk,en.", "text", c.subtitles.language.clone()), false),
        (range(row("subtitles.scale", "Subtitles", "Font size", "Subtitle scale factor (1.0 = default).", "slider", format!("{}", c.subtitles.scale)), 0.5, 3.0, 0.1), false),
        (opts(row("subtitles.auto_load", "Subtitles", "Auto-load external files", "Load .srt/.ass files next to the video: exact name, fuzzy match or every file in the folder.", "choice", c.subtitles.auto_load.clone()), &["no", "exact", "fuzzy", "all"]), true),
        (row("", "Keyboard Shortcuts", "Keyboard Shortcuts", "", "header", String::new()), false),
    ];
    for (t, k) in SHORTCUTS {
        v.push((row("", "Keyboard Shortcuts", t, "", "key", k.into()), false));
    }
    v.push((
        row(
            "",
            "Presets",
            "Per-show presets",
            "",
            "header",
            String::new(),
        ),
        false,
    ));
    if c.presets.is_empty() {
        v.push((
            row(
                "",
                "Presets",
                "No presets yet. Save one from the side panel → Subtitles.",
                "",
                "info",
                String::new(),
            ),
            false,
        ));
    }
    for (k, p) in &c.presets {
        let info = format!(
            "sub delay {:+.2}s · audio delay {:+.2}s · sub scale {:.1} · sub pos {} · sub lang {} · audio lang {}",
            p.sub_delay, p.audio_delay, p.sub_scale, p.sub_pos,
            if p.sub_lang.is_empty() { "-" } else { &p.sub_lang },
            if p.audio_lang.is_empty() { "-" } else { &p.audio_lang },
        );
        v.push((
            row(
                &format!("preset.delete:{k}"),
                "Presets",
                k,
                &info,
                "action",
                "Delete".into(),
            ),
            false,
        ));
    }
    v.push((
        row("", "Privacy", "Privacy", "", "header", String::new()),
        false,
    ));
    v.push((
        row(
            "privacy.recent",
            "Privacy",
            "Clear recent files",
            "Forget the list on the start screen.",
            "action",
            "Clear".into(),
        ),
        false,
    ));
    v.push((
        row(
            "privacy.watch_later",
            "Privacy",
            "Clear resume positions",
            "Delete every saved playback position (watch-later).",
            "action",
            "Clear".into(),
        ),
        false,
    ));
    v
}

fn filtered(c: &Config, cat: i32, search: &str, advanced: bool) -> Vec<SettingRow> {
    let q = search.trim().to_lowercase();
    let catname = CATEGORIES.get(cat as usize).copied().unwrap_or("Interface");
    all_rows(c)
        .into_iter()
        .filter(|(_, adv)| advanced || !*adv)
        .filter(|(r, _)| {
            if q.is_empty() {
                r.category == catname
            } else {
                r.kind != "header"
                    && (r.title.to_lowercase().contains(&q)
                        || r.info.to_lowercase().contains(&q)
                        || r.category.to_lowercase().contains(&q))
            }
        })
        .map(|(r, _)| r)
        .collect()
}

pub fn apply_theme(a: &AppWindow, c: &Config) {
    let t = a.global::<Theme>();
    t.set_dark(c.interface.theme != "light");
    t.set_accent(hex(&c.interface.accent));
    AUTOHIDE_SECS.store(c.interface.autohide_secs.max(1), Ordering::Relaxed);
}

/// mpv options driven by settings (applied at startup and on change).
pub fn apply_player(p: &Player, c: &Config) {
    p.set("hwdec", &c.playback.hwdec);
    p.set("speed", &c.playback.speed.to_string());
    p.set(
        "resume-playback",
        if c.playback.resume { "yes" } else { "no" },
    );
    p.set(
        "save-position-on-quit",
        if c.playback.resume { "yes" } else { "no" },
    );
    p.set("alang", &c.audio.language);
    p.set("volume-max", &c.audio.volume_max.to_string());
    p.set("slang", &c.subtitles.language);
    p.set("sub-scale", &c.subtitles.scale.to_string());
    p.set("sub-auto", &c.subtitles.auto_load);
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

fn pick_lang(p: &Player, kind: &str, prop: &str, lang: &str) {
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

fn current_preset(p: &Player) -> Preset {
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

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>, log: bool) {
    app.set_settings_categories(ModelRc::new(VecModel::from(
        CATEGORIES
            .iter()
            .map(|s| SharedString::from(*s))
            .collect::<Vec<_>>(),
    )));
    apply_theme(app, &cfg.borrow());
    apply_player(player, &cfg.borrow());

    let query = Rc::new(RefCell::new((0i32, String::new(), false)));
    let refresh = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        let query = query.clone();
        move || {
            let Some(a) = w.upgrade() else { return };
            let q = query.borrow();
            a.set_settings_rows(ModelRc::new(VecModel::from(filtered(
                &cfg.borrow(),
                q.0,
                &q.1,
                q.2,
            ))));
        }
    };
    let save = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        move || {
            let res = cfg.borrow().save();
            if let Some(a) = w.upgrade() {
                a.set_settings_status(match res {
                    Ok(()) => format!("Saved to {}", config::path().display()).into(),
                    Err(e) => format!("Could not save: {e}").into(),
                });
            }
        }
    };

    {
        let refresh = refresh.clone();
        let query = query.clone();
        app.on_settings_refresh(move |c, s, adv| {
            *query.borrow_mut() = (c, s.to_string(), adv);
            refresh();
        });
    }
    {
        let w = app.as_weak();
        let p = player.clone();
        let cfg = cfg.clone();
        let refresh = refresh.clone();
        let save = save.clone();
        app.on_setting_changed(move |id, value| {
            let Some(a) = w.upgrade() else { return };
            let v = value.as_str();
            let num = v.parse::<f64>().unwrap_or(0.0);
            if log {
                eprintln!("setting {id} = {v}");
            }
            {
                let mut c = cfg.borrow_mut();
                match id.as_str() {
                    "interface.theme" => c.interface.theme = v.into(),
                    "interface.accent" => c.interface.accent = v.into(),
                    "interface.autohide" => c.interface.autohide_secs = num as u32,
                    "playback.resume" => c.playback.resume = v == "true",
                    "playback.speed" => c.playback.speed = (num * 100.0).round() / 100.0,
                    "playback.hwdec" => c.playback.hwdec = v.into(),
                    "audio.language" => c.audio.language = v.trim().into(),
                    "audio.volume_max" => c.audio.volume_max = num as i64,
                    "subtitles.language" => c.subtitles.language = v.trim().into(),
                    "subtitles.scale" => c.subtitles.scale = (num * 10.0).round() / 10.0,
                    "subtitles.auto_load" => c.subtitles.auto_load = v.into(),
                    "privacy.recent" => {
                        a.invoke_clear_recent();
                        a.set_settings_status("Recent files cleared".into());
                        return;
                    }
                    "privacy.watch_later" => {
                        let _ = std::fs::remove_dir_all(store::watch_later_dir());
                        let _ = std::fs::create_dir_all(store::watch_later_dir());
                        a.set_settings_status("Resume positions cleared".into());
                        return;
                    }
                    k if k.starts_with("preset.delete:") => {
                        c.presets.remove(&k["preset.delete:".len()..]);
                    }
                    _ => return,
                }
            }
            apply_theme(&a, &cfg.borrow());
            apply_player(&p, &cfg.borrow());
            save();
            // Text fields re-render on every keystroke otherwise and lose focus.
            if !id.ends_with(".language") {
                refresh();
            }
        });
    }
    {
        let w = app.as_weak();
        let p = player.clone();
        app.on_save_preset(move || {
            let Some(a) = w.upgrade() else { return };
            let Some(path) = p.get("path") else { return };
            let key = config::show_key(&path);
            cfg.borrow_mut()
                .presets
                .insert(key.clone(), current_preset(&p));
            save();
            refresh();
            a.set_resume_text(format!("Preset saved for “{key}”").into());
            if log {
                eprintln!("preset saved: {key} {:?}", cfg.borrow().presets.get(&key));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_spans_categories() {
        let c = Config::default();
        let r = filtered(&c, 0, "language", true);
        assert!(r.len() >= 2 && r.iter().all(|r| r.kind != "header"));
        let basic = filtered(&c, 1, "", false);
        assert!(basic.iter().all(|r| r.id.as_str() != "playback.hwdec"));
        let adv = filtered(&c, 1, "", true);
        assert!(adv.iter().any(|r| r.id.as_str() == "playback.hwdec"));
    }
}
