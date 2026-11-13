//! Configurable Home: a list of widgets, each with on / off, position, size and width.
//!
//! Built-in widgets come from [`BUILTIN`]; plugins add theirs with the `home.widget`
//! contribution point. The user's choices live in `[home]` of the config (an empty list
//! means the defaults). Widgets are laid out in a 3-column grid: each takes ⅓, ⅔ or the
//! full width ([`pack`]); narrow windows use one column.

use serde::{Deserialize, Serialize};

/// `[home]` in the config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Home {
    pub version: u32,
    /// In display order. Empty = the defaults. Ids no longer known (an uninstalled
    /// plugin) are kept so the widget comes back in its place.
    pub widgets: Vec<HomeWidget>,
}

impl Default for Home {
    fn default() -> Self {
        Self {
            version: 1,
            widgets: Vec::new(),
        }
    }
}

/// One widget's settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HomeWidget {
    pub id: String,
    pub on: bool,
    /// "s" | "m" | "l"
    pub size: String,
    /// Columns out of 3.
    pub span: u8,
}

impl Default for HomeWidget {
    fn default() -> Self {
        Self {
            id: String::new(),
            on: true,
            size: "m".into(),
            span: 3,
        }
    }
}

/// A built-in widget.
pub struct Def {
    pub id: &'static str,
    pub title: &'static str,
    /// Allowed sizes, e.g. "sml".
    pub sizes: &'static str,
    /// Allowed widths in columns.
    pub spans: &'static [u8],
    pub on: bool,
    pub size: &'static str,
    pub span: u8,
}

const fn def(
    id: &'static str,
    title: &'static str,
    sizes: &'static str,
    spans: &'static [u8],
    on: bool,
    size: &'static str,
    span: u8,
) -> Def {
    Def {
        id,
        title,
        sizes,
        spans,
        on,
        size,
        span,
    }
}

/// Built-in widgets in their default order: Continue Watching ⅔ + Jump Back In ⅓,
/// Your Mixes ⅓ + Recently Added ⅔, then Listen Again full. Other shelves start off;
/// plugin widgets the user never placed are appended at full width.
pub const BUILTIN: &[Def] = &[
    def(
        "continue",
        "Continue Watching",
        "sml",
        &[1, 2, 3],
        true,
        "l",
        2,
    ),
    def("jump", "Jump Back In", "sm", &[1, 2], true, "m", 1),
    def("mixes", "Your Mixes", "sml", &[1, 2, 3], true, "m", 1),
    def("added", "Recently Added", "sml", &[1, 2, 3], true, "m", 2),
    def("listen", "Listen Again", "sml", &[1, 2, 3], true, "m", 3),
    def("favorites", "Favorites", "sml", &[1, 2, 3], false, "m", 3),
    def("streams", "Streams", "s", &[1, 2, 3], false, "s", 3),
    def("sources", "Sources", "s", &[1, 2, 3], false, "s", 3),
    def(
        "folders",
        "Frequently Visited Folders",
        "sm",
        &[1, 2, 3],
        false,
        "m",
        3,
    ),
    def(
        "artists",
        "Frequently Listened Artists",
        "sm",
        &[1, 2, 3],
        false,
        "m",
        3,
    ),
    def(
        "lists",
        "Frequently Visited Playlists",
        "sm",
        &[1, 2, 3],
        false,
        "m",
        3,
    ),
    def("recent", "Recently Played", "sm", &[1, 2, 3], false, "m", 3),
];

/// A plugin's Home widget, as the host reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginDef {
    /// `plug:<plugin id>:<widget id>`
    pub id: String,
    pub title: String,
    pub sizes: String,
    pub spans: Vec<u8>,
    pub size: String,
    pub span: u8,
    /// Shown when the user has not placed it yet (the old `ui.home-hidden` setting).
    pub on: bool,
}

/// Id of a plugin widget.
pub fn plugin_id(plugin: &str, widget: &str) -> String {
    format!("plug:{plugin}:{widget}")
}

/// Allowed sizes of a manifest list ("s", "m", "l"; anything else is dropped).
pub fn sizes_of(v: &[String]) -> String {
    let mut s = String::new();
    for c in ["s", "m", "l"] {
        if v.iter().any(|x| x.eq_ignore_ascii_case(c)) {
            s.push_str(c);
        }
    }
    if s.is_empty() {
        s.push('m');
    }
    s
}

/// A widget with its settings checked against what it allows.
#[derive(Debug, Clone, PartialEq)]
pub struct Slot {
    pub id: String,
    pub title: String,
    pub sizes: String,
    pub spans: Vec<u8>,
    pub on: bool,
    pub size: String,
    pub span: u8,
    pub plugin: bool,
}

