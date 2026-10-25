//! Live content of package Home widgets and side-drawer tabs (`vivi-content`, issue #83).
//!
//! A package that contributes `home.widget` or `drawer.panel` can fill its Home card and
//! drawer tab with live data instead of the static manifest text:
//!
//! * `vivi-content <id> <surface> <json>`: `surface` is `home`, `drawer` or `all`; the
//!   JSON is a [`ContentUpdate`] (every field optional, only the given ones change, so a
//!   package can send the whole thing once and then update the value line alone).
//! * `vivi-content-clear <id> [surface]`: back to the static manifest text.
//!
//! Rows carry an optional `bar` (0–1) drawn as a native bar (no block characters), a
//! `heading` row starts a section. The host keeps the latest content per package and
//! surface for the session; Home shows the first [`HOME_ROWS`] rows, the drawer up to
//! [`MAX_ROWS`]. See docs/plugins-architecture.md ("Host API for packages").

use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;

/// Rows a Home card shows (the rest are left out).
pub const HOME_ROWS: usize = 8;
/// Rows kept per surface.
pub const MAX_ROWS: usize = 60;

/// Where the content goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    Home,
    Drawer,
}

/// `home` / `drawer` / `all` (also `both`, `*`) → the surfaces it names.
pub fn surfaces(s: &str) -> Option<&'static [Surface]> {
    match s.trim().to_ascii_lowercase().as_str() {
        "home" | "home.widget" | "widget" => Some(&[Surface::Home]),
        "drawer" | "drawer.panel" | "panel" => Some(&[Surface::Drawer]),
        "all" | "both" | "*" | "" => Some(&[Surface::Home, Surface::Drawer]),
        _ => None,
    }
}

/// One row: `label` on the left, `value` on the right, an optional native bar between
/// them (`bar`, 0–1) and a small `sub` line below. `heading`: a section title (only
/// `label` is used).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct LiveRow {
    pub label: String,
    pub value: String,
    pub sub: String,
    pub bar: Option<f64>,
    pub heading: bool,
}

/// `vivi-content` payload: every field optional; only the given ones change.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ContentUpdate {
    /// Replaces the card / tab title (default: the contribution's label).
    pub title: Option<String>,
    /// The large value line ("2h 14m today").
    pub value: Option<String>,
    /// Small text under the value (replaces the plugin description).
    pub sub: Option<String>,
    /// A progress bar under the value (0–1); a negative number removes it.
    pub progress: Option<f64>,
    pub rows: Option<Vec<LiveRow>>,
    /// Start from empty content instead of merging into the previous one.
    pub replace: bool,
}

/// The content shown on one surface.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Content {
    pub title: String,
    pub value: String,
    pub sub: String,
    /// 0–1, or `None` for no progress bar.
    pub progress: Option<f32>,
    pub rows: Vec<LiveRow>,
}

/// A bar / progress value clamped to 0–1; negative, NaN or missing → none.
pub fn fraction(v: Option<f64>) -> Option<f32> {
    v.filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v.min(1.0) as f32)
}

impl Content {
    pub fn apply(&mut self, u: ContentUpdate) {
        if u.replace {
            *self = Content::default();
        }
        if let Some(v) = u.title {
            self.title = v;
        }
        if let Some(v) = u.value {
            self.value = v;
        }
        if let Some(v) = u.sub {
            self.sub = v;
        }
        if u.progress.is_some() {
            self.progress = fraction(u.progress);
        }
        if let Some(mut rows) = u.rows {
            rows.truncate(MAX_ROWS);
            for r in &mut rows {
                r.bar = fraction(r.bar).map(f64::from);
            }
            self.rows = rows;
        }
    }

    /// The Slint rows (first `max`); a row without a bar has `bar` -1.
    pub fn ui_rows(&self, max: usize) -> Vec<crate::LiveRow> {
        self.rows
            .iter()
            .take(max)
            .map(|r| crate::LiveRow {
                label: r.label.as_str().into(),
                value: r.value.as_str().into(),
                sub: r.sub.as_str().into(),
                bar: r.bar.map_or(-1.0, |b| b as f32),
                heading: r.heading,
            })
            .collect()
    }
}

thread_local! {
    static STORE: RefCell<HashMap<(String, Surface), Content>> = RefCell::new(HashMap::new());
}

/// Apply an update; true when the stored content changed.
pub fn update(id: &str, s: Surface, u: ContentUpdate) -> bool {
    STORE.with(|st| {
        let mut st = st.borrow_mut();
        let c = st.entry((id.to_string(), s)).or_default();
        let before = c.clone();
        c.apply(u);
        *c != before
    })
}

/// Drop a package's live content on `s`; true when it had some.
pub fn clear(id: &str, s: Surface) -> bool {
    STORE.with(|st| st.borrow_mut().remove(&(id.to_string(), s)).is_some())
}

