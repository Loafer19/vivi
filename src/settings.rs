//! Settings dialog model, live application of settings and per-show presets (#16).

use crate::config::{self, Config, Preset};
use crate::player::Player;
use crate::{store, AppWindow, SettingRow, Theme};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Seconds of inactivity before the controls hide (read by the auto-hide timer).
pub static AUTOHIDE_SECS: AtomicU32 = AtomicU32::new(2);

const CATEGORIES: [&str; 8] = [
    "Interface",
    "Playback",
    "Audio",
    "Subtitles",
    "Keyboard Shortcuts",
    "Presets",
    "Privacy",
    "Library",
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
        (opts(row("interface.profile", "Interface", "Toolbar profile", "Which buttons appear in the bottom bar. Minimal, Default and Power are built in; Custom is your own layout.", "choice", c.interface.profile.clone()), &["Minimal", "Default", "Power", "Custom"]), false),
        (row("interface.customize", "Interface", "Customize player", "Choose, order and remove bottom-bar buttons.", "action", "Customize…".into()), false),
        (row("interface.onboarding", "Interface", "Welcome tour", "Show the first-run onboarding again.", "action", "Show".into()), true),
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
        row(
            "",
            "Library",
            "Library folders",
            "",
            "header",
            String::new(),
        ),
        false,
    ));
    if c.library.folders.is_empty() {
        v.push((
            row("", "Library", "No folders yet.", "", "info", String::new()),
            false,
        ));
    }
    for f in &c.library.folders {
        v.push((
            row(
                &format!("library.remove:{f}"),
                "Library",
                f,
                "Scanned for video and music (sub-folders included).",
                "action",
                "Remove".into(),
            ),
            false,
        ));
    }
    v.push((
        row(
            "library.add",
            "Library",
            "Add a folder",
            "Pick a folder to include in the library.",
            "action",
            "Add folder…".into(),
        ),
        false,
    ));
    v.push((
        row(
            "library.rescan",
            "Library",
            "Rescan library",
            "Look for new, changed and deleted files now. Vivi also rescans on start.",
            "action",
            "Rescan".into(),
        ),
        false,
    ));
    v.push((row("library.thumbnails", "Library", "Thumbnails", "Generate video thumbnails and album covers with ffmpeg (if installed), cached in ~/.cache/vivi/thumbs.", "toggle", b(c.library.thumbnails)), true));
    v.push((
        row(
            "library.clear_thumbs",
            "Library",
            "Clear thumbnail cache",
            "Delete cached thumbnails; they are regenerated when needed.",
            "action",
            "Clear".into(),
        ),
        true,
    ));
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
    a.set_accent_name(c.interface.accent.clone().into());
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

fn strings(v: impl IntoIterator<Item = impl Into<SharedString>>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
}

/// Push the configured bottom-bar layout to the UI.
pub fn apply_toolbar(a: &AppWindow, c: &Config) {
    let t = config::sanitize_toolbar(&c.interface.toolbar);
    let mid = t.iter().position(|s| s == "|").unwrap_or(0);
    a.set_bar_left(strings(t[..mid].iter().cloned()));
    a.set_bar_right(strings(t[mid + 1..].iter().cloned()));
    a.set_profile(c.interface.profile.clone().into());
    a.set_customize_available(strings(
        config::TOOL_ITEMS
            .iter()
            .filter(|i| !t.iter().any(|s| s == *i))
            .copied(),
    ));
    a.set_volume_max(c.audio.volume_max as f32);
}

/// Mirror mpv's current values into the side-panel sliders.
pub fn sync_panel(a: &AppWindow, p: &Player) {
    let f = |k: &str, d: f64| p.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d) as f32;
    a.set_pv_sub_delay(f("sub-delay", 0.0));
    a.set_pv_sub_scale(f("sub-scale", 1.0));
    a.set_pv_sub_pos(f("sub-pos", 100.0));
    a.set_pv_audio_delay(f("audio-delay", 0.0));
    a.set_pv_boost(f("volume", 100.0));
    a.set_pv_rotate(f("video-rotate", 0.0) as i32);
    a.set_pv_zoom(f("video-zoom", 0.0));
    a.set_pv_panscan(f("panscan", 0.0) * 100.0);
    a.set_pv_brightness(f("brightness", 0.0));
    a.set_pv_contrast(f("contrast", 0.0));
    a.set_pv_saturation(f("saturation", 0.0));
    a.set_pv_gamma(f("gamma", 0.0));
    a.set_pv_speed(f("speed", 1.0));
}