impl Slot {
    fn widget(&self) -> HomeWidget {
        HomeWidget {
            id: self.id.clone(),
            on: self.on,
            size: self.size.clone(),
            span: self.span,
        }
    }
}

fn builtin(id: &str) -> Option<&'static Def> {
    BUILTIN.iter().find(|d| d.id == id)
}

fn slot_of_def(d: &Def) -> Slot {
    Slot {
        id: d.id.into(),
        title: d.title.into(),
        sizes: d.sizes.into(),
        spans: d.spans.to_vec(),
        on: d.on,
        size: d.size.into(),
        span: d.span,
        plugin: false,
    }
}

fn slot_of_plugin(p: &PluginDef) -> Slot {
    Slot {
        id: p.id.clone(),
        title: p.title.clone(),
        sizes: p.sizes.clone(),
        spans: p.spans.clone(),
        on: p.on,
        size: p.size.clone(),
        span: p.span,
        plugin: true,
    }
}

/// Nearest allowed size: the asked one, else the middle-most allowed one.
fn fit_size(want: &str, sizes: &str) -> String {
    if want.len() == 1 && sizes.contains(want) {
        return want.into();
    }
    if sizes.contains('m') {
        return "m".into();
    }
    sizes.chars().next().unwrap_or('m').to_string()
}

/// Nearest allowed width.
fn fit_span(want: u8, spans: &[u8]) -> u8 {
    spans
        .iter()
        .copied()
        .min_by_key(|s| (i16::from(*s) - i16::from(want)).abs() * 2 - i16::from(*s > want))
        .unwrap_or(3)
}

/// The widgets to show, in order: the user's list checked against what exists, new
/// built-ins at their default place, plugin widgets the user never placed at the end.
pub fn resolve(home: &Home, plugins: &[PluginDef]) -> Vec<Slot> {
    let mut out: Vec<Slot> = Vec::new();
    if home.widgets.is_empty() {
        out.extend(BUILTIN.iter().map(slot_of_def));
    } else {
        for w in &home.widgets {
            if out.iter().any(|s| s.id == w.id) {
                continue;
            }
            let base = if let Some(d) = builtin(&w.id) {
                slot_of_def(d)
            } else if let Some(p) = plugins.iter().find(|p| p.id == w.id) {
                slot_of_plugin(p)
            } else {
                continue;
            };
            out.push(Slot {
                on: w.on,
                size: fit_size(&w.size, &base.sizes),
                span: fit_span(w.span, &base.spans),
                ..base
            });
        }
        // Built-ins added in a later version: after the built-in that precedes them.
        for (i, d) in BUILTIN.iter().enumerate() {
            if out.iter().any(|s| s.id == d.id) {
                continue;
            }
            let at = BUILTIN[..i]
                .iter()
                .rev()
                .find_map(|p| out.iter().position(|s| s.id == p.id))
                .map_or(0, |p| p + 1);
            out.insert(at, slot_of_def(d));
        }
    }
    for p in plugins {
        if !out.iter().any(|s| s.id == p.id) {
            // Never placed by the user: append at full width (⅓ / ⅔ if the plugin forbids 3).
            out.push(Slot {
                size: fit_size(&p.size, &p.sizes),
                span: fit_span(3, &p.spans),
                ..slot_of_plugin(p)
            });
        }
    }
    out
}

/// One edit of the Home layout.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Move a widget one place up (-1) or down (+1).
    Step(String, i32),
    /// Move a widget to index `n` of the shown list.
    MoveTo(String, usize),
    Toggle(String, bool),
    Size(String, String),
    Span(String, u8),
}

/// `home` after `op`, given the widgets shown now (`slots` = [`resolve`]). Entries of
/// unknown widgets stay at their index.
pub fn apply(home: &Home, slots: &[Slot], op: &Op) -> Home {
    let mut v: Vec<Slot> = slots.to_vec();
    let find = |v: &[Slot], id: &str| v.iter().position(|s| s.id == id);
    match op {
        Op::Step(id, d) => {
            if let Some(i) = find(&v, id) {
                let j = (i as i64 + i64::from(*d)).clamp(0, v.len() as i64 - 1) as usize;
                let s = v.remove(i);
                v.insert(j, s);
            }
        }
        Op::MoveTo(id, n) => {
            if let Some(i) = find(&v, id) {
                let s = v.remove(i);
                v.insert((*n).min(v.len()), s);
            }
        }
        Op::Toggle(id, on) => {
            if let Some(i) = find(&v, id) {
                v[i].on = *on;
            }
        }
        Op::Size(id, size) => {
            if let Some(i) = find(&v, id) {
                v[i].size = fit_size(size, &v[i].sizes);
            }
        }
        Op::Span(id, span) => {
            if let Some(i) = find(&v, id) {
                v[i].span = fit_span(*span, &v[i].spans);
            }
        }
    }
    let mut widgets: Vec<HomeWidget> = v.iter().map(Slot::widget).collect();
    for (i, w) in home.widgets.iter().enumerate() {
        if !slots.iter().any(|s| s.id == w.id) && !widgets.iter().any(|x| x.id == w.id) {
            widgets.insert(i.min(widgets.len()), w.clone());
        }
    }
    Home {
        version: 1,
        widgets,
    }
}

