//! Home widgets on screen: packs the [`crate::home`] layout into the Slint grid
//! (`Library.home`) and runs Customize Home (edit mode, Settings → Interface → Home).
//!
//! The widget data comes from the Library models libview fills (Continue Watching,
//! Recently Added, …), the Streams and Sources rows and the plugins' `home.widget`
//! entries with their live content (`vivi-content`). Widgets without data are left out,
//! except in edit mode, which shows every widget (hidden ones dimmed in their place).

use crate::app::state::{AppMsg, AppState};
use crate::home::{self, Home, Op, PluginDef, Slot};
use crate::plugins::{self, content, contrib};
use crate::{AppWindow, HomeCell, HomeLine, LibItem};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

struct St {
    app: slint::Weak<AppWindow>,
    cfg: AppState,
    /// Layout when edit mode started (Cancel puts it back).
    backup: Option<Home>,
    narrow: bool,
    /// Widget ids per line of the last packing (drag and drop targets).
    lines: Vec<Vec<String>>,
}

thread_local! {
    static ST: RefCell<Option<St>> = const { RefCell::new(None) };
    /// Plugin item pictures decoded so far, by their `image` value.
    static PICS: RefCell<HashMap<String, slint::Image>> = RefCell::new(HashMap::new());
    static ASKED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

pub fn setup(app: &AppWindow, cfg: AppState) {
    ST.with(|s| {
        *s.borrow_mut() = Some(St {
            app: app.as_weak(),
            cfg,
            backup: None,
            narrow: false,
            lines: Vec::new(),
        })
    });
    let g = app.global::<crate::Library>();
    g.on_home_action(|a, id, v| action(&a, &id, &v));
    g.on_home_narrow(|n| {
        let changed = ST.with(|s| {
            s.borrow_mut()
                .as_mut()
                .is_some_and(|s| std::mem::replace(&mut s.narrow, n) != n)
        });
        if changed {
            // Not from inside the layout pass that reported it.
            slint::Timer::single_shot(std::time::Duration::ZERO, repack);
        }
    });
}

fn state() -> Option<(AppWindow, AppState, bool, bool)> {
    ST.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        Some((
            s.app.upgrade()?,
            s.cfg.clone(),
            s.narrow,
            s.backup.is_some(),
        ))
    })
}

/// The plugins' Home widgets: (layout, contribution). Hidden ones are included (the
/// old *Show on Home* switch is their default until the user places them).
fn plugin_widgets(c: &crate::config::Config) -> Vec<(PluginDef, plugins::Contributed)> {
    let mut seen = HashSet::new();
    plugins::contributions_all(contrib::HOME_WIDGET)
        .into_iter()
        .filter_map(|e| {
            let id = home::plugin_id(&e.id, &e.home.widget);
            if !seen.insert(id.clone()) {
                return None;
            }
            let span = if e.home.span == 0 {
                1
            } else {
                e.home.span.min(3)
            };
            let sizes = home::sizes_of(&e.home.sizes);
            Some((
                PluginDef {
                    id,
                    title: e.label.clone(),
                    size: if sizes.contains('m') {
                        "m".into()
                    } else {
                        sizes[..1].to_string()
                    },
                    sizes,
                    spans: vec![1, 2, 3],
                    span,
                    on: !plugins::hidden_at(c, contrib::HOME_WIDGET, &e.id),
                },
                e,
            ))
        })
        .collect()
}

/// The widgets as they stand: the saved layout checked against what exists.
fn slots(c: &crate::config::Config) -> (Vec<Slot>, Vec<(PluginDef, plugins::Contributed)>) {
    let p = plugin_widgets(c);
    let defs: Vec<PluginDef> = p.iter().map(|x| x.0.clone()).collect();
    (home::resolve(&c.home, &defs), p)
}

fn width_label(span: u8) -> &'static str {
    match span {
        1 => "⅓",
        2 => "⅔",
        _ => "Full",
    }
}

fn span_of(label: &str) -> u8 {
    match label {
        "⅓" => 1,
        "⅔" => 2,
        _ => 3,
    }
}

fn strs<I: IntoIterator<Item = String>>(v: I) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        v.into_iter().map(SharedString::from).collect::<Vec<_>>(),
    ))
}

