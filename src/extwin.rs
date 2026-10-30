//! Extensions window and the sidebar's Sources (ui/extensions.slint,
//! ui/state/extensions.slint).
//!
//! The window has a vertical tab per plugin plus All / Marketplace. A plugin tab shows
//! its overview (description, status, where it shows up from its contribution points,
//! its action buttons, Open for sources); the gear switches to its settings, plus
//! "Show in sidebar" / order for sources and "Show on Home" / order for Home widgets. Row changes go through
//! `Settings.setting-changed`, so the window and Settings share one handler.

use crate::config::Config;
use crate::plugins::{self, contrib, Manifest, Origin, SettingKind};
use crate::settings::{self, Rows};
use crate::{AppWindow, ExtAction, ExtTab, ExtWidget, SettingRow};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

struct State {
    app: slint::Weak<AppWindow>,
    cfg: Rc<RefCell<Config>>,
    rows: Rc<VecModel<SettingRow>>,
}

thread_local! {
    static ST: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub fn setup(app: &AppWindow, cfg: Rc<RefCell<Config>>) {
    let rows: Rc<VecModel<SettingRow>> = Rc::new(VecModel::default());
    let g = app.global::<crate::Extensions>();
    g.set_rows(ModelRc::from(rows.clone()));
    ST.with(|s| {
        *s.borrow_mut() = Some(State {
            app: app.as_weak(),
            cfg,
            rows,
        })
    });
    let w = app.as_weak();
    g.on_select(move |tab, gear| {
        if let Some(a) = w.upgrade() {
            open(&a, &tab, gear);
        }
    });
    g.on_run(|id, cmd| {
        let cmd = if cmd == "::source" {
            source_command(&id)
        } else {
            cmd.to_string()
        };
        plugins::run(&id, &cmd, &[]);
    });
    let w = app.as_weak();
    g.on_open_replacement(move |r| {
        if let Some(a) = w.upgrade() {
            plugins::packages::open_replacement(&a, &r);
        }
    });
    let w = app.as_weak();
    g.on_uninstall(move || {
        if let Some(a) = w.upgrade() {
            uninstall(&a);
        }
    });
    let w = app.as_weak();
    g.on_open_browser(move || {
        let Some(a) = w.upgrade() else { return };
        let id = a.global::<crate::Extensions>().get_tab().to_string();
        let cmd = source_command(&id);
        if !cmd.is_empty() {
            a.global::<crate::Overlays>().set_extensions_open(false);
            plugins::run(&id, &cmd, &[]);
        }
    });
}

/// Command of plugin `id`'s `sidebar.source` entry ("" opens its tab).
fn source_command(id: &str) -> String {
    plugins::contributions_all(contrib::SIDEBAR_SOURCE)
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| e.command)
        .unwrap_or_default()
}

/// "Uninstall…" on a package's page: ask (with "Keep its data"), uninstall, then go back
/// to the plugin list.
pub(crate) fn uninstall(a: &AppWindow) {
    let id = a.global::<crate::Extensions>().get_tab().to_string();
    if plugins::manifests()
        .iter()
        .any(|m| m.id == id && m.origin == Origin::Package)
    {
        confirm_uninstall(a, &id, true);
    }
}

/// Ask before uninstalling downloaded plugin `id` (#96), from its page or from Settings →
/// Extensions: "Keep its data" (unchecked) leaves its settings and data folder. Sidebar
/// icons, chips and menus follow through [`refresh`]; `back` returns the Extensions window
/// to the plugin list.
pub(crate) fn confirm_uninstall(a: &AppWindow, id: &str, back: bool) {
    let name = plugins::manifests()
        .into_iter()
        .find(|m| m.id == id)
        .map(|m| m.name)
        .or_else(|| {
            plugins::packages::with_state(|s| {
                s.catalog
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.name.clone())
            })
        })
        .unwrap_or_else(|| id.to_string());
    let id = id.to_string();
    let w = a.as_weak();
    crate::dialogs::confirm_core_check(
        a,
        "uninstall-plugin",
        "Uninstall",
        &format!("Uninstall {name}?"),
        "Its settings and downloaded data are removed.",
        "Uninstall",
        (
            "Keep its data",
            "Its settings and downloaded data stay, ready if you install it again.",
        ),
        move |keep| {
            let res = plugins::packages::remove(&id, keep);
            let Some(a) = w.upgrade() else { return };
            if back {
                let g = a.global::<crate::Extensions>();
                g.set_tab("".into());
                g.set_gear(false);
            }
            settings::refresh_ui();
            plugins::packages::toast(&match res {
                Ok(()) => {
                    format!("Uninstalled {name}. If it is running, it stops when Vivi restarts")
                }
                Err(e) => format!("Could not uninstall {name}: {e}"),
            });
        },
    );
}

