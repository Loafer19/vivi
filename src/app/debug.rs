//! Screenshot / test helpers: env-driven UI state (VIVI_SETTINGS, VIVI_PANEL, ...), active in debug builds or with VIVI_SCREENSHOT=1.

use crate::*;
use slint::ComponentHandle;

/// Debug builds, or release builds started with `VIVI_SCREENSHOT=1`: the env-driven UI
/// helpers below (used to capture docs/screenshots) are active.
pub(crate) fn screenshot_mode() -> bool {
    cfg!(debug_assertions) || std::env::var_os("VIVI_SCREENSHOT").is_some()
}

/// Debug builds (or `VIVI_SCREENSHOT=1`): open dialogs from env vars for screenshots/tests.
/// VIVI_SETTINGS=<category index or name> [VIVI_ADVANCED=1] [VIVI_SEARCH=text], VIVI_CUSTOMIZE=<profile>,
/// VIVI_ONBOARDING=<step>, VIVI_LIB=<nav>, VIVI_LIBQ=<search>,
/// VIVI_NEW_PLAYLIST=<name>, VIVI_URL=<url>, VIVI_LIBRARY, VIVI_PAGE=<streams hub page>,
/// VIVI_MENU, VIVI_SUB, VIVI_DRAWER=<plugin id> (player side drawer; also the `drawer` socket command), VIVI_EXT=<plugin id>[:gear] (Extensions window), VIVI_SUBS[=folder|search] (Add Subtitles dialog), VIVI_MUSIC=<plugin id>
/// [VIVI_MUSICQ=query] [VIVI_MUSICSRC=source] [VIVI_MUSICPLAY=row] (a plugin dialog,
/// searched when a query or source is given, then a result row played).
/// Show plugin `id` in Settings → Extensions → Browse (search filtered to its name).
pub(crate) fn open_plugin_link(app: &AppWindow, id: &str) {
    use plugins::packages;
    packages::with_state(|s| s.tab = "Browse".into());
    let name = packages::with_state(|s| {
        s.catalog
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.name.clone())
    });
    let n = crate::settings::category("Extensions");
    let q: slint::SharedString = name.unwrap_or_default().into();
    app.global::<crate::Overlays>().set_settings_open(true);
    app.global::<crate::Settings>().set_category(n);
    app.global::<crate::Settings>().set_search(q.clone());
    app.global::<crate::Settings>().invoke_refresh(n, q, false);
}

/// A Settings category from VIVI_SETTINGS: its index or its name ("Mouse").
pub(crate) fn settings_index(v: &str) -> i32 {
    v.trim()
        .parse()
        .unwrap_or_else(|_| crate::settings::category(v.trim()))
}