/// Whether built-in widget `id` has anything to show.
fn filled(a: &AppWindow, id: &str) -> bool {
    let l = a.global::<crate::Library>();
    let n = match id {
        "continue" => l.get_continue().row_count(),
        "jump" | "recent" => l.get_recent().row_count(),
        "mixes" => l.get_mixes().row_count(),
        "added" => l.get_items().row_count(),
        "listen" => l.get_listen().row_count(),
        "favorites" => l.get_pinned().row_count(),
        "folders" => l.get_folders().row_count(),
        "artists" => l.get_artists().row_count(),
        "lists" => l.get_lists().row_count(),
        "streams" => a.global::<crate::Streams>().get_home().row_count(),
        "sources" => a.global::<crate::Extensions>().get_sources().row_count(),
        _ => 1,
    };
    n > 0
}

/// A plugin item's picture: a file in the plugin's data folder, or an https picture
/// fetched into the image cache in the background (the grid is re-packed when it lands).
fn picture(plugin: &str, image: &str) -> Option<slint::Image> {
    if image.is_empty() {
        return None;
    }
    if let Some(i) = PICS.with(|p| p.borrow().get(image).cloned()) {
        return Some(i);
    }
    if !ASKED.with(|a| a.borrow_mut().insert(image.to_string())) {
        return None;
    }
    let key = image.to_string();
    let decode = move |path: PathBuf, key: String| {
        let k = key.clone();
        let done = move |i: slint::Image| {
            PICS.with(|p| p.borrow_mut().insert(k, i));
            repack();
        };
        if let Some(i) = crate::images::get(&path, crate::images::TILE, done) {
            PICS.with(|p| p.borrow_mut().insert(key, i));
            repack();
        }
    };
    if image.starts_with("https://") {
        let url = image.to_string();
        std::thread::spawn(move || {
            if let Some(path) = crate::plugins::network_streams::fetch_logo(&url) {
                let _ = slint::invoke_from_event_loop(move || decode(path, key));
            }
        });
    } else if let Some(path) =
        crate::plugins::host::inside(&crate::plugins::host::data_dir(plugin), image)
    {
        decode(path, key);
    }
    None
}

fn plugin_items(plugin: &str, c: &content::Content) -> Vec<LibItem> {
    c.items
        .iter()
        .map(|it| {
            let thumb = picture(plugin, &it.image);
            LibItem {
                key: it.id.as_str().into(),
                title: it.title.as_str().into(),
                subtitle: it.sub.as_str().into(),
                kind: "plugin".into(),
                has_thumb: thumb.is_some(),
                thumb: thumb.unwrap_or_default(),
                icon: "plugins".into(),
                ..Default::default()
            }
        })
        .collect()
}

fn cell(s: &Slot, a: &AppWindow, plugin: Option<&plugins::Contributed>, edit: bool) -> HomeCell {
    let mut c = HomeCell {
        id: s.id.as_str().into(),
        kind: if s.plugin {
            "plugin".into()
        } else {
            s.id.as_str().into()
        },
        title: s.title.as_str().into(),
        size: s.size.as_str().into(),
        width: width_label(s.span).into(),
        size_opts: strs(s.sizes.chars().map(|c| c.to_ascii_uppercase().to_string())),
        width_opts: strs(s.spans.iter().map(|x| width_label(*x).to_string())),
        on: s.on,
        empty: false,
        progress: -1.0,
        template: "card".into(),
        ..Default::default()
    };
    if let Some(e) = plugin {
        let wid = format!("{}/{}", e.id, e.home.widget);
        let live = content::get(&wid, content::Surface::Home)
            .or_else(|| content::get(&e.id, content::Surface::Home));
        let info = plugins::manifests()
            .into_iter()
            .find(|m| m.id == e.id)
            .map(|m| m.description)
            .unwrap_or_default();
        let lc = live.clone().unwrap_or_default();
        let items = plugin_items(&e.id, &lc);
        let template = match e.home.template.as_str() {
            "tiles" | "list" if !items.is_empty() => e.home.template.as_str(),
            _ => "card",
        };
        let rows = match s.size.as_str() {
            "s" => 3,
            "l" => content::HOME_ROWS,
            _ => 5,
        };
        c.plugin = e.id.as_str().into();
        c.command = e.command.as_str().into();
        c.icon = e.icon.as_str().into();
        c.template = template.into();
        c.live = live.is_some();
        c.info = if lc.sub.is_empty() {
            info
        } else {
            lc.sub.clone()
        }
        .into();
        if !lc.title.is_empty() {
            c.title = lc.title.as_str().into();
        }
        c.value = lc.value.as_str().into();
        c.progress = lc.progress.unwrap_or(-1.0);
        c.rows = ModelRc::new(VecModel::from(lc.ui_rows(rows)));
        c.items = ModelRc::new(VecModel::from(items));
    } else {
        c.empty = !filled(a, &s.id);
    }
    let _ = edit;
    c
}

