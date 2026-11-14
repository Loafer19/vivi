//! Configurable Home: a list of widgets, each with on / off, position, and a size.
//!
//! Built-in widgets come from [`BUILTIN`]; plugins add theirs with the `home.widget`
//! contribution point. The user's choices live in `[home]` of the config (an empty list
//! means the defaults). Size is one control (S, M, L): S is ⅓ of the row, M is ⅔, L is
//! the full width, each with a matching height. Widgets are laid out in a 3-column grid
//! ([`pack`]); a row still shares one height. Narrow windows use one column.
//!
//! `[home]` version 1 stored size and width separately. [`migrate`] turns a saved width
//! into the nearest new size so the grid columns do not jump.

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
            version: VERSION,
            widgets: Vec::new(),
        }
    }
}

/// Current `[home]` schema: one size (S/M/L) that also sets the width.
pub const VERSION: u32 = 2;

/// One widget's settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HomeWidget {
    pub id: String,
    pub on: bool,
    /// "s" | "m" | "l" — also the width: S=⅓, M=⅔, L=full.
    pub size: String,
    /// Columns out of 3 (derived from [`Self::size`]; kept so older configs and
    /// tests still round-trip).
    #[serde(default)]
    pub span: u8,
}

impl Default for HomeWidget {
    fn default() -> Self {
        Self {
            id: String::new(),
            on: true,
            size: "l".into(),
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
    pub on: bool,
    pub size: &'static str,
}

const fn def(
    id: &'static str,
    title: &'static str,
    sizes: &'static str,
    on: bool,
    size: &'static str,
) -> Def {
    Def {
        id,
        title,
        sizes,
        on,
        size,
    }
}

/// Built-in widgets in their default order: Continue Watching M + Jump Back In S,
/// Your Mixes S + Recently Added M, then Listen Again L. Other shelves start off
/// at L; plugin widgets the user never placed are appended at L if they allow it.
pub const BUILTIN: &[Def] = &[
    def("continue", "Continue Watching", "sml", true, "m"),
    def("jump", "Jump Back In", "sm", true, "s"),
    def("mixes", "Your Mixes", "sml", true, "s"),
    def("added", "Recently Added", "sml", true, "m"),
    def("listen", "Listen Again", "sml", true, "l"),
    def("favorites", "Favorites", "sml", false, "l"),
    def("streams", "Streams", "sml", false, "l"),
    def("sources", "Sources", "sml", false, "l"),
    def("folders", "Frequently Visited Folders", "sml", false, "l"),
    def("artists", "Frequently Listened Artists", "sml", false, "l"),
    def("lists", "Frequently Visited Playlists", "sml", false, "l"),
    def("recent", "Recently Played", "sml", false, "l"),
];

/// A plugin's Home widget, as the host reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginDef {
    /// `plug:<plugin id>:<widget id>`
    pub id: String,
    pub title: String,
    pub sizes: String,
    pub size: String,
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
        // Omitted list: every size is allowed; the default size is L.
        s.push_str("sml");
    }
    s
}

/// Columns for a size: S=1, M=2, L=3.
pub fn span_of_size(size: &str) -> u8 {
    match size {
        "s" => 1,
        "l" => 3,
        _ => 2,
    }
}

/// Size that preserves a v1 width: 1→S, 2→M, 3→L.
pub fn size_of_span(span: u8) -> &'static str {
    match span {
        1 => "s",
        3 => "l",
        _ => "m",
    }
}

/// Allowed column spans, in the same order as [`sizes_of`].
pub fn spans_of(sizes: &str) -> Vec<u8> {
    sizes
        .chars()
        .filter_map(|c| match c {
            's' => Some(1),
            'm' => Some(2),
            'l' => Some(3),
            _ => None,
        })
        .collect()
}

/// Rewrite a v1 `[home]` list so the saved width becomes the new size. The
/// widget order and on/off flags stay put; columns stay the same (a ⅔ widget
/// becomes M, a full-width widget becomes L). Height follows the new size.
pub fn migrate(home: &mut Home) {
    if home.version >= VERSION {
        return;
    }
    for w in &mut home.widgets {
        w.size = size_of_span(w.span).into();
        w.span = span_of_size(&w.size);
    }
    home.version = VERSION;
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
        spans: spans_of(d.sizes),
        on: d.on,
        size: d.size.into(),
        span: span_of_size(d.size),
        plugin: false,
    }
}

fn slot_of_plugin(p: &PluginDef) -> Slot {
    let size = fit_size(&p.size, &p.sizes);
    Slot {
        id: p.id.clone(),
        title: p.title.clone(),
        sizes: p.sizes.clone(),
        spans: spans_of(&p.sizes),
        on: p.on,
        span: span_of_size(&size),
        size,
        plugin: true,
    }
}