pub(crate) fn debug_ui(app: &AppWindow) {
    let var = |k: &str| std::env::var(k).ok();
    // VIVI_SETTINGS=<index or category name>, e.g. 5 or Mouse.
    if let Some(n) = var("VIVI_SETTINGS").map(|v| settings_index(&v)) {
        let adv = var("VIVI_ADVANCED").is_some();
        let q = var("VIVI_SEARCH").unwrap_or_default();
        app.global::<crate::Overlays>().set_settings_open(true);
        app.global::<crate::Settings>().set_category(n);
        app.global::<crate::Settings>().set_advanced(adv);
        app.global::<crate::Settings>().set_search(q.clone().into());
        app.global::<crate::Settings>()
            .invoke_refresh(n, q.into(), adv);
    }
    // VIVI_SETTINGS_ACTION=<row id>: press a Settings action button after 3 s, e.g.
    // privacy.bookmarks (its confirmation dialog).
    if let Some(id) = var("VIVI_SETTINGS_ACTION") {
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(3000), move || {
            if let Some(a) = w.upgrade() {
                a.global::<crate::Settings>()
                    .invoke_setting_changed(id.into(), "".into());
            }
        });
    }
    // VIVI_MOUSE=<button>,…: report mouse buttons on the video (click, double-click,
    // middle, back, forward, wheel-up, wheel-down) one every 1.5 s from 4 s, as the window
    // does, so Settings → Controls → Mouse can be checked without synthesized input.
    if let Some(list) = var("VIVI_MOUSE") {
        for (i, b) in list.split(',').map(str::trim).enumerate() {
            let w = app.as_weak();
            let b = b.to_string();
            slint::Timer::single_shot(Duration::from_millis(4000 + 1500 * i as u64), move || {
                let Some(a) = w.upgrade() else { return };
                let pb = a.global::<crate::Playback>();
                let done = match b.as_str() {
                    "wheel-up" => pb.invoke_wheel(120.0),
                    "wheel-down" => pb.invoke_wheel(-120.0),
                    _ => {
                        pb.invoke_mouse(b.as_str().into());
                        true
                    }
                };
                eprintln!("vivi: VIVI_MOUSE {b} (handled: {done})");
            });
        }
    }
    if let Some(p) = var("VIVI_CUSTOMIZE") {
        app.global::<crate::Customize>()
            .invoke_select_profile(p.into());
    }
    if let Some(n) = var("VIVI_LIB") {
        app.global::<crate::Library>().invoke_navigate(n.into());
    }
    if let Some(q) = var("VIVI_LIBQ") {
        app.global::<crate::Library>().set_query(q.clone().into());
        app.global::<crate::Library>().invoke_search(q.into());
    }
    if let Some(u) = var("VIVI_STREAM_ERROR") {
        // The failed-stream banner (Retry / Copy) without a network round trip.
        let name = u.rsplit('/').next().unwrap_or(&u).to_string();
        super::session::show_stream_error(
            app,
            &format!("Could not open {name}: the stream did not respond"),
            &u,
        );
    }
    if let Some(u) = var("VIVI_URL") {
        // After the render context exists (files must not load before it).
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1500), move || {
            if let Some(a) = w.upgrade() {
                a.global::<crate::Playback>().invoke_open_url(u.into());
            }
        });
    }
    if let Some(id) = var("VIVI_MUSIC") {
        let q = var("VIVI_MUSICQ");
        let src = var("VIVI_MUSICSRC");
        let play = var("VIVI_MUSICPLAY");
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2000), move || {
            plugins::command(&id, "open", &[]);
            let Some(a) = w.upgrade() else { return };
            if let Some(s) = &src {
                a.global::<crate::MusicDialogState>()
                    .set_source(s.as_str().into());
            }
            if let Some(q) = &q {
                a.global::<crate::MusicDialogState>()
                    .set_query(q.as_str().into());
            }
            if src.is_some() || q.is_some() {
                plugins::command(&id, "search", &[]);
            }
            // VIVI_MUSICPLAY=<row>: play that result row once the search had time to answer.
            if let Some(row) = play {
                slint::Timer::single_shot(Duration::from_secs(12), move || {
                    plugins::command(&id, "primary", &[row]);
                });
            }
        });
    }
    if let Some(n) = var("VIVI_NEW_PLAYLIST") {
        app.global::<crate::Dialogs>()
            .set_new_playlist_name(n.into());
        app.global::<crate::Overlays>().set_new_playlist_open(true);
    }
    if var("VIVI_LIBRARY").is_some() {
        app.global::<crate::Overlays>().set_library_open(true);
    }
    // VIVI_PAGE=streams[:tab] | streams:music:<id> | streams:url | service:<id>: open a
    // Streams hub place (#86) once the plugins are loaded.
    if let Some(p) = var("VIVI_PAGE") {
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(1500), move || {
            if let Some(a) = w.upgrade() {
                crate::streams::open_page(&a, &p);
            }
        });
    }
    // VIVI_MENU="kind;x;y;ctx" (ctx lines joined by '|' or '~'), VIVI_SUB="row-id;y": open a menu.
    if let Some(m) = var("VIVI_MENU") {
        let sub = var("VIVI_SUB");
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2500), move || {
            let Some(a) = w.upgrade() else { return };
            let f: Vec<&str> = m.splitn(4, ';').collect();
            let num = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(100.0);
            let ctx = f.get(3).copied().unwrap_or("").replace(['|', '~'], "\n");
            a.global::<crate::Menus>()
                .invoke_menu_request(f[0].into(), num(1), num(2), ctx.into());
            if let Some((id, y)) = sub.as_deref().and_then(|s| s.split_once(';')) {
                a.global::<crate::Menus>()
                    .set_sub_y(y.parse().unwrap_or(0.0));
                a.global::<crate::Menus>().set_sub_open(id.into());
                a.global::<crate::Menus>().invoke_menu_sub(id.into());
            }
        });
    }
    // VIVI_SUBS=folder: "To This Folder"; =search: "From OpenSubtitles".
    if let Some(m) = var("VIVI_SUBS") {
        app.global::<crate::Subtitles>()
            .set_target(i32::from(m == "folder"));
        app.global::<crate::Subtitles>()
            .set_source(i32::from(m != "search"));
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2500), move || {
            if let Some(a) = w.upgrade() {
                a.invoke_add_subs();
            }
        });
    }
    // VIVI_EXT=<plugin id>[:gear] (or "" for All / Marketplace): the Extensions window.
    if let Some(t) = var("VIVI_EXT") {
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(2500), move || {
            if let Some(a) = w.upgrade() {
                let (id, gear) = t
                    .split_once(':')
                    .map_or((t.as_str(), false), |(i, g)| (i, g == "gear"));
                crate::extwin::open(&a, id, gear);
            }
        });
    }
    // VIVI_DRAWER=<plugin id> (or "" for the first): the player side drawer on that panel.
    if let Some(id) = var("VIVI_DRAWER") {
        let w = app.as_weak();
        slint::Timer::single_shot(Duration::from_millis(3000), move || {
            if let Some(a) = w.upgrade() {
                crate::drawer::open(&a, &id);
            }
        });
    }
    if let Some(n) = var("VIVI_ONBOARDING").and_then(|v| v.parse().ok()) {
        app.global::<crate::Settings>().set_onboarding_step(n);
        app.global::<crate::Overlays>().set_onboarding_open(true);
    }
}

/// `VIVI_PANEL=<n>` (screenshot helper): the side panel tab by its old number.
pub(crate) fn panel_tab(n: u8) -> PanelTab {
    match n {
        1 => PanelTab::Audio,
        2 => PanelTab::Video,
        3 => PanelTab::Playback,
        4 => PanelTab::Queue,
        5 => PanelTab::Library,
        6 => PanelTab::Bookmarks,
        7 => PanelTab::Chapters,
        _ => PanelTab::Subtitles,
    }
}
