//! Extensions window and the sidebar's Sources (ui/extensions.slint,
//! ui/state/extensions.slint).
//!
//! The window has a vertical tab per plugin plus All / Marketplace. A plugin tab shows
//! its overview (description, status, where it shows up from its contribution points,
//! its action buttons, Open for sources); the gear switches to its settings, plus
//! "Show in sidebar" / order for sources. Row changes go through
//! `Settings.setting-changed`, so the window and Settings share one handler.

use crate::config::Config;
use crate::plugins::{self, contrib, Manifest, Origin, SettingKind};
use crate::settings::{self, Rows};
use crate::{AppWindow, ExtAction, ExtTab, SettingRow};
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

/// "Where it shows up" lines of a manifest.
pub fn places(m: &Manifest) -> Vec<String> {
    let mut v: Vec<String> = m
        .contributes
        .iter()
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
}

/// A `src.<id>[.show]` row changed (from `Settings.setting-changed`).
pub fn source_setting(rest: &str, value: &str) {
    match rest.strip_suffix(".show") {
        Some(id) => plugins::arrange_source(id, if value == "true" { "show" } else { "hide" }),
        None => plugins::arrange_source(rest, if value == "Move up" { "up" } else { "down" }),
    }
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
            g.set_source(m.role == contrib::Role::Source && !source_command(&m.id).is_empty());
            g.set_status(plugins::script_status(&m.id).unwrap_or_default().into());
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