/// Lines of the grid: (index into `spans`, fraction of the line width). Widgets fill
/// lines of `cols` columns in order; one that does not fit starts the next line and the
/// last widget of the line before takes the leftover room, so no line has a gap.
pub fn pack(spans: &[u8], cols: u8) -> Vec<Vec<(usize, f32)>> {
    let cols = cols.max(1);
    let mut lines: Vec<Vec<(usize, u8)>> = Vec::new();
    let mut used = 0u8;
    for (i, s) in spans.iter().enumerate() {
        let s = (*s).clamp(1, cols);
        if lines.is_empty() || used + s > cols {
            lines.push(Vec::new());
            used = 0;
        }
        lines.last_mut().unwrap().push((i, s));
        used += s;
    }
    lines
        .into_iter()
        .map(|mut l| {
            let total: u8 = l.iter().map(|x| x.1).sum();
            if let Some(last) = l.last_mut() {
                last.1 += cols - total;
            }
            l.into_iter()
                .map(|(i, s)| (i, f32::from(s) / f32::from(cols)))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plug(id: &str, on: bool) -> PluginDef {
        PluginDef {
            id: plugin_id(id, "show"),
            title: id.into(),
            sizes: "m".into(),
            spans: vec![1, 2],
            size: "m".into(),
            span: 1,
            on,
        }
    }

    #[test]
    fn defaults_are_continue_jump_then_mixes_added_then_listen() {
        let v = resolve(&Home::default(), &[plug("stats", true), plug("x", false)]);
        assert_eq!(v[0].id, "continue");
        assert_eq!((v[0].size.as_str(), v[0].span), ("l", 2));
        assert_eq!((v[1].id.as_str(), v[1].span), ("jump", 1));
        assert_eq!((v[2].id.as_str(), v[2].span), ("mixes", 1));
        assert_eq!((v[3].id.as_str(), v[3].span), ("added", 2));
        assert_eq!((v[4].id.as_str(), v[4].span), ("listen", 3));
        for id in [
            "favorites",
            "streams",
            "sources",
            "folders",
            "artists",
            "lists",
            "recent",
        ] {
            assert!(
                !v.iter().find(|s| s.id == id).unwrap().on,
                "{id} starts off"
            );
        }
        let n = v.len();
        assert_eq!(v[n - 2].id, "plug:stats:show");
        assert!(v[n - 2].on);
        assert_eq!(
            v[n - 2].span,
            2,
            "full width if allowed, else the widest the plugin allows"
        );
        assert!(!v[n - 1].on, "the old hidden flag carries over");
    }

    #[test]
    fn unplaced_plugins_append_at_full_width_when_allowed() {
        let p = PluginDef {
            id: plugin_id("wide", "show"),
            title: "wide".into(),
            sizes: "m".into(),
            spans: vec![1, 2, 3],
            size: "m".into(),
            span: 1,
            on: true,
        };
        let v = resolve(&Home::default(), &[p]);
        let w = v.iter().find(|s| s.id == "plug:wide:show").unwrap();
        assert_eq!(w.span, 3);
    }

    #[test]
    fn saved_home_layout_is_not_overwritten_by_new_defaults() {
        // A layout saved before the new template (Mixes and Added full width, Favorites on).
        let home = Home {
            version: 1,
            widgets: vec![
                HomeWidget {
                    id: "continue".into(),
                    on: true,
                    size: "l".into(),
                    span: 2,
                },
                HomeWidget {
                    id: "jump".into(),
                    on: true,
                    size: "m".into(),
                    span: 1,
                },
                HomeWidget {
                    id: "mixes".into(),
                    on: true,
                    size: "m".into(),
                    span: 3,
                },
                HomeWidget {
                    id: "added".into(),
                    on: true,
                    size: "m".into(),
                    span: 3,
                },
                HomeWidget {
                    id: "listen".into(),
                    on: true,
                    size: "m".into(),
                    span: 3,
                },
                HomeWidget {
                    id: "favorites".into(),
                    on: true,
                    size: "m".into(),
                    span: 3,
                },
            ],
        };
        let keep = home.clone();
        let v = resolve(&home, &[]);
        assert_eq!(home, keep, "[home] in the config is not rewritten");
        assert_eq!(v.iter().find(|s| s.id == "mixes").unwrap().span, 3);
        assert_eq!(v.iter().find(|s| s.id == "added").unwrap().span, 3);
        assert!(v.iter().find(|s| s.id == "favorites").unwrap().on);
    }

    #[test]
    fn saved_layout_is_checked_and_new_builtins_are_inserted() {
        let home = Home {
            version: 1,
            widgets: vec![
                HomeWidget {
                    id: "added".into(),
                    on: false,
                    size: "xl".into(),
                    span: 1,
                },
                HomeWidget {
                    id: "plug:gone:show".into(),
                    ..Default::default()
                },
                HomeWidget {
                    id: "continue".into(),
                    ..Default::default()
                },
            ],
        };
        let v = resolve(&home, &[]);
        assert_eq!(v[0].id, "added");
        assert!(!v[0].on);
        assert_eq!(
            (v[0].size.as_str(), v[0].span),
            ("m", 1),
            "size clamped, ⅓ kept"
        );
        let at = |id: &str| v.iter().position(|s| s.id == id).unwrap();
        assert_eq!(
            v[1].id, "listen",
            "missing built-ins follow their predecessor"
        );
        assert_eq!(at("jump"), at("continue") + 1);
        assert!(!v.iter().any(|s| s.id == "plug:gone:show"));
        assert_eq!(v.len(), BUILTIN.len());
        // Editing keeps the unknown entry at its index.
        let h = apply(&home, &v, &Op::Step("continue".into(), -1));
        assert_eq!(h.widgets[1].id, "plug:gone:show");
        let ids: Vec<String> = resolve(&h, &[]).into_iter().map(|s| s.id).collect();
        let was = v.iter().position(|s| s.id == "continue").unwrap();
        assert_eq!(ids.iter().position(|s| s == "continue"), Some(was - 1));
    }

    #[test]
    fn ops_move_toggle_and_resize() {
        let home = Home::default();
        let v = resolve(&home, &[]);
        let h = apply(&home, &v, &Op::MoveTo("recent".into(), 0));
        assert_eq!(h.widgets[0].id, "recent");
        let v = resolve(&h, &[]);
        let h = apply(&h, &v, &Op::Toggle("recent".into(), true));
        assert!(h.widgets[0].on);
        let v = resolve(&h, &[]);
        let h = apply(&h, &v, &Op::Size("continue".into(), "s".into()));
        let h2 = apply(&h, &resolve(&h, &[]), &Op::Span("jump".into(), 3));
        let c = h2.widgets.iter().find(|w| w.id == "continue").unwrap();
        assert_eq!(c.size, "s");
        let j = h2.widgets.iter().find(|w| w.id == "jump").unwrap();
        assert_eq!(j.span, 2, "jump allows ⅓ and ⅔ only");
        let h3 = apply(&h2, &resolve(&h2, &[]), &Op::Step("recent".into(), -1));
        assert_eq!(h3.widgets[0].id, "recent", "the first one stays first");
    }

    #[test]
    fn shelves_allow_one_third_width() {
        for d in BUILTIN {
            if d.id == "jump" {
                assert!(d.spans.contains(&1) && d.spans.contains(&2));
                continue;
            }
            assert!(
                d.spans.contains(&1) && d.spans.contains(&2) && d.spans.contains(&3),
                "{} should allow ⅓, ⅔ and Full",
                d.id
            );
        }
    }

    #[test]
    fn pack_fills_lines_without_gaps() {
        let l = pack(&[2, 1, 3, 1, 2, 2], 3);
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], vec![(0, 2.0 / 3.0), (1, 1.0 / 3.0)]);
        assert_eq!(l[1], vec![(2, 1.0)]);
        assert_eq!(l[2], vec![(3, 1.0 / 3.0), (4, 2.0 / 3.0)]);
        assert_eq!(l[3], vec![(5, 1.0)], "a lone ⅔ widget stretches");
        let narrow = pack(&[2, 1], 1);
        assert_eq!(narrow, vec![vec![(0, 1.0)], vec![(1, 1.0)]]);
        assert!(pack(&[], 3).is_empty());
    }
}