/// Bottom-bar actions that need the player.
fn bar_action(a: &AppWindow, p: &Player, id: &str) {
    match id {
        "back10" => p.seek_rel(-10.0),
        "fwd10" => p.seek_rel(10.0),
        "stop" => {
            p.set("pause", "yes");
            p.seek_abs(0.0);
        }
        "snapshot" => {
            p.run("screenshot");
            a.set_resume_text("Snapshot saved".into());
        }
        "repeat" => {
            let on = !a.get_repeat_on();
            p.set("loop-file", if on { "inf" } else { "no" });
            a.set_repeat_on(on);
        }
        "shuffle" => {
            let on = !a.get_shuffle_on();
            p.run(if on {
                "playlist-shuffle"
            } else {
                "playlist-unshuffle"
            });
            a.set_shuffle_on(on);
        }
        _ => {}
    }
}

fn customize(
    app: &AppWindow,
    player: &Rc<Player>,
    cfg: &Rc<RefCell<Config>>,
    save: impl Fn() + Clone + 'static,
    log: bool,
) {
    let backup: Rc<RefCell<Option<config::Interface>>> = Rc::new(RefCell::new(None));
    let edit = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        move |f: &dyn Fn(&mut Vec<String>)| {
            let Some(a) = w.upgrade() else { return };
            let mut c = cfg.borrow_mut();
            let mut t = config::sanitize_toolbar(&c.interface.toolbar);
            f(&mut t);
            c.interface.profile = config::PROFILES
                .iter()
                .find(|p| config::profile_toolbar(p).as_ref() == Some(&t))
                .map_or("Custom".into(), |p| p.to_string());
            c.interface.toolbar = t;
            apply_toolbar(&a, &c);
        }
    };
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        let backup = backup.clone();
        app.on_customize_profile(move |p| {
            let Some(a) = w.upgrade() else { return };
            if !a.get_customize_open() {
                *backup.borrow_mut() = Some(cfg.borrow().interface.clone());
                a.set_customize_open(true);
            }
            if let Some(t) = config::profile_toolbar(&p) {
                let mut c = cfg.borrow_mut();
                c.interface.toolbar = t;
                c.interface.profile = p.to_string();
            }
            apply_toolbar(&a, &cfg.borrow());
        });
    }
    {
        let edit = edit.clone();
        app.on_customize_move(move |i, d| {
            edit(&|t| {
                let (i, j) = (i as usize, i as usize as i64 + d as i64);
                if j >= 0 && (j as usize) < t.len() && i < t.len() {
                    t.swap(i, j as usize);
                }
            })
        });
    }
    {
        let edit = edit.clone();
        app.on_customize_remove(move |i| {
            edit(&|t| {
                if t.get(i as usize).is_some_and(|s| s != "|") {
                    t.remove(i as usize);
                }
            })
        });
    }
    app.on_customize_add(move |id| edit(&|t| t.push(id.to_string())));
    {
        let w = app.as_weak();
        let cfg = cfg.clone();
        let p = player.clone();
        app.on_customize_done(move |keep| {
            let Some(a) = w.upgrade() else { return };
            a.set_customize_open(false);
            let old = backup.borrow_mut().take();
            if keep {
                save();
                if log {
                    eprintln!("toolbar saved: {:?}", cfg.borrow().interface.toolbar);
                }
            } else if let Some(old) = old {
                cfg.borrow_mut().interface = old;
            }
            apply_toolbar(&a, &cfg.borrow());
        });
        let w = app.as_weak();
        app.on_bar_action(move |id| {
            if let Some(a) = w.upgrade() {
                bar_action(&a, &p, &id);
            }
        });
    }
}

