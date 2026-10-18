//! Command palette (Ctrl+K, ui/palette.slint): every keymap action plus a few app
//! commands, and the plugins' `palette.command` contributions, with a fuzzy filter.
//! A keymap action runs by replaying its key through the window's `key` callback, so
//! the palette and the shortcut always do the same thing.

use crate::keymap::{self, Action, Mod, K};
use crate::plugins::{self, contrib};
use crate::{AppWindow, PaletteItem};
use slint::{ComponentHandle, ModelRc, VecModel};

/// One palette entry before filtering.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub group: String,
    pub hint: String,
}

fn entries() -> Vec<Entry> {
    let mut v: Vec<Entry> = keymap::BINDINGS
        .iter()
        .enumerate()
        .filter(|(_, b)| !matches!(b.action, Action::Escape | Action::Palette))
        .map(|(i, b)| Entry {
            id: format!("key:{i}"),
            label: b.label.to_string(),
            group: String::new(),
            hint: b.shown.split("  ").next().unwrap_or_default().to_string(),
        })
        .collect();
    let app = |id: &str, label: &str| Entry {
        id: id.into(),
        label: label.into(),
        group: String::new(),
        hint: String::new(),
    };
    v.push(app("app:marketplace", "Extensions: All / Marketplace"));
    v.push(app("app:customize", "Customize Player"));
    v.push(app("app:network", "Open Network Stream"));
    for e in plugins::contributions(contrib::DRAWER_PANEL) {
        v.push(Entry {
            id: format!("drawer:{}", e.id),
            label: format!("Side drawer: {}", e.label),
            group: e.name,
            hint: String::new(),
        });
    }
    for e in plugins::contributions(contrib::PALETTE_COMMAND) {
        // A plugin command that is also a shortcut action is listed once (with its key).
        if v.iter().any(|x| x.label.eq_ignore_ascii_case(&e.label)) {
            continue;
        }
        v.push(Entry {
            id: format!("plug:{}|{}", e.id, e.command),
            label: e.label,
            group: e.name,
            hint: String::new(),
        });
    }
    // Sources open from the palette too.
    for e in plugins::contributions(contrib::SIDEBAR_SOURCE) {
        v.push(Entry {
            id: format!("plug:{}|{}", e.id, e.command),
            label: format!("Open {}", e.label),
            group: "Sources".into(),
            hint: String::new(),
        });
    }
    v
}

/// Fuzzy score of `q` in `text` (case-insensitive subsequence); higher is better.
/// Consecutive matches and matches at word starts count more; None = no match.
pub fn score(q: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = q
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut qi, mut s, mut prev) = (0, 0, None::<usize>);
    for (i, c) in t.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            s += 1;
            if prev == Some(i.wrapping_sub(1)) {
                s += 3;
            }
            if i == 0 || !t[i - 1].is_alphanumeric() {
                s += 4;
            }
            prev = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(s - (t.len() as i32) / 16)
}

/// Entries matching `q`, best first (stable for equal scores).
pub fn filter(q: &str, all: Vec<Entry>) -> Vec<Entry> {
    let mut v: Vec<(i32, Entry)> = all
        .into_iter()
        .filter_map(|e| {
            let hay = format!("{} {}", e.label, e.group);
            score(q, &hay).map(|s| (s, e))
        })
        .collect();
    v.sort_by_key(|(s, _)| -s);
    v.into_iter().map(|(_, e)| e).collect()
}

fn show(a: &AppWindow, q: &str) {
    let items: Vec<PaletteItem> = filter(q, entries())
        .into_iter()
        .map(|e| PaletteItem {
            id: e.id.into(),
            label: e.label.into(),
            group: e.group.into(),
            hint: e.hint.into(),
        })
        .collect();
    a.global::<crate::Palette>()
        .set_items(ModelRc::new(VecModel::from(items)));
}

pub fn setup(app: &AppWindow) {
    let w = app.as_weak();
    app.global::<crate::Palette>().on_search(move |q| {
        if let Some(a) = w.upgrade() {
            show(&a, &q);
        }
    });
    let w = app.as_weak();
    app.global::<crate::Palette>().on_run(move |id| {
        let Some(a) = w.upgrade() else { return };
        a.global::<crate::Overlays>().set_palette_open(false);
        a.invoke_focus_main();
        run(&a, &id);
    });
}

/// Ctrl+K: open (fresh list) or close the palette.
pub fn toggle(a: &AppWindow) {
    let o = a.global::<crate::Overlays>();
    if o.get_palette_open() {
        o.set_palette_open(false);
        return;
    }
    a.global::<crate::Palette>().set_query("".into());
    a.global::<crate::Palette>().set_selected(0);
    show(a, "");
    o.set_palette_open(true);
}

fn run(a: &AppWindow, id: &str) {
    let (head, arg) = id.split_once(':').unwrap_or((id, ""));
    match head {
        "key" => {
            let Some(b) = arg
                .parse::<usize>()
                .ok()
                .and_then(|i| keymap::BINDINGS.get(i))
            else {
                return;
            };
            let text: slint::SharedString = match b.keys[0] {
                K::Text(t) => t.into(),
                K::Named(k) => k.into(),
            };
            a.invoke_key(
                text,
                b.shift == Mod::On,
                b.ctrl == Mod::On,
                b.alt == Mod::On,
            );
        }
        "plug" => {
            let (pid, cmd) = arg.split_once('|').unwrap_or((arg, ""));
            plugins::run(pid, cmd, &[]);
        }
        "drawer" => {
            crate::drawer::open(a, arg);
        }
        _ => match arg {
            "marketplace" => crate::extwin::open(a, "", false),
            "customize" => a.global::<crate::Overlays>().set_customize_open(true),
            "network" => a.global::<crate::Overlays>().set_network_open(true),
            _ => {}
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_order() {
        assert!(score("xyz", "Fullscreen").is_none());
        assert!(score("fs", "Fullscreen").is_some());
        let e = |l: &str| Entry {
            id: l.into(),
            label: l.into(),
            group: String::new(),
            hint: String::new(),
        };
        let v = filter(
            "sub",
            vec![
                e("Find lyrics"),
                e("Search subtitles online"),
                e("Subtitle offset"),
            ],
        );
        let ids: Vec<&str> = v.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["Subtitle offset", "Search subtitles online"]);
        assert_eq!(filter("", vec![e("a"), e("b")]).len(), 2);
    }

    #[test]
    fn every_binding_replays() {
        // Each listed action (Escape and the palette itself excepted) maps back to itself.
        for b in keymap::BINDINGS {
            if matches!(b.action, Action::Escape | Action::Palette) {
                continue;
            }
            let text: slint::SharedString = match b.keys[0] {
                K::Text(t) => t.into(),
                K::Named(k) => k.into(),
            };
            assert_eq!(
                keymap::lookup(
                    &text,
                    b.shift == Mod::On,
                    b.ctrl == Mod::On,
                    b.alt == Mod::On
                ),
                Some(b.action),
                "{}",
                b.label
            );
        }
    }
}
