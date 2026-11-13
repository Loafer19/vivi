//! Single owner of the UI-thread application state (#101).
//!
//! Before this, the config was handed around as `crate::app::state::AppState`. A handler that held
//! `borrow_mut()` while calling into other code (plugin buttons, the toolbar, a nested Slint
//! callback) aborted Vivi with "RefCell already mutably borrowed" (0.16.3, Customize Player
//! and Clear recent). [`AppState`] removes that class of bug by construction:
//!
//! * **Reads are snapshots.** [`AppState::config`] returns an `Rc<Config>`; the internal
//!   cell is borrowed only for the pointer copy, never while caller code runs. A snapshot
//!   stays valid (and unchanged) however the state is updated meanwhile.
//! * **Writes are messages.** [`AppState::update`] (and [`AppState::send`] with an
//!   [`AppMsg`]) changes a private copy and swaps it in. An update requested while another
//!   one is running (re-entry from a callback) is queued and applied right after the outer
//!   one commits, in order, instead of panicking or being lost.
//! * **Pure transitions.** [`reduce`] is a plain `fn(&mut Config, AppMsg)`, so the UI logic
//!   behind the messages is unit-tested without a window (see the tests below).
//!
//! Session state that lived in `thread_local!` slots moves here module by module; the slots
//! that remain are process-wide caches (decoded images, timers, the ticker) or one weak
//! window handle per feature, documented where they are declared.

use crate::config::{self, Config};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

type Edit = Box<dyn FnOnce(&mut Config)>;

struct Inner {
    cfg: RefCell<Rc<Config>>,
    /// True while an update closure runs; updates arriving then are queued.
    busy: Cell<bool>,
    queue: RefCell<VecDeque<Edit>>,
    /// Bumped on every committed change (cheap "did anything change" checks).
    version: Cell<u64>,
    /// A queued change asked to be saved: write the file once the queue is drained.
    save_due: Cell<bool>,
    /// False for test states: [`AppState::save`] then writes nothing.
    persist: bool,
}

/// Cheap to clone handle to the one application state (UI thread only).
#[derive(Clone)]
pub struct AppState(Rc<Inner>);

/// Config changes the UI sends; applied by [`reduce`].
#[derive(Debug, Clone, PartialEq)]
pub enum AppMsg {
    /// Replace the toolbar (already resolved against the plugin buttons) and recompute
    /// the interface profile.
    SetToolbar(Vec<String>),
    /// Pick a built-in interface profile ("Minimal", "Classic", …); unknown names are ignored.
    SelectProfile(String),
    SetBarStyle(String),
    SetVolumeStyle(String),
    /// Put back an interface snapshot (Customize → Cancel).
    RestoreInterface(Box<config::Interface>),
    /// Plugin enable flag (`None` = the plugin's default).
    SetPluginEnabled(String, Option<bool>),
    /// One plugin setting value.
    SetPluginSetting(String, String, String),
    /// A placeholder (`stub.…`) setting value.
    SetStub(String, String),
    /// Drop a plugin's on / off choice and settings.
    ForgetPlugin(String),
}

/// Apply one message to a config. Pure: no I/O, no UI.
pub fn reduce(c: &mut Config, msg: AppMsg) {
    match msg {
        AppMsg::SetToolbar(t) => {
            c.interface.toolbar = t;
            c.interface.profile = crate::settings::profile_of(&c.interface);
        }
        AppMsg::SelectProfile(p) => {
            if let Some(t) = config::profile_toolbar(&p) {
                c.interface.toolbar = t;
                c.interface.bar_style = config::profile_style(&p).into();
                c.interface.volume_style = config::profile_volume(&p).into();
                c.interface.profile = p;
            }
        }
        AppMsg::SetBarStyle(s) => {
            c.interface.bar_style = s;
            c.interface.profile = crate::settings::profile_of(&c.interface);
        }
        AppMsg::SetVolumeStyle(s) => {
            c.interface.volume_style = s;
            c.interface.profile = crate::settings::profile_of(&c.interface);
        }
        AppMsg::RestoreInterface(i) => c.interface = *i,
        AppMsg::SetPluginEnabled(id, on) => c.plugins.entry(id).or_default().enabled = on,
        AppMsg::SetPluginSetting(id, k, v) => {
            c.plugins.entry(id).or_default().settings.insert(k, v);
        }
        AppMsg::SetStub(k, v) => {
            c.stubs.insert(k, v);
        }
        AppMsg::ForgetPlugin(id) => {
            c.plugins.remove(&id);
        }
    }
}