/// Open the window on `tab` (a plugin id, "" = All / Marketplace), on its settings
/// when `gear`.
pub fn open(a: &AppWindow, tab: &str, gear: bool) {
    let g = a.global::<crate::Extensions>();
    g.set_tab(tab.into());
    g.set_gear(gear && !tab.is_empty());
    a.global::<crate::Overlays>().set_settings_open(false);
    a.global::<crate::Overlays>().set_extensions_open(true);
    refresh();
}

/// Toggle the window (Ctrl+E).
pub fn toggle(a: &AppWindow) {
    if a.global::<crate::Overlays>().get_extensions_open() {
        a.global::<crate::Overlays>().set_extensions_open(false);
    } else {
        let tab = a.global::<crate::Extensions>().get_tab();
        open(a, &tab, false);
    }
}

fn tab_of(m: &Manifest, c: &Config) -> ExtTab {
    ExtTab {
        id: m.id.as_str().into(),
        name: m.name.as_str().into(),
        icon: m.icon.as_str().into(),
        on: plugins::is_enabled(c, m),
        source: m.role == contrib::Role::Source,
    }
}

/// "Where it shows up" lines of a manifest (entries that moved to an enabled plugin left
/// out, as the menus and palette leave them out; see `contrib::MOVED`).
pub fn places(m: &Manifest) -> Vec<String> {
    let mut v: Vec<String> = m
        .contributes
        .iter()
        .filter(|c| !moved_away(&m.id, c))
        .map(|c| {
            let at = contrib::describe(&c.point);
            if c.label.is_empty() || c.label == m.name {
                at.to_string()
            } else {
                format!("{at}: {}", c.label)
            }
        })
        .collect();
    v.dedup();
    v
}

/// Entry `c` of plugin `id` moved to another plugin that offers it at the same point now.
fn moved_away(id: &str, c: &contrib::Contribution) -> bool {
    contrib::MOVED.iter().any(|(oid, ocmd, nid, ncmd)| {
        id == *oid
            && c.command == *ocmd
            && plugins::contributions_all(&c.point)
                .iter()
                .any(|e| e.id == *nid && e.command == *ncmd)
    })
}

/// Rows of the gear view: the plugin's settings, then sidebar options for sources.
fn gear_rows(r: &mut Rows, m: &Manifest) {
    r.cat("Extensions");
    for st in &m.settings {
        let id = plugins::setting_id(&m.id, &st.key);
        let val = plugins::setting_value(r.c, m, &st.key);
        match &st.kind {
            SettingKind::Toggle => {
                r.toggle(&id, &st.title, &st.info, val == "true");
            }
            SettingKind::Choice(o) => {
                let o: Vec<&str> = o.iter().map(String::as_str).collect();
                r.choice(&id, &st.title, &st.info, &val, &o);
            }
            SettingKind::Text(ph) => {
                r.text(&id, &st.title, &st.info, &val, ph);
            }
            SettingKind::Secret => {
                r.text(&id, &st.title, &st.info, &val, "");
                r.last().kind = "secret".into();
            }
            SettingKind::Action(l) => {
                r.action(&id, &st.title, &st.info, l);
            }
        }
    }
    if m.role == contrib::Role::Source {
        r.header("Sidebar");
        r.toggle(
            &format!("src.{}.show", m.id),
            "Show in Sources",
            "List this plugin under Sources in the sidebar and on Home.",
            !plugins::source_hidden(r.c, &m.id),
        );
        r.push(
            &format!("src.{}", m.id),
            "Position in Sources",
            "",
            "buttons",
            String::new(),
        );
        r.last().labels = settings::strs(&["Move up", "Move down"]);
    }
    if m.contributes
        .iter()
        .any(|c| c.point == contrib::HOME_WIDGET)
    {
        r.header("Home");
        r.toggle(
            &format!("src.{}.home-show", m.id),
            "Show on Home",
            "Show this plugin's widgets on the Home screen.",
            !plugins::hidden_at(r.c, contrib::HOME_WIDGET, &m.id),
        );
        r.push(
            &format!("src.{}.home", m.id),
            "Position on Home",
            "",
            "buttons",
            String::new(),
        );
        r.last().labels = settings::strs(&["Move up", "Move down"]);
    }
}

