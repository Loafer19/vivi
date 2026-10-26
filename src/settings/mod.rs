//! Settings dialog model, live application of settings and per-show presets (#16).

mod apply;
mod customize;
mod extensions;
mod onboarding;
mod rows;
mod schema;
pub(crate) use apply::*;
pub(crate) use customize::*;
pub(crate) use extensions::*;
pub(crate) use onboarding::*;
pub(crate) use rows::*;

use crate::config::{self, Config, Preset};
use crate::player::Player;
use crate::plugins;
use crate::{store, AppWindow, SettingRow, Theme};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The Remote Control extension (phone remote), pointed to from Controls → Remotes.
pub(crate) const REMOTE_CONTROL: &str = "remote-control";
pub(crate) const REMOTE_CONTROL_ROW: &str = "controls.remote_control";

/// What the Remotes row offers: "Install", "Enable", or `None` (on: open its settings).
pub(crate) fn remote_control_action(c: &Config) -> Option<&'static str> {
    let m = plugins::manifests()
        .into_iter()
        .find(|m| m.id == REMOTE_CONTROL);
    remote_action(m.is_some(), m.is_some_and(|m| plugins::is_enabled(c, &m)))
}

fn remote_action(installed: bool, enabled: bool) -> Option<&'static str> {
    match (installed, enabled) {
        (false, _) => Some("Install"),
        (true, false) => Some("Enable"),
        (true, true) => None,
    }
}

/// Seconds of inactivity before the controls hide (read by the auto-hide timer).
pub static AUTOHIDE_SECS: AtomicU32 = AtomicU32::new(2);

const CATEGORIES: [&str; 14] = [
    "General",
    "Interface",
    "Privacy",
    "Controls",
    "Playback",
    "Video",
    "Audio",
    "Subtitles",
    "Library",
    "Network",
    "Performance",
    "Recording",
    "Extensions",
    "About",
];
const ACCENTS: [&str; 9] = [
    "#D37918", "#C8452E", "#A68A1C", "#2E9A44", "#1E9AAE", "#2155C4", "#7C3FD1", "#C23B98",
    "#8C8C8C",
];

/// Index of a Settings category by name (0 = General when unknown).
pub fn category(name: &str) -> i32 {
    CATEGORIES.iter().position(|c| *c == name).unwrap_or(0) as i32
}