impl AppState {
    pub fn new(cfg: Config) -> Self {
        Self(Rc::new(Inner {
            cfg: RefCell::new(Rc::new(cfg)),
            busy: Cell::new(false),
            queue: RefCell::new(VecDeque::new()),
            version: Cell::new(0),
            save_due: Cell::new(false),
            persist: true,
        }))
    }

    /// A state that never writes the config file (unit tests).
    #[cfg(test)]
    pub fn in_memory(cfg: Config) -> Self {
        Self(Rc::new(Inner {
            cfg: RefCell::new(Rc::new(cfg)),
            busy: Cell::new(false),
            queue: RefCell::new(VecDeque::new()),
            version: Cell::new(0),
            save_due: Cell::new(false),
            persist: false,
        }))
    }

    /// The current config. A snapshot: later updates do not change it.
    pub fn config(&self) -> Rc<Config> {
        self.0.cfg.borrow().clone()
    }

    /// Change the config. Runs now, or, when called from inside another update, right
    /// after that one (never re-entrant, never a panic).
    pub fn update(&self, f: impl FnOnce(&mut Config) + 'static) {
        self.0.queue.borrow_mut().push_back(Box::new(f));
        if self.0.busy.get() {
            return;
        }
        self.0.busy.set(true);
        loop {
            // The queue borrow ends before the edit runs, so an edit may enqueue more.
            let next = self.0.queue.borrow_mut().pop_front();
            let Some(edit) = next else { break };
            let mut c = Config::clone(&self.config());
            edit(&mut c);
            *self.0.cfg.borrow_mut() = Rc::new(c);
            self.0.version.set(self.0.version.get() + 1);
        }
        self.0.busy.set(false);
        if self.0.save_due.replace(false) {
            self.save_or_log();
        }
    }

    /// [`update`](Self::update), then save the config file once the change (and anything
    /// queued with it) is committed.
    pub fn update_and_save(&self, f: impl FnOnce(&mut Config) + 'static) {
        self.0.save_due.set(true);
        self.update(f);
    }

    /// [`send`](Self::send), then save (see [`update_and_save`](Self::update_and_save)).
    pub fn send_and_save(&self, msg: AppMsg) {
        self.update_and_save(move |c| reduce(c, msg));
    }

    /// Change the config now and return a value; `None` (with a log line) when called
    /// from inside another update, where the result could not be known yet.
    pub fn update_with<R>(&self, f: impl FnOnce(&mut Config) -> R) -> Option<R> {
        if self.0.busy.get() {
            eprintln!("vivi: state: nested config change skipped (re-entrant call)");
            return None;
        }
        self.0.busy.set(true);
        let mut c = Config::clone(&self.config());
        let r = f(&mut c);
        *self.0.cfg.borrow_mut() = Rc::new(c);
        self.0.version.set(self.0.version.get() + 1);
        self.0.busy.set(false);
        // Anything queued meanwhile (none in practice) runs now.
        if !self.0.queue.borrow().is_empty() {
            self.update(|_| {});
        }
        Some(r)
    }

    /// Replace the whole config (a copy edited outside the state, e.g. a settings form).
    pub fn replace(&self, c: Config) {
        self.update(move |x| *x = c);
    }

