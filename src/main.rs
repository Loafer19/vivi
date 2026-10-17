#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod about;
mod app;
mod config;
mod desktop;
mod dialogs;
mod fx;
mod ipc;
mod keymap;
mod library;
mod libview;
mod loading;
mod menus;
mod mini;
#[cfg(target_os = "linux")]
mod mpris;
mod overlays;
mod peaks;
mod player;
mod plugins;
mod preview;
mod render;
mod scenes;
mod settings;
mod store;
mod util;
mod viz;

use app::debug::{debug_ui, open_plugin_link, panel_tab, screenshot_mode};
use app::keys::{forward_key, key_debug, LAST_PHYSICAL};
use app::panel::*;
use app::session::*;
use player::{Player, PlayerEvent};
use slint::{ComponentHandle, Model};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

slint::include_modules!();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::startup::init()?;

    let app = AppWindow::new()?;
    let log = std::env::var_os("VIVI_LOG").is_some();

    let weak = app.as_weak();
    let (player, events) = Player::new(move || {
        let w = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(a) = w.upgrade() {
                a.window().request_redraw();
            }
        });
    })?;
    let player = Rc::new(player);
    let cfg = Rc::new(std::cell::RefCell::new(config::Config::load()));
    app::startup::setup_features(&app, &player, &cfg, log);

    // Drain player events on the UI thread.
    app::wiring::events::wire(&app, &player, &cfg, events, log);

    // First CLI file opens once rendering is ready; further files are queued.
    // vivi://plugin/<id> (marketplace "Open in Vivi") opens the plugin in Extensions → Browse.
    let (links, cli): (Vec<String>, Vec<String>) = std::env::args()
        .skip(1)
        .partition(|a| a.starts_with("vivi://"));
    if let Some(id) = links.iter().find_map(|a| plugins::packages::deep_link(a)) {
        open_plugin_link(&app, &id);
    }
    let p = player.clone();
    render::attach(&app, player.mpv.clone(), move || {
        let mut it = cli.iter();
        if let Some(first) = it.next() {
            p.open(first);
        }
        for f in it {
            p.enqueue(f);
        }
    })?;

    let (toggle_fs, open_file) = app::wiring::transport::wire(&app, &player);

    app::wiring::keys::wire(&app, &player, toggle_fs, open_file);

    // Auto-hide controls/cursor after inactivity while playing.
    let _hide_timer = app::wiring::autohide::wire(&app, &player);

    app::wiring::window::wire(&app, &player);

    side_panel(&app, &player, log);
    queue_and_resume(&app, &player, log);

    app.run()?;
    player.save_position();
    Ok(())
}

pub(crate) fn playlist_items() -> slint::ModelRc<Item> {
    items(
        store::playlists()
            .into_iter()
            .map(|(n, p)| (p.to_string_lossy().into_owned(), n, false)),
    )
}