thread_local! {
    static REFRESH: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Rebuild the visible settings rows (e.g. after a background plugin job finished).
pub fn refresh_ui() {
    if let Some(f) = REFRESH.with(|r| r.borrow().clone()) {
        f();
    }
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
            a.global::<crate::Notices>()
                .set_resume_text("Snapshot saved".into());
        }
        "repeat" => {
            let on = !a.global::<crate::Playback>().get_repeat_on();
            p.set("loop-file", if on { "inf" } else { "no" });
            a.global::<crate::Playback>().set_repeat_on(on);
        }
        "shuffle" => {
            let on = !a.global::<crate::Playback>().get_shuffle_on();
            crate::loopsync::set_shuffle(a, p, on);
        }
        _ => {
            // A plugin's button (`controlbar.button`).
            if let Some((plugin, cmd)) = id
                .strip_prefix(config::PLUGIN_ITEM)
                .and_then(|r| r.split_once('|'))
            {
                crate::plugins::run(plugin, cmd, &[]);
            }
        }
    }
}

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>, log: bool) {
    app.global::<crate::Settings>()
        .set_categories(ModelRc::new(VecModel::from(
            CATEGORIES
                .iter()
                .map(|s| SharedString::from(*s))
                .collect::<Vec<_>>(),
        )));
    app.global::<crate::Settings>()
        .set_accent_names(strings(ACCENTS));
    app.global::<crate::Settings>()
        .set_accent_colors(ModelRc::new(VecModel::from(
            ACCENTS.iter().map(|a| hex(a)).collect::<Vec<_>>(),
        )));
    app.global::<crate::Customize>()
        .set_profiles(strings(config::PROFILES));
    apply_theme(app, &cfg.borrow());
    apply_toolbar(app, &cfg.borrow());
    apply_player(player, &cfg.borrow());
    player.set_volume(cfg.borrow().audio.startup_volume as f64);
    let devices: Rc<Vec<String>> = Rc::new(
        player
            .audio_devices()
            .into_iter()
            .map(|(name, _)| name)
            .collect(),
    );

    let query = Rc::new(RefCell::new((0i32, String::new(), false)));
    let rows: Rc<VecModel<SettingRow>> = Rc::new(VecModel::default());
    app.global::<crate::Settings>()
        .set_rows(ModelRc::from(rows.clone()));
    // Update rows in place when the set of rows is unchanged, so focused
    // widgets (e.g. the language text fields) are not recreated.
    let refresh = {
        let cfg = cfg.clone();
        let query = query.clone();
        move || {
            let q = query.borrow();
            let new = filtered(&cfg.borrow(), &devices, q.0, &q.1, q.2);
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
            // The Extensions window and the sidebar's Sources show the same plugins.
            crate::extwin::refresh();
        }
    };
    let save = {
        let w = app.as_weak();
        let cfg = cfg.clone();
        move || {
            let res = cfg.borrow().save();
            if let Some(a) = w.upgrade() {
                a.global::<crate::Settings>().set_status(match res {
                    Ok(()) => format!("Saved to {}", config::path().display()).into(),
                    Err(e) => format!("Could not save: {e}").into(),
                });
            }
        }
    };

    {
        let refresh = refresh.clone();
        REFRESH.with(|r| *r.borrow_mut() = Some(Rc::new(refresh)));
    }
    crate::about::setup(app, player, &cfg, save.clone());
    customize(app, player, &cfg, save.clone(), log);
    onboarding(app, &cfg, save.clone());
    {
        let refresh = refresh.clone();
        let query = query.clone();
        app.global::<crate::Settings>()
            .on_refresh(move |c, s, adv| {
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
        app.global::<crate::Settings>()
            .on_setting_changed(move |id, value| {
                let Some(a) = w.upgrade() else { return };
                let v = value.as_str();
                if log {
                    eprintln!("setting {id} = {v}");
                }
                if id.starts_with("plugin.") {
                    let msg = plugins::setting_changed(&id, v);
                    a.global::<crate::Settings>()
                        .set_status(msg.unwrap_or_default().into());
                    refresh();
                    return;
                }
                if let Some(rest) = id.strip_prefix("src.") {
                    crate::extwin::source_setting(rest, v);
                    refresh();
                    return;
                }
                if id.as_str() == "ext.window" {
                    crate::extwin::open(&a, "", false);
                    return;
                }
                if let Some(pid) = id.strip_prefix("pkg.") {
                    plugins::packages::action(pid, v);
                    refresh();
                    return;
                }
                if let Some(what) = id.strip_prefix("ext.") {
                    match what {
                        "tab" => plugins::packages::with_state(|s| {
                            s.tab = v.into();
                            s.status.clear();
                        }),
                        "check" => plugins::packages::check(true),
                        "autocheck" | crate::notify::KEY => {
                            cfg.borrow_mut()
                                .plugins
                                .entry(plugins::packages::GLOBAL_ID.into())
                                .or_default()
                                .settings
                                .insert(what.into(), v.into());
                            save();
                        }
                        _ => {}
                    }
                    refresh();
                    return;
                }
                if let Some(what) = id.strip_prefix("scripts.") {
                    match what {
                        "startup" => {
                            cfg.borrow_mut()
                                .plugins
                                .entry(plugins::scripts::GLOBAL_ID.into())
                                .or_default()
                                .settings
                                .insert("startup".into(), v.into());
                            save();
                        }
                        "folder" => {
                            let d = config::scripts_dir();
                            let _ = std::fs::create_dir_all(&d);
                            open_url(&d.to_string_lossy());
                        }
                        "rescan" => {
                            let n = plugins::rescan();
                            a.global::<crate::Settings>()
                                .set_status(format!("{n} mpv scripts found").into());
                        }
                        "catalog" => open_url(CATALOG_URL),
                        _ => {}
                    }
                    refresh();
                    return;
                }
                if id.as_str() == REMOTE_CONTROL_ROW {
                    let action = remote_control_action(&cfg.borrow());
                    match action {
                        Some("Install") => {
                            plugins::packages::install_and_enable(&a, REMOTE_CONTROL)
                        }
                        Some(_) => {
                            let msg = plugins::setting_changed(
                                &plugins::toggle_id(REMOTE_CONTROL),
                                "true",
                            );
                            refresh();
                            plugins::packages::toast(
                                msg.as_deref().unwrap_or("Enabled Remote Control"),
                            );
                        }
                        None => crate::extwin::open(&a, REMOTE_CONTROL, true),
                    }
                    return;
                }
                let mut rescan = false;
                {
                    let mut c = cfg.borrow_mut();
                    match id.as_str() {
                        "interface.theme" => c.interface.theme = v.to_lowercase(),
                        id if schema::has(id) => {
                            schema::set(&mut c, id, v);
                        }
                        "general.log_file" | "video.screenshot_dir" | "recording.dir" => {
                            let mut val = v.to_string();
                            if v == "::browse" {
                                let picked = if id.as_str() == "general.log_file" {
                                    rfd::FileDialog::new()
                                        .set_title("Log file")
                                        .set_file_name("vivi.log")
                                        .save_file()
                                } else {
                                    rfd::FileDialog::new()
                                        .set_title("Choose folder")
                                        .pick_folder()
                                };
                                let Some(d) = picked else { return };
                                val = d.to_string_lossy().into_owned();
                            }
                            match id.as_str() {
                                "general.log_file" => c.general.log_file = val,
                                "video.screenshot_dir" => c.video.screenshot_dir = val,
                                _ => c.recording.dir = val,
                            }
                        }
                        "interface.profile" => {
                            if let Some(t) = config::profile_toolbar(v) {
                                c.interface.toolbar = t;
                                c.interface.bar_style = config::profile_style(v).into();
                                c.interface.volume_style = config::profile_volume(v).into();
                            }
                            c.interface.profile = v.into();
                        }
                        "interface.customize" => {
                            drop(c);
                            a.global::<crate::Overlays>().set_settings_open(false);
                            a.global::<crate::Customize>()
                                .invoke_select_profile("".into());
                            return;
                        }
                        "interface.onboarding" => {
                            a.global::<crate::Overlays>().set_settings_open(false);
                            a.global::<crate::Settings>().set_onboarding_step(0);
                            a.global::<crate::Overlays>().set_onboarding_open(true);
                            return;
                        }
                        "subtitles.outline" => {
                            c.subtitles.outline = match v {
                                "None" => 0.0,
                                "Thin" => 1.5,
                                "Thick" => 5.0,
                                _ => 3.0,
                            }
                        }
                        "video.reset_picture" => {
                            let v = &mut c.video;
                            (v.hue, v.brightness, v.contrast, v.saturation, v.gamma) =
                                (0, 0, 0, 0, 0);
                        }
                        "video.aspect" => {
                            c.video.aspect = if v == "Automatic" {
                                "no".into()
                            } else {
                                v.into()
                            }
                        }
                        k if k.starts_with("stub.") => {
                            c.stubs.insert(k.into(), v.into());
                        }
                        "privacy.recent" => {
                            a.global::<crate::Library>().invoke_clear_recent();
                            a.global::<crate::Settings>()
                                .set_status("Recent files cleared".into());
                            return;
                        }
                        "privacy.scrobbler" => {
                            let id = rows::SCROBBLER;
                            match rows::scrobbler_action(&c) {
                                "Install" => {
                                    drop(c);
                                    crate::plugins::packages::install_and_enable(&a, id);
                                }
                                "Enable" => {
                                    drop(c);
                                    let msg = crate::plugins::setting_changed(
                                        &crate::plugins::toggle_id(id),
                                        "true",
                                    );
                                    crate::settings::refresh_ui();
                                    crate::plugins::packages::toast(
                                        msg.as_deref().unwrap_or("Enabled Scrobbler"),
                                    );
                                }
                                _ => {
                                    drop(c);
                                    crate::extwin::open(&a, id, true);
                                }
                            }
                            return;
                        }
                        "privacy.bookmarks" => {
                            drop(c);
                            let n = crate::bookmarks::count();
                            if n == 0 {
                                a.global::<crate::Settings>()
                                    .set_status("No bookmarks to clear".into());
                                return;
                            }
                            let w = a.as_weak();
                            crate::dialogs::confirm_core(
                                &a,
                                "clear-bookmarks",
                                "Clear bookmarks",
                                &format!(
                                    "Delete {n} bookmark{}?",
                                    if n == 1 { "" } else { "s" }
                                ),
                                "Every saved bookmark of every file and its saved frame is deleted. This cannot be undone.",
                                "Clear",
                                move || {
                                    let n = crate::bookmarks::clear_all();
                                    if let Some(a) = w.upgrade() {
                                        a.global::<crate::Settings>().set_status(
                                            format!(
                                                "{n} bookmark{} cleared",
                                                if n == 1 { "" } else { "s" }
                                            )
                                            .into(),
                                        );
                                    }
                                },
                            );
                            return;
                        }
                        "privacy.watch_later" => {
                            let _ = std::fs::remove_dir_all(store::watch_later_dir());
                            let _ = std::fs::create_dir_all(store::watch_later_dir());
                            a.global::<crate::Settings>()
                                .set_status("Resume positions cleared".into());
                            return;
                        }
                        "library.add" => {
                            if let Some(d) = rfd::FileDialog::new()
                                .set_title("Add library folder")
                                .pick_folder()
                            {
                                let d = d
                                    .to_string_lossy()
                                    .replace('\\', "/")
                                    .trim_end_matches('/')
                                    .to_string();
                                if !c.library.folders.contains(&d) {
                                    c.library.folders.push(d);
                                }
                            }
                            rescan = true;
                        }
                        "library.rescan" => rescan = true,
                        "library.clear_thumbs" => {
                            crate::images::clear();
                            refresh();
                            a.global::<crate::Settings>()
                                .set_status("Image cache cleared".into());
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
                    a.global::<crate::Library>().invoke_rescan();
                }
            });
    }
    {
        let w = app.as_weak();
        let p = player.clone();
        app.global::<crate::Panel>().on_save_preset(move || {
            let Some(a) = w.upgrade() else { return };
            let Some(path) = p.get("path") else { return };
            let key = config::show_key(&path);
            cfg.borrow_mut()
                .presets
                .insert(key.clone(), current_preset(&p));
            save();
            refresh();
            a.global::<crate::Notices>()
                .set_resume_text(format!("Preset saved for “{key}”").into());
            if log {
                eprintln!("preset saved: {key} {:?}", cfg.borrow().presets.get(&key));
            }
        });
    }
}

/// Config volume style name ("Cone", "Bar", …) as the typed UI state.
fn volume_style(s: &str) -> crate::VolumeStyle {
    match config::volume_style_name(s) {
        "Bar" => crate::VolumeStyle::Bar,
        "Popup" => crate::VolumeStyle::Popup,
        "Percent" => crate::VolumeStyle::Percent,
        _ => crate::VolumeStyle::Cone,
    }
}

/// Config bar style name ("Compact", "Classic", …) as the typed UI state.
fn bar_style(s: &str) -> crate::BarStyle {
    match config::bar_style_name(s) {
        "Classic" => crate::BarStyle::Classic,
        "Waveform" => crate::BarStyle::Waveform,
        "Plain" => crate::BarStyle::Plain,
        _ => crate::BarStyle::Compact,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn remote_control_row_offers_install_enable_or_settings() {
        assert_eq!(super::remote_action(false, false), Some("Install"));
        assert_eq!(super::remote_action(true, false), Some("Enable"));
        assert_eq!(super::remote_action(true, true), None);
    }

    use super::*;
    #[test]
    fn search_spans_categories() {
        let c = Config::default();
        let r = filtered(&c, &[], 0, "language", false);
        assert!(r.len() >= 2 && r.iter().all(|r| r.kind != "header"));
        // Performance: hwdec is basic, video-sync is behind "Advanced Options".
        let basic = filtered(&c, &[], 10, "", false);
        assert!(basic.iter().any(|r| r.id.as_str() == "playback.hwdec"));
        assert!(basic.iter().any(|r| r.kind.as_str() == "advanced"));
        assert!(basic
            .iter()
            .all(|r| r.id.as_str() != "performance.video_sync"));
        let adv = filtered(&c, &[], 10, "", true);
        assert!(adv
            .iter()
            .any(|r| r.id.as_str() == "performance.video_sync"));
    }
    #[test]
    fn every_category_has_rows() {
        let c = Config::default();
        // About is a custom page (ui/about.slint), not setting rows.
        for i in 0..CATEGORIES.len() - 1 {
            assert!(!filtered(&c, &[], i as i32, "", false).is_empty(), "{i}");
        }
        assert_eq!(mpv_color("#FF0000", 50), "#7FFF0000");
        assert_eq!(lead_num("10 s"), 10);
    }
}
