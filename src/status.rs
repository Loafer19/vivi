//! Top-bar status indicators: a small accent chip for each feature that is on, so it is
//! clear at a glance what changes playback. Built in: the sleep timer (moon + time
//! left), loudness normalization, an A-B loop, speed other than 1×, and remote control
//! (the agent socket or the Remote Control extension). Packages add their own with
//! `script-message vivi-indicator <id> <key> <json>` (host feature `indicators`);
//! clicking one sends them `{"event":"indicator","args":[key]}` on `vivi-ui`.
//! Built-in chips open the matching side-panel tab or settings page.

use crate::player::Player;
use crate::{AppWindow, Indicator, PanelTab};
use serde::Deserialize;
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

/// A package indicator (`vivi-indicator` JSON).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PluginIndicator {
    /// Icon name from Vivi's set (falls back to "extension").
    pub icon: String,
    pub tip: String,
    /// Short text after the icon (trimmed to 12 characters).
    pub label: String,
}

/// Most chips a package can show at once.
const PER_PLUGIN: usize = 3;

/// "1.5×", "0.75×", "2×".
pub fn speed_label(s: f64) -> String {
    let t = format!("{s:.2}");
    let t = t.trim_end_matches('0').trim_end_matches('.');
    format!("{t}×")
}

/// Parse a package indicator; None for bad JSON or one without an icon or tooltip.
pub fn parse_plugin(json: &str) -> Option<PluginIndicator> {
    let mut i: PluginIndicator = serde_json::from_str(json).ok()?;
    i.icon = i.icon.trim().to_string();
    i.tip = i.tip.trim().to_string();
    i.label = i.label.trim().chars().take(12).collect();
    if i.icon.is_empty() && i.tip.is_empty() {
        return None;
    }
    if i.icon.is_empty()
        || !i
            .icon
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        i.icon = "extension".into();
    }
    Some(i)
}

/// What a click on chip `id` does.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    Panel(PanelTab),
    Settings(&'static str),
    Plugin { id: String, key: String },
    None,
}

pub fn click_target(id: &str) -> Click {
    match id {
        "sleep" | "ab-loop" | "speed" => Click::Panel(PanelTab::Playback),
        "leveling" => Click::Settings("Audio"),
        // The agent socket and the phone remote are both on Controls → Remotes.
        "remote" => Click::Settings("Remotes"),
        _ => match id.strip_prefix("plugin:").and_then(|r| r.split_once(':')) {
            Some((p, k)) => Click::Plugin {
                id: p.to_string(),
                key: k.to_string(),
            },
            None => Click::None,
        },
    }
}

fn chip(id: &str, icon: &str, tip: &str, label: &str) -> Indicator {
    Indicator {
        id: id.into(),
        icon: icon.into(),
        tip: tip.into(),
        label: label.into(),
    }
}

thread_local! {
    static CTX: RefCell<Option<(slint::Weak<AppWindow>, Rc<Player>)>> = const { RefCell::new(None) };
    /// (package id, key) → indicator.
    static PLUGINS: RefCell<BTreeMap<(String, String), PluginIndicator>> = const { RefCell::new(BTreeMap::new()) };
    static TICK: slint::Timer = slint::Timer::default();
}

fn num(p: &Player, name: &str) -> Option<f64> {
    p.get(name)
        .filter(|v| v != "no")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

fn remote_extension() -> bool {
    crate::plugins::loaded(crate::settings::REMOTE_CONTROL)
}

/// Tooltip of the top-bar remote chip, or none when nothing can control Vivi (#94):
/// `agents` = "Allow AI agents and scripts" is on and this instance owns the control
/// socket; `phone` = the Remote Control extension is installed, enabled and loaded.
fn remote_tip(agents: bool, phone: bool) -> Option<&'static str> {
    Some(match (agents, phone) {
        (true, true) => "Remote control: agent socket and phone remote are on",
        (true, false) => "Remote control: the agent socket (vivi-mcp) is on",
        (false, true) => "Remote control: the phone remote is on",
        (false, false) => return None,
    })
}