    /// Send a message (see [`reduce`]).
    pub fn send(&self, msg: AppMsg) {
        self.update(move |c| reduce(c, msg));
    }

    /// Write the current config to disk.
    pub fn save(&self) -> std::io::Result<()> {
        if !self.0.persist {
            return Ok(());
        }
        self.config().save()
    }

    /// Save, logging a failure instead of returning it.
    pub fn save_or_log(&self) {
        if let Err(e) = self.save() {
            eprintln!("vivi: save config: {e}");
        }
    }

    /// Number of committed changes so far.
    pub fn version(&self) -> u64 {
        self.0.version.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_survives_update() {
        let s = AppState::in_memory(Config::default());
        let before = s.config();
        s.update(|c| c.interface.bar_style = "pill".into());
        assert_ne!(before.interface.bar_style, "pill");
        assert_eq!(s.config().interface.bar_style, "pill");
        assert_eq!(s.version(), 1);
    }

    #[test]
    fn read_while_updating_does_not_panic() {
        // The 0.16.3 abort: code inside a change reads the config again.
        let s = AppState::in_memory(Config::default());
        let s2 = s.clone();
        s.update(move |c| {
            let seen = s2.config();
            c.interface.profile = format!("{}+", seen.interface.profile);
        });
        assert!(s.config().interface.profile.ends_with('+'));
    }

    #[test]
    fn nested_update_runs_after_outer_in_order() {
        let s = AppState::in_memory(Config::default());
        let s2 = s.clone();
        s.update(move |c| {
            c.stubs.insert("order".into(), "outer".into());
            s2.update(|c| {
                let v = c.stubs.get("order").cloned().unwrap_or_default();
                c.stubs.insert("order".into(), format!("{v},inner"));
            });
        });
        assert_eq!(s.config().stubs["order"], "outer,inner");
        assert_eq!(s.version(), 2);
    }

    #[test]
    fn nested_update_with_is_skipped_not_panicking() {
        let s = AppState::in_memory(Config::default());
        let s2 = s.clone();
        s.update(move |_| assert!(s2.update_with(|_| 1).is_none()));
        assert_eq!(s.update_with(|_| 2), Some(2));
    }

    #[test]
    fn nested_save_waits_for_the_queued_change() {
        let s = AppState::in_memory(Config::default());
        let s2 = s.clone();
        s.update(move |c| {
            c.stubs.insert("a".into(), "1".into());
            s2.send_and_save(AppMsg::SetStub("b".into(), "2".into()));
            // Queued: not visible yet, and nothing was written mid-change.
            assert!(!s2.config().stubs.contains_key("b"));
        });
        let c = s.config();
        assert_eq!((c.stubs["a"].as_str(), c.stubs["b"].as_str()), ("1", "2"));
        assert!(!s.0.save_due.get());
    }

    #[test]
    fn reduce_customize_messages() {
        let mut c = Config::default();
        reduce(&mut c, AppMsg::SetBarStyle("pill".into()));
        assert_eq!(c.interface.bar_style, "pill");
        let saved = c.interface.clone();
        reduce(&mut c, AppMsg::SetToolbar(vec!["play".into()]));
        assert_eq!(c.interface.toolbar, vec!["play".to_string()]);
        reduce(&mut c, AppMsg::RestoreInterface(Box::new(saved.clone())));
        assert_eq!(c.interface, saved);
        reduce(&mut c, AppMsg::SelectProfile("no such profile".into()));
        assert_eq!(c.interface, saved);
    }

    #[test]
    fn reduce_plugin_messages() {
        let mut c = Config::default();
        reduce(&mut c, AppMsg::SetPluginEnabled("lyrics".into(), Some(false)));
        reduce(
            &mut c,
            AppMsg::SetPluginSetting("lyrics".into(), "k".into(), "v".into()),
        );
        assert_eq!(c.plugins["lyrics"].enabled, Some(false));
        assert_eq!(c.plugins["lyrics"].settings["k"], "v");
    }
}