/// Rebuild `Library.home` from the layout, the data on screen and the window width.
pub fn repack() {
    let Some((a, cfg, narrow, edit)) = state() else {
        return;
    };
    let c = cfg.config();
    let (slots, plugs) = slots(&c);
    let cells: Vec<(HomeCell, u8)> = slots
        .iter()
        .map(|s| {
            let p = plugs.iter().find(|p| p.0.id == s.id).map(|p| &p.1);
            (cell(s, &a, p, edit), s.span)
        })
        .filter(|(c, _)| edit || (c.on && !c.empty))
        .map(|(c, span)| {
            let span = if edit && !c.on { 3 } else { span };
            (c, span)
        })
        .collect();
    let spans: Vec<u8> = cells.iter().map(|c| c.1).collect();
    let packed = home::pack(&spans, if narrow { 1 } else { 3 });
    let mut ids = Vec::new();
    let lines: Vec<HomeLine> = packed
        .iter()
        .map(|l| {
            ids.push(l.iter().map(|(i, _)| cells[*i].0.id.to_string()).collect());
            let v: Vec<HomeCell> = l
                .iter()
                .map(|(i, f)| HomeCell {
                    frac: *f,
                    ..cells[*i].0.clone()
                })
                .collect();
            HomeLine {
                cells: ModelRc::new(VecModel::from(v)),
            }
        })
        .collect();
    ST.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.lines = ids;
        }
    });
    let g = a.global::<crate::Library>();
    g.set_home(ModelRc::new(VecModel::from(lines)));
    g.set_home_edit(edit);
}

/// Next allowed value after `cur` in `all` (wrapping).
fn next<T: PartialEq + Copy>(all: &[T], cur: T) -> T {
    let i = all.iter().position(|x| *x == cur).map_or(0, |i| i + 1);
    all[i % all.len().max(1)]
}

/// Index in the widget list a dragged widget lands at when dropped on line `to`.
fn drop_index(lines: &[Vec<String>], slots: &[Slot], id: &str, to: usize) -> Option<usize> {
    let from = lines.iter().position(|l| l.iter().any(|x| x == id))?;
    let target = lines.get(to)?;
    if to == from {
        return None;
    }
    let at = |w: &String| slots.iter().position(|s| s.id == *w);
    if to > from {
        at(target.last()?)
    } else {
        at(target.first()?)
    }
}

/// Apply one edit. In edit mode it is saved on Done; elsewhere (Settings) right away.
pub fn edit(op: Op) {
    let Some((_, cfg, _, editing)) = state() else {
        return;
    };
    let c = cfg.config();
    let (slots, _) = slots(&c);
    let h = home::apply(&c.home, &slots, &op);
    if editing {
        cfg.send(AppMsg::SetHome(Box::new(h)));
    } else {
        cfg.send_and_save(AppMsg::SetHome(Box::new(h)));
    }
    repack();
    crate::settings::refresh_ui();
}

fn set_editing(on: bool) {
    let cfg = ST.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut()?;
        s.backup = on.then(|| s.cfg.config().home.clone());
        Some(s.cfg.clone())
    });
    if !on {
        if let Some(cfg) = cfg {
            cfg.save_or_log();
        }
    }
    repack();
}

/// `Library.home-action(action, widget id, value)` from the Home screen.
pub fn action(a: &str, id: &str, v: &str) {
    let Some((_, cfg, _, _)) = state() else {
        return;
    };
    let c = cfg.config();
    let (slots, _) = slots(&c);
    let slot = slots.iter().find(|s| s.id == id);
    match a {
        "edit" => set_editing(true),
        "refresh" => {
            if let Some((a, ..)) = state() {
                crate::extwin::refresh_widgets();
                a.global::<crate::Library>().invoke_refresh();
                a.global::<crate::Library>().invoke_rescan();
            }
        }
        "done" => set_editing(false),
        "cancel" => {
            let backup = ST.with(|s| s.borrow_mut().as_mut().and_then(|s| s.backup.take()));
            if let Some(b) = backup {
                cfg.send(AppMsg::SetHome(Box::new(b)));
            }
            set_editing(false);
        }
        "reset" => {
            cfg.send(AppMsg::SetHome(Box::default()));
            if ST.with(|s| s.borrow().as_ref().is_some_and(|s| s.backup.is_none())) {
                cfg.save_or_log();
            }
            repack();
            crate::settings::refresh_ui();
        }
        "up" => edit(Op::Step(id.into(), -1)),
        "down" => edit(Op::Step(id.into(), 1)),
        "toggle" => edit(Op::Toggle(id.into(), v == "true")),
        "size" => edit(Op::Size(id.into(), v.to_ascii_lowercase())),
        "width" => edit(Op::Span(id.into(), span_of(v))),
        "size-next" => {
            if let Some(s) = slot {
                let all: Vec<char> = s.sizes.chars().collect();
                let cur = s.size.chars().next().unwrap_or('m');
                edit(Op::Size(id.into(), next(&all, cur).to_string()));
            }
        }
        "width-next" => {
            if let Some(s) = slot {
                edit(Op::Span(id.into(), next(&s.spans, s.span)));
            }
        }
        "drop" => {
            let lines = ST.with(|s| s.borrow().as_ref().map(|s| s.lines.clone()));
            let to = v.parse::<usize>().ok();
            if let (Some(lines), Some(to)) = (lines, to) {
                if let Some(n) = drop_index(&lines, &slots, id, to) {
                    edit(Op::MoveTo(id.into(), n));
                }
            }
        }
        _ => {}
    }
}