/// A `src.<id>[.show|.home|.home-show]` row changed (from `Settings.setting-changed`).
pub fn source_setting(rest: &str, value: &str) {
    let show = if value == "true" { "show" } else { "hide" };
    let mv = if value == "Move up" { "up" } else { "down" };
    if let Some(id) = rest.strip_suffix(".home-show") {
        plugins::arrange(contrib::HOME_WIDGET, id, show);
    } else if let Some(id) = rest.strip_suffix(".home") {
        plugins::arrange(contrib::HOME_WIDGET, id, mv);
    } else if let Some(id) = rest.strip_suffix(".show") {
        plugins::arrange_source(id, show);
    } else {
        plugins::arrange_source(rest, mv);
    }
}

/// Home widgets of the enabled plugins: the static manifest text, or the live content a
/// package sent (`vivi-content <id> home`, see `plugins::content`).
fn widgets(all: &[Manifest]) -> Vec<ExtWidget> {
    use plugins::content::{self, Surface, HOME_ROWS};
    plugins::contributions(contrib::HOME_WIDGET)
        .into_iter()
        .map(|e| {
            let info = all
                .iter()
                .find(|m| m.id == e.id)
                .map(|m| m.description.as_str())
                .unwrap_or_default();
            let live = content::get(&e.id, Surface::Home);
            let c = live.clone().unwrap_or_default();
            ExtWidget {
                live: live.is_some(),
                info: if c.sub.is_empty() || live.is_none() {
                    info
                } else {
                    c.sub.as_str()
                }
                .into(),
                label: if c.title.is_empty() {
                    e.label.as_str()
                } else {
                    c.title.as_str()
                }
                .into(),
                value: c.value.as_str().into(),
                progress: c.progress.unwrap_or(-1.0),
                rows: ModelRc::new(VecModel::from(c.ui_rows(HOME_ROWS))),
                id: e.id.into(),
                name: e.name.into(),
                icon: e.icon.into(),
                command: e.command.into(),
            }
        })
        .collect()
}

/// A package changed its Home widget content: rebuild only the widgets.
pub fn refresh_widgets() {
    let Some(a) = ST.with(|s| s.borrow().as_ref().and_then(|s| s.app.upgrade())) else {
        return;
    };
    let all = plugins::manifests();
    a.global::<crate::Extensions>()
        .set_widgets(ModelRc::new(VecModel::from(widgets(&all))));
}