/// Tooltip of the volume leveling chip: ReplayGain (#88) and / or Normalize loudness.
fn leveling_tip(replaygain: Option<&str>, norm: bool) -> Option<String> {
    let rg = replaygain.filter(|m| matches!(*m, "track" | "album"));
    let what = match (rg, norm) {
        (Some(m), true) => format!("ReplayGain ({m}) and loudness normalization are on"),
        (Some(m), false) => format!("ReplayGain ({m}) is on"),
        (None, true) => "Loudness normalization is on".into(),
        (None, false) => return None,
    };
    Some(format!("{what} — open Audio settings"))
}

/// The chips to show now.
fn current(p: &Player) -> Vec<Indicator> {
    let mut v = Vec::new();
    if let Some((label, tip)) = crate::sleep::indicator() {
        v.push(chip("sleep", "moon", &tip, &label));
    }
    if let Some(tip) = leveling_tip(
        p.get("replaygain").as_deref(),
        p.get("af").is_some_and(|f| f.contains("vivi-norm")),
    ) {
        v.push(chip("leveling", "loudness", &tip, ""));
    }
    let (a, b) = (num(p, "ab-loop-a"), num(p, "ab-loop-b"));
    if a.is_some() || b.is_some() {
        v.push(chip(
            "ab-loop",
            "ab-loop",
            &format!("A-B loop: {}", crate::abloop::state_text(a, b)),
            "",
        ));
    }
    if let Some(s) = num(p, "speed").filter(|s| (s - 1.0).abs() > 0.001) {
        let l = speed_label(s);
        v.push(chip("speed", "speed", &format!("Playing at {l}"), &l));
    }
    if let Some(tip) = remote_tip(crate::ipc::listening(), remote_extension()) {
        v.push(chip("remote", "remote", tip, ""));
    }
    PLUGINS.with(|m| {
        for ((id, key), i) in m.borrow().iter() {
            if crate::plugins::loaded(id) {
                v.push(chip(
                    &format!("plugin:{id}:{key}"),
                    &i.icon,
                    &i.tip,
                    &i.label,
                ));
            }
        }
    });
    v
}

/// Recompute the chips; the model is only replaced when something changed.
pub fn refresh() {
    let Some((a, p)) = CTX.with(|c| {
        let c = c.borrow();
        let (w, p) = c.as_ref()?;
        Some((w.upgrade()?, p.clone()))
    }) else {
        return;
    };
    // Speed changed outside the panel (keys, menus, IPC, plugins): keep the Playback
    // panel's presets / readout in step with mpv.
    if let Some(s) = num(&p, "speed") {
        let pg = a.global::<crate::Panel>();
        if (pg.get_pv_speed() as f64 - s).abs() > 0.001 {
            pg.set_pv_speed(s as f32);
        }
    }
    let now = current(&p);
    let g = a.global::<crate::Status>();
    let old = g.get_items();
    if old.row_count() == now.len() && old.iter().zip(now.iter()).all(|(x, y)| x == *y) {
        return;
    }
    g.set_items(slint::ModelRc::new(slint::VecModel::from(now)));
}

/// `vivi-indicator <id> <key> <json>`.
pub fn plugin_set(id: &str, key: &str, json: &str) {
    let Some(i) = parse_plugin(json) else {
        eprintln!("vivi: {id}: bad vivi-indicator json");
        return;
    };
    PLUGINS.with(|m| {
        let mut m = m.borrow_mut();
        let k = (id.to_string(), key.to_string());
        let n = m.keys().filter(|(p, _)| p == id).count();
        if !m.contains_key(&k) && n >= PER_PLUGIN {
            eprintln!("vivi: {id}: more than {PER_PLUGIN} indicators; {key} ignored");
            return;
        }
        m.insert(k, i);
    });
    refresh();
}

