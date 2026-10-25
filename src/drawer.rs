//! Player side drawer (ui/drawer.slint): a right slide-out panel hosting plugin panels
//! (`drawer.panel` contributions). Lyrics shows its lines; other plugins their actions.
//! The contribution's command (if any) is sent when its panel opens. Packages can fill
//! their tab with live content (`vivi-content <id> drawer <json>`, see
//! `plugins::content`) and open it (`vivi-drawer <id>`, [`request`]).

use crate::plugins::content::{self, Surface, MAX_ROWS};
use crate::plugins::{self, contrib, SettingKind};
use crate::{AppWindow, ExtAction, ExtTab};
use slint::{ComponentHandle, ModelRc, VecModel};

pub fn setup(app: &AppWindow) {
    let w = app.as_weak();
    app.global::<crate::Drawer>().on_open(move |id| {
        if let Some(a) = w.upgrade() {
            open(&a, &id);
        }
    });
    let w = app.as_weak();
    app.global::<crate::Drawer>().on_settings(move || {
        if let Some(a) = w.upgrade() {
            let id = a.global::<crate::Drawer>().get_tab();
            crate::extwin::open(&a, &id, true);
        }
    });
    let w = app.as_weak();
    app.global::<crate::Drawer>().on_run_action(move |id| {
        if let Some(a) = w.upgrade() {
            a.global::<crate::Settings>()
                .invoke_setting_changed(id, "run".into());
        }
    });
}

/// Open the drawer on plugin `id`'s panel ("" = the first one). False when no
/// enabled plugin contributes a panel.
pub fn open(a: &AppWindow, id: &str) -> bool {
    let panels = plugins::contributions(contrib::DRAWER_PANEL);
    let Some(p) = panels
        .iter()
        .find(|e| e.id == id)
        .or(panels.first())
        .cloned()
    else {
        crate::plugins::toast(
            a,
            "No plugin offers a drawer panel: turn one on in Extensions",
        );
        return false;
    };
    let g = a.global::<crate::Drawer>();
    let tabs: Vec<ExtTab> = panels
        .iter()
        .map(|e| ExtTab {
            id: e.id.as_str().into(),
            name: e.label.as_str().into(),
            icon: e.icon.as_str().into(),
            on: true,
            source: false,
        })
        .collect();
    g.set_tabs(ModelRc::new(VecModel::from(tabs)));
    g.set_tab(p.id.as_str().into());
    let m = plugins::manifests().into_iter().find(|m| m.id == p.id);
    g.set_info(
        m.as_ref()
            .map(|m| m.description.clone())
            .unwrap_or_default()
            .into(),
    );
    let actions: Vec<ExtAction> = m
        .iter()
        .flat_map(|m| {
            m.settings.iter().filter_map(|s| match &s.kind {
                SettingKind::Action(l) => Some(ExtAction {
                    id: plugins::setting_id(&m.id, &s.key).into(),
                    title: s.title.as_str().into(),
                    label: l.as_str().into(),
                }),
                _ => None,
            })
        })
        .collect();
    g.set_actions(ModelRc::new(VecModel::from(actions)));
    show_content(a, &p.id, &p.label);
    a.global::<crate::Overlays>().set_drawer_open(true);
    if !p.command.is_empty() {
        plugins::run(&p.id, &p.command, &[]);
    }
    true
}

/// Live content of plugin `id` (or none) on the drawer's content fields.
fn show_content(a: &AppWindow, id: &str, label: &str) {
    let g = a.global::<crate::Drawer>();
    let c = content::get(id, Surface::Drawer);
    g.set_live(c.is_some());
    let c = c.unwrap_or_default();
    g.set_title(
        if c.title.is_empty() {
            label
        } else {
            c.title.as_str()
        }
        .into(),
    );
    g.set_value(c.value.as_str().into());
    g.set_sub(c.sub.as_str().into());
    g.set_progress(c.progress.unwrap_or(-1.0));
    g.set_rows(ModelRc::new(VecModel::from(c.ui_rows(MAX_ROWS))));
}

/// Package `id` changed its drawer content: redraw it when its tab is the one shown.
pub fn refresh_content(a: &AppWindow, id: &str) {
    if a.global::<crate::Drawer>().get_tab() != id {
        return;
    }
    let label = plugins::contributions(contrib::DRAWER_PANEL)
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| e.label)
        .unwrap_or_default();
    show_content(a, id, &label);
}

/// Whether the drawer is open on plugin `id`'s tab (it shows while media plays).
pub fn is_open_on(a: &AppWindow, id: &str) -> bool {
    a.global::<crate::Overlays>().get_drawer_open() && a.global::<crate::Drawer>().get_tab() == id
}

/// `vivi-drawer <id> [open|close|toggle]` and the `drawer` control-socket command: open
/// (`Some(true)`), close (`Some(false)`) or toggle (`None`) the drawer on `id`'s tab. A
/// package without a drawer tab is ignored; "" opens the first tab. Returns whether the
/// drawer is open now.
pub fn request(a: &AppWindow, id: &str, open_it: Option<bool>) -> bool {
    let ov = a.global::<crate::Overlays>();
    let own = id.is_empty()
        || plugins::contributions(contrib::DRAWER_PANEL)
            .iter()
            .any(|e| e.id == id);
    let want = open_it.unwrap_or(!is_open_on(a, id));
    if want && !own {
        return false; // not a drawer panel of this package: do not show another's
    }
    if !want {
        ov.set_drawer_open(false);
        return false;
    }
    open(a, id)
}

/// Toggle the drawer (player menu → Plugins → Side drawer, palette).
pub fn toggle(a: &AppWindow) {
    if a.global::<crate::Overlays>().get_drawer_open() {
        a.global::<crate::Overlays>().set_drawer_open(false);
    } else {
        open(a, "");
    }
}