/// Rebuild tabs, sidebar sources and the selected tab (after any plugin change).
pub fn refresh() {
    let Some((a, cfg, rows)) = ST.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        Some((s.app.upgrade()?, s.cfg.clone(), s.rows.clone()))
    }) else {
        return;
    };
    let c = cfg.borrow().clone();
    let g = a.global::<crate::Extensions>();
    let mut all = plugins::manifests();
    // Sources first (the window lists them in their own group), then features.
    all.sort_by_key(|m| m.role != contrib::Role::Source);
    let tabs: Vec<ExtTab> = all.iter().map(|m| tab_of(m, &c)).collect();
    g.set_tabs(ModelRc::new(VecModel::from(tabs)));
    let sources: Vec<ExtTab> = plugins::contributions(contrib::SIDEBAR_SOURCE)
        .into_iter()
        .map(|e| ExtTab {
            id: e.id.into(),
            name: e.label.into(),
            icon: e.icon.into(),
            on: true,
            source: true,
        })
        .collect();
    g.set_sources(ModelRc::new(VecModel::from(sources)));
    g.set_widgets(ModelRc::new(VecModel::from(widgets(&all))));
    // Plugin buttons in the bottom bar follow the plugins being turned on / off.
    settings::apply_toolbar(&a, &c);
    if !a.global::<crate::Overlays>().get_extensions_open() {
        return;
    }
    let tab = g.get_tab().to_string();
    g.set_index(
        all.iter()
            .position(|m| m.id == tab)
            .map_or(-1, |i| i as i32),
    );
    let mut r = Rows {
        c: &c,
        v: Vec::new(),
        cat: "Extensions",
        adv: false,
    };
    match all.iter().find(|m| m.id == tab) {
        None => {
            g.set_tab("".into());
            g.set_gear(false);
            settings::extensions(&mut r);
        }
        Some(m) => {
            let src = match m.origin {
                Origin::Builtin => "Built-in",
                Origin::Package => "Downloaded",
                Origin::Script => "mpv script",
            };
            let mut meta = Vec::new();
            if !m.version.is_empty() {
                meta.push(format!("v{}", m.version));
            }
            if !m.author.is_empty() {
                meta.push(m.author.clone());
            }
            meta.push(src.to_string());
            meta.push(if m.role == contrib::Role::Source {
                "Source".into()
            } else {
                "Feature".into()
            });
            g.set_title(m.name.as_str().into());
            g.set_info(m.description.as_str().into());
            g.set_meta(meta.join("  ·  ").into());
            g.set_icon(m.icon.as_str().into());
            g.set_on(plugins::is_enabled(&c, m));
            g.set_removable(m.origin == Origin::Package);
            g.set_source(m.role == contrib::Role::Source && !source_command(&m.id).is_empty());
            // Packages report through script messages; built-ins answer a "status" query
            // (e.g. who is signed in).
            let status = plugins::script_status(&m.id).or_else(|| {
                (m.origin == Origin::Builtin && plugins::is_enabled(&c, m))
                    .then(|| plugins::query::<String>(&m.id, "status"))
                    .flatten()
            });
            g.set_status(status.unwrap_or_default().into());
            let dep = (m.origin == Origin::Package)
                .then(|| plugins::packages::deprecation(&m.id))
                .flatten();
            g.set_deprecated(
                dep.as_ref()
                    .map(|d| d.text(&m.name))
                    .unwrap_or_default()
                    .into(),
            );
            let label = match &dep {
                Some(d) if d.can_open() && d.action.is_empty() => "Open".to_string(),
                Some(d) if d.can_open() => d.action.clone(),
                _ => String::new(),
            };
            g.set_replacement_label(label.into());
            g.set_replacement(dep.map(|d| d.replacement).unwrap_or_default().into());
            let mut p = places(m);
            p.push("Extensions window".into());
            g.set_places(crate::util::strings(p.iter().map(String::as_str)));
            let actions: Vec<ExtAction> = m
                .settings
                .iter()
                .filter_map(|s| match &s.kind {
                    SettingKind::Action(l) => Some(ExtAction {
                        id: plugins::setting_id(&m.id, &s.key).into(),
                        title: s.title.as_str().into(),
                        label: l.as_str().into(),
                    }),
                    _ => None,
                })
                .collect();
            g.set_actions(ModelRc::new(VecModel::from(actions)));
            g.set_has_settings(
                m.role == contrib::Role::Source
                    || m.settings
                        .iter()
                        .any(|s| !matches!(s.kind, SettingKind::Action(_))),
            );
            if g.get_gear() {
                gear_rows(&mut r, m);
            }
        }
    }
    let new: Vec<SettingRow> = r.v.into_iter().map(|(row, _)| row).collect();
    // Same rows: update values in place so a focused text field keeps its focus.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_describe_contributions() {
        let m = Manifest::builtin("x", "X", "", "plugins")
            .source("open")
            .contribute(contrib::MENU_SUBTITLES, "Search online…", "open");
        assert_eq!(
            places(&m),
            ["Sidebar → Sources", "Subtitles menu: Search online…"]
        );
    }
}