/// `vivi-indicator-clear <id> [key]` ("" clears all of the package's).
pub fn plugin_clear(id: &str, key: &str) {
    PLUGINS.with(|m| {
        m.borrow_mut()
            .retain(|(p, k), _| !(p == id && (key.is_empty() || k == key)))
    });
    refresh();
}

fn clicked(a: &AppWindow, id: &str) {
    match click_target(id) {
        Click::Panel(tab) => {
            a.global::<crate::Overlays>().set_settings_open(false);
            a.global::<crate::Panel>().set_tab(tab);
            a.global::<crate::Overlays>().set_panel_open(true);
        }
        Click::Settings(cat) => {
            let n = crate::settings::category(cat);
            a.global::<crate::Settings>().set_category(n);
            a.global::<crate::Overlays>().set_settings_open(true);
            a.global::<crate::Settings>()
                .invoke_refresh(n, "".into(), false);
        }
        Click::Plugin { id, key } => {
            let ev = serde_json::json!({"event": "indicator", "args": [key]}).to_string();
            crate::plugins::run(&id, "vivi-ui", &[ev]);
        }
        Click::None => {}
    }
}

pub fn setup(app: &AppWindow, player: &Rc<Player>) {
    CTX.with(|c| *c.borrow_mut() = Some((app.as_weak(), player.clone())));
    let w = app.as_weak();
    app.global::<crate::Status>().on_clicked(move |id| {
        if let Some(a) = w.upgrade() {
            clicked(&a, &id);
        }
    });
    TICK.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_secs(1), refresh));
    refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_chip_only_when_something_listens() {
        // #94: both off (socket not allowed, extension missing / disabled) → no chip.
        assert_eq!(remote_tip(false, false), None);
        assert!(remote_tip(true, false).unwrap().contains("agent socket"));
        assert!(remote_tip(false, true).unwrap().contains("phone remote"));
        assert!(remote_tip(true, true).unwrap().contains("and phone remote"));
    }

    #[test]
    fn leveling_chip() {
        assert_eq!(leveling_tip(Some("no"), false), None);
        assert_eq!(leveling_tip(None, false), None);
        assert_eq!(
            leveling_tip(Some("album"), false).unwrap(),
            "ReplayGain (album) is on — open Audio settings"
        );
        assert_eq!(
            leveling_tip(Some("no"), true).unwrap(),
            "Loudness normalization is on — open Audio settings"
        );
        assert!(leveling_tip(Some("track"), true)
            .unwrap()
            .starts_with("ReplayGain (track) and"));
    }

    #[test]
    fn speed_labels() {
        assert_eq!(speed_label(1.5), "1.5×");
        assert_eq!(speed_label(0.75), "0.75×");
        assert_eq!(speed_label(2.0), "2×");
        assert_eq!(speed_label(1.25), "1.25×");
    }

    #[test]
    fn plugin_json() {
        let i =
            parse_plugin(r#"{"icon":"cast","tip":"Casting to TV","label":"  Living room TV  "}"#)
                .unwrap();
        assert_eq!(i.icon, "cast");
        assert_eq!(i.label, "Living room ");
        assert_eq!(parse_plugin(r#"{"tip":"x"}"#).unwrap().icon, "extension");
        assert_eq!(
            parse_plugin(r#"{"icon":"../../etc","tip":"x"}"#)
                .unwrap()
                .icon,
            "extension"
        );
        assert!(parse_plugin("{}").is_none());
        assert!(parse_plugin("nope").is_none());
    }

    #[test]
    fn clicks() {
        assert_eq!(click_target("sleep"), Click::Panel(PanelTab::Playback));
        assert_eq!(click_target("speed"), Click::Panel(PanelTab::Playback));
        assert_eq!(click_target("leveling"), Click::Settings("Audio"));
        assert_eq!(click_target("remote"), Click::Settings("Remotes"));
        assert_eq!(
            click_target("plugin:cast:tv"),
            Click::Plugin {
                id: "cast".into(),
                key: "tv".into()
            }
        );
        assert_eq!(click_target("plugin:bad"), Click::None);
        assert_eq!(click_target("other"), Click::None);
    }
}