/// Ids of plugin `pid`'s Home widgets, in order.
fn plugin_slots(pid: &str) -> Vec<Slot> {
    let Some((_, cfg, _, _)) = state() else {
        return Vec::new();
    };
    let prefix = format!("plug:{pid}:");
    slots(&cfg.config())
        .0
        .into_iter()
        .filter(|s| s.id.starts_with(&prefix))
        .collect()
}

/// Whether any Home widget of plugin `pid` is shown (Extensions → *Show on Home*).
pub fn plugin_on(pid: &str) -> bool {
    plugin_slots(pid).iter().any(|s| s.on)
}

/// Extensions → *Show on Home* / *Position on Home* for plugin `pid`.
pub fn plugin_arrange(pid: &str, what: &str) {
    for s in plugin_slots(pid) {
        match what {
            "show" | "hide" => edit(Op::Toggle(s.id, what == "show")),
            "up" => edit(Op::Step(s.id, -1)),
            "down" => edit(Op::Step(s.id, 1)),
            _ => {}
        }
    }
}

/// A `home.<…>` row of Settings → Interface → Home changed.
pub fn setting(rest: &str, v: &str) {
    match rest {
        "customize" => {
            if let Some((a, ..)) = state() {
                a.global::<crate::Overlays>().set_settings_open(false);
                a.global::<crate::Overlays>().set_library_open(true);
                a.global::<crate::Library>().invoke_navigate("home".into());
            }
            set_editing(true);
        }
        "reset" => action("reset", "", ""),
        _ => {
            let Some((id, what)) = rest.rsplit_once('.') else {
                return;
            };
            match what {
                "on" => edit(Op::Toggle(id.into(), v == "true")),
                "size" => edit(Op::Size(id.into(), v.to_ascii_lowercase())),
                "width" => edit(Op::Span(id.into(), span_of(v))),
                "move" => edit(Op::Step(id.into(), if v == "Move up" { -1 } else { 1 })),
                _ => {}
            }
        }
    }
}

/// One widget as Settings → Interface → Home lists it.
pub struct SettingsRow {
    pub id: String,
    pub title: String,
    pub on: bool,
    /// "S" | "M" | "L"
    pub size: String,
    /// "⅓" | "⅔" | "Full"
    pub width: String,
    pub sizes: Vec<String>,
    pub widths: Vec<String>,
}

/// The widgets for Settings → Interface → Home, in order.
pub fn rows() -> Vec<SettingsRow> {
    let Some((_, cfg, _, _)) = state() else {
        return Vec::new();
    };
    let (slots, _) = slots(&cfg.config());
    slots
        .into_iter()
        .map(|s| SettingsRow {
            size: s.size.to_ascii_uppercase(),
            width: width_label(s.span).to_string(),
            sizes: s
                .sizes
                .chars()
                .map(|c| c.to_ascii_uppercase().to_string())
                .collect(),
            widths: s
                .spans
                .iter()
                .map(|x| width_label(*x).to_string())
                .collect(),
            id: s.id,
            title: s.title,
            on: s.on,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_land_before_or_after_the_target_line() {
        let slots = home::resolve(&Home::default(), &[]);
        let id = |i: usize| slots[i].id.clone();
        // Lines: [0,1] [2] [3]
        let lines = vec![vec![id(0), id(1)], vec![id(2)], vec![id(3)]];
        assert_eq!(
            drop_index(&lines, &slots, &id(0), 2),
            Some(3),
            "down: after"
        );
        assert_eq!(drop_index(&lines, &slots, &id(3), 0), Some(0), "up: before");
        assert_eq!(drop_index(&lines, &slots, &id(1), 0), None, "same line");
        assert_eq!(next(&[1u8, 2, 3], 3), 1);
        assert_eq!(span_of(width_label(2)), 2);
    }
}