/// Nearest allowed size: the asked one, else the middle-most allowed one.
pub fn fit_size(want: &str, sizes: &str) -> String {
    if want.len() == 1 && sizes.contains(want) {
        return want.into();
    }
    if sizes.contains('m') {
        return "m".into();
    }
    sizes.chars().next().unwrap_or('m').to_string()
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
            let size = if home.version < VERSION {
                // v1: keep the saved width so the grid does not jump.
                fit_size(size_of_span(w.span), &base.sizes)
            } else {
                fit_size(&w.size, &base.sizes)
            };
            let span = span_of_size(&size);
            out.push(Slot {
                on: w.on,
                size,
                span,
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
            // Never placed by the user: append at L (or the widest size they allow).
            out.push(Slot {
                size: fit_size("l", &p.sizes),
                span: span_of_size(&fit_size("l", &p.sizes)),
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
                v[i].span = span_of_size(&v[i].size);
            }
        }
        // Legacy width control: treated as picking the size for that width.
        Op::Span(id, span) => {
            if let Some(i) = find(&v, id) {
                v[i].size = fit_size(size_of_span(*span), &v[i].sizes);
                v[i].span = span_of_size(&v[i].size);
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
        version: VERSION,
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
            size: "m".into(),
            on,
        }
    }

    #[test]
    fn defaults_are_continue_jump_then_mixes_added_then_listen() {
        let v = resolve(&Home::default(), &[plug("stats", true), plug("x", false)]);
        assert_eq!(v[0].id, "continue");
        assert_eq!((v[0].size.as_str(), v[0].span), ("m", 2));
        assert_eq!(
            (v[1].id.as_str(), v[1].size.as_str(), v[1].span),
            ("jump", "s", 1)
        );
        assert_eq!(
            (v[2].id.as_str(), v[2].size.as_str(), v[2].span),
            ("mixes", "s", 1)
        );
        assert_eq!(
            (v[3].id.as_str(), v[3].size.as_str(), v[3].span),
            ("added", "m", 2)
        );
        assert_eq!(
            (v[4].id.as_str(), v[4].size.as_str(), v[4].span),
            ("listen", "l", 3)
        );
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
            (v[n - 2].size.as_str(), v[n - 2].span),
            ("m", 2),
            "L if allowed, else the widest the plugin allows"
        );
        assert!(!v[n - 1].on, "the old hidden flag carries over");
    }

    #[test]
    fn unplaced_plugins_append_at_full_width_when_allowed() {
        let p = PluginDef {
            id: plugin_id("wide", "show"),
            title: "wide".into(),
            sizes: "sml".into(),
            size: "m".into(),
            on: true,
        };
        let v = resolve(&Home::default(), &[p]);
        let w = v.iter().find(|s| s.id == "plug:wide:show").unwrap();
        assert_eq!((w.size.as_str(), w.span), ("l", 3));
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
        // v1 resolve keeps the columns even without migrate().
        assert_eq!(v.iter().find(|s| s.id == "continue").unwrap().size, "m");
        assert_eq!(v.iter().find(|s| s.id == "jump").unwrap().size, "s");
        assert_eq!(v.iter().find(|s| s.id == "mixes").unwrap().size, "l");
    }

    #[test]
    fn migrate_turns_v1_width_into_the_nearest_size() {
        let mut home = Home {
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
                    size: "s".into(),
                    span: 2,
                },
                HomeWidget {
                    id: "listen".into(),
                    on: false,
                    size: "s".into(),
                    span: 3,
                },
            ],
        };
        migrate(&mut home);
        assert_eq!(home.version, VERSION);
        let at = |id: &str| home.widgets.iter().find(|w| w.id == id).unwrap();
        assert_eq!(
            (at("continue").size.as_str(), at("continue").span),
            ("m", 2)
        );
        assert_eq!((at("jump").size.as_str(), at("jump").span), ("s", 1));
        assert_eq!((at("mixes").size.as_str(), at("mixes").span), ("l", 3));
        assert_eq!((at("added").size.as_str(), at("added").span), ("m", 2));
        assert_eq!(
            (
                at("listen").size.as_str(),
                at("listen").span,
                at("listen").on
            ),
            ("l", 3, false)
        );
        let keep = home.clone();
        migrate(&mut home);
        assert_eq!(home, keep, "a second pass is a no-op");
        let v = resolve(&home, &[]);
        assert_eq!(v.iter().find(|s| s.id == "mixes").unwrap().span, 3);
        assert!(v.iter().find(|s| s.id == "favorites").is_some());
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
            ("s", 1),
            "v1 width ⅓ becomes S; the bad size is ignored"
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
        let h2 = apply(&h, &resolve(&h, &[]), &Op::Size("jump".into(), "l".into()));
        let c = h2.widgets.iter().find(|w| w.id == "continue").unwrap();
        assert_eq!((c.size.as_str(), c.span), ("s", 1));
        let j = h2.widgets.iter().find(|w| w.id == "jump").unwrap();
        assert_eq!(
            (j.size.as_str(), j.span),
            ("m", 2),
            "jump allows S and M only"
        );
        let h3 = apply(&h2, &resolve(&h2, &[]), &Op::Step("recent".into(), -1));
        assert_eq!(h3.widgets[0].id, "recent", "the first one stays first");
    }

    #[test]
    fn shelves_declare_the_sizes_they_allow() {
        for d in BUILTIN {
            if d.id == "jump" {
                assert_eq!(d.sizes, "sm");
                continue;
            }
            assert_eq!(d.sizes, "sml", "{} should allow S, M and L", d.id);
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