fn onboarding(app: &AppWindow, cfg: &Rc<RefCell<Config>>, save: impl Fn() + 'static) {
    let keys = [
        ("Play / pause", "Space"),
        ("Seek ±5 s (Shift: 60 s)", "← →"),
        ("Volume", "↑ ↓"),
        ("Fullscreen", "F"),
        ("Open file", "Ctrl O"),
        ("Settings", "Ctrl ,"),
    ];
    app.set_onboarding_actions(strings(keys.iter().map(|k| k.0)));
    app.set_onboarding_keys(strings(keys.iter().map(|k| k.1)));
    app.set_onboarding_open(!cfg.borrow().interface.onboarded);
    let w = app.as_weak();
    let cfg = cfg.clone();
    app.on_onboarding_done(move || {
        if let Some(a) = w.upgrade() {
            a.set_onboarding_open(false);
        }
        cfg.borrow_mut().interface.onboarded = true;
        save();
    });
}

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>, log: bool) {
    app.set_settings_categories(ModelRc::new(VecModel::from(
        CATEGORIES
            .iter()
            .map(|s| SharedString::from(*s))
            .collect::<Vec<_>>(),
    )));
    app.set_accent_names(strings(ACCENTS));
    app.set_accent_colors(ModelRc::new(VecModel::from(
        ACCENTS.iter().map(|a| hex(a)).collect::<Vec<_>>(),
    )));
    app.set_profiles(strings(config::PROFILES));
    apply_theme(app, &cfg.borrow());
    apply_toolbar(app, &cfg.borrow());
    apply_player(player, &cfg.borrow());

    let query = Rc::new(RefCell::new((0i32, String::new(), false)));
    let rows: Rc<VecModel<SettingRow>> = Rc::new(VecModel::default());
    app.set_settings_rows(ModelRc::from(rows.clone()));
    // Update rows in place when the set of rows is unchanged, so focused
    // widgets (e.g. the language text fields) are not recreated.
    let refresh = {
        let cfg = cfg.clone();
        let query = query.clone();
        move || {
            let q = query.borrow();
            let new = filtered(&cfg.borrow(), q.0, &q.1, q.2);
            let same = new.len() == rows.row_count()
                && new
                    .iter()
                    .zip(rows.iter())
                    .all(|(a, b)| a.id == b.id && a.title == b.title && a.kind == b.kind);
            if same {
                for (i, r) in new.into_iter().enumerate() {
                    if rows
                        .row_data(i)
                        .is_some_and(|o| o.value != r.value || o.info != r.info)
                    {
                        rows.set_row_data(i, r);
                    }
                }
            } else {
                rows.set_vec(new);
            }
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

    customize(app, player, &cfg, save.clone(), log);
    onboarding(app, &cfg, save.clone());
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
            let mut rescan = false;
            {
                let mut c = cfg.borrow_mut();
                match id.as_str() {
                    "interface.theme" => c.interface.theme = v.into(),
                    "interface.accent" => c.interface.accent = v.into(),
                    "interface.profile" => {
                        if let Some(t) = config::profile_toolbar(v) {
                            c.interface.toolbar = t;
                        }
                        c.interface.profile = v.into();
                    }
                    "interface.customize" => {
                        drop(c);
                        a.set_settings_open(false);
                        a.invoke_customize_profile("".into());
                        return;
                    }
                    "interface.onboarding" => {
                        a.set_settings_open(false);
                        a.set_onboarding_step(0);
                        a.set_onboarding_open(true);
                        return;
                    }
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
                    "library.add" => {
                        if let Some(d) = rfd::FileDialog::new()
                            .set_title("Add library folder")
                            .pick_folder()
                        {
                            let d = d.to_string_lossy().trim_end_matches('/').to_string();
                            if !c.library.folders.contains(&d) {
                                c.library.folders.push(d);
                            }
                        }
                        rescan = true;
                    }
                    "library.rescan" => rescan = true,
                    "library.thumbnails" => c.library.thumbnails = v == "true",
                    "library.clear_thumbs" => {
                        let _ = std::fs::remove_dir_all(store::cache_dir().join("thumbs"));
                        a.set_settings_status("Thumbnail cache cleared".into());
                        return;
                    }
                    k if k.starts_with("library.remove:") => {
                        let f = &k["library.remove:".len()..];
                        c.library.folders.retain(|x| x != f);
                        rescan = true;
                    }
                    k if k.starts_with("preset.delete:") => {
                        c.presets.remove(&k["preset.delete:".len()..]);
                    }
                    _ => return,
                }
            }
            apply_theme(&a, &cfg.borrow());
            apply_toolbar(&a, &cfg.borrow());
            apply_player(&p, &cfg.borrow());
            sync_panel(&a, &p);
            save();
            refresh();
            if rescan {
                a.invoke_lib_rescan();
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