/// The live content of package `id` on `s`, if it sent any.
pub fn get(id: &str, s: Surface) -> Option<Content> {
    STORE.with(|st| st.borrow().get(&(id.to_string(), s)).cloned())
}

/// `vivi-content <id> <surface> <json>`: store it and redraw the surfaces it changed.
pub fn message(id: &str, surface: &str, json: &str) {
    let Some(list) = surfaces(surface) else {
        eprintln!("vivi: {id}: vivi-content for unknown surface {surface:?} ignored");
        return;
    };
    let u: ContentUpdate = match serde_json::from_str(json) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("vivi: {id}: bad vivi-content json: {e}");
            return;
        }
    };
    let mut changed = Vec::new();
    for s in list {
        if update(id, *s, u.clone()) {
            changed.push(*s);
        }
    }
    redraw(id, &changed);
}

/// `vivi-content-clear <id> [surface]`.
pub fn clear_message(id: &str, surface: &str) {
    let Some(list) = surfaces(surface) else {
        return;
    };
    let changed: Vec<Surface> = list.iter().copied().filter(|s| clear(id, *s)).collect();
    redraw(id, &changed);
}

fn redraw(id: &str, changed: &[Surface]) {
    if changed.contains(&Surface::Home) {
        crate::extwin::refresh_widgets();
    }
    if changed.contains(&Surface::Drawer) {
        if let Some(a) = super::with(|r| r.app.upgrade()).flatten() {
            crate::drawer::refresh_content(&a, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(json: &str) -> ContentUpdate {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn parses_and_merges_updates() {
        let mut c = Content::default();
        c.apply(upd(
            r#"{"value":"1h 05m","sub":"today","progress":0.5,
                "rows":[{"label":"Mon","value":"20m","bar":0.25},{"label":"Top","heading":true},
                        {"label":"x","bar":3},{"label":"y","bar":-1},{"label":"z","unknown":1}]}"#,
        ));
        assert_eq!((c.value.as_str(), c.sub.as_str()), ("1h 05m", "today"));
        assert_eq!(c.progress, Some(0.5));
        assert_eq!(c.rows.len(), 5);
        assert_eq!(c.rows[0].bar, Some(0.25));
        assert!(c.rows[1].heading);
        assert_eq!(c.rows[2].bar, Some(1.0)); // clamped
        assert_eq!(c.rows[3].bar, None); // negative: no bar
        assert_eq!(c.rows[4].bar, None);
        // A later update changes only what it names.
        c.apply(upd(r#"{"value":"1h 06m"}"#));
        assert_eq!(
            (c.value.as_str(), c.sub.as_str(), c.rows.len()),
            ("1h 06m", "today", 5)
        );
        c.apply(upd(r#"{"progress":-1}"#));
        assert_eq!(c.progress, None);
        c.apply(upd(r#"{"replace":true,"title":"T"}"#));
        assert_eq!(
            c,
            Content {
                title: "T".into(),
                ..Default::default()
            }
        );
        let ui = {
            let mut c = Content::default();
            c.apply(upd(
                r#"{"rows":[{"label":"a"},{"label":"b","bar":0.5},{"label":"c"}]}"#,
            ));
            c.ui_rows(2)
        };
        assert_eq!(ui.len(), 2);
        assert_eq!((ui[0].bar, ui[1].bar), (-1.0, 0.5));
    }

    #[test]
    fn rows_are_bounded_and_bad_numbers_dropped() {
        let rows: Vec<String> = (0..100).map(|i| format!(r#"{{"label":"{i}"}}"#)).collect();
        let mut c = Content::default();
        c.apply(upd(&format!(r#"{{"rows":[{}]}}"#, rows.join(","))));
        assert_eq!(c.rows.len(), MAX_ROWS);
        assert_eq!(fraction(Some(f64::NAN)), None);
        assert_eq!(fraction(Some(f64::INFINITY)), None);
        assert_eq!(fraction(Some(0.0)), Some(0.0));
        assert_eq!(fraction(None), None);
    }

    #[test]
    fn store_per_package_and_surface() {
        let id = "content-test-pkg";
        assert!(update(id, Surface::Home, upd(r#"{"value":"a"}"#)));
        assert!(!update(id, Surface::Home, upd(r#"{"value":"a"}"#))); // unchanged
        assert_eq!(get(id, Surface::Home).unwrap().value, "a");
        assert_eq!(get(id, Surface::Drawer), None);
        assert!(clear(id, Surface::Home));
        assert!(!clear(id, Surface::Home));
        assert_eq!(get(id, Surface::Home), None);
    }

    #[test]
    fn surface_names() {
        assert_eq!(surfaces("home"), Some(&[Surface::Home][..]));
        assert_eq!(surfaces("Drawer"), Some(&[Surface::Drawer][..]));
        assert_eq!(surfaces("all").map(|s| s.len()), Some(2));
        assert_eq!(surfaces(""), surfaces("all"));
        assert_eq!(surfaces("sidebar"), None);
    }
}
