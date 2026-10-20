#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod abloop;
mod about;
mod app;
mod config;
mod desktop;
mod dialogs;
mod downloads;
mod drawer;
mod extwin;
mod fx;
mod images;
mod ipc;
mod keymap;
mod library;
mod libview;
mod loading;
mod loopsync;
mod menus;
mod mini;
#[cfg(target_os = "linux")]
mod mpris;
mod notify;
mod nowplaying;
mod overlays;
mod palette;
mod peaks;
mod player;
mod plugins;
mod preview;
mod render;
mod scenes;
mod settings;
mod store;
mod suboffset;
mod tray;
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
    // NVIDIA-only machines: load just the NVIDIA EGL vendor (no Mesa). Must run before
    // any thread starts, since it sets an environment variable.
    #[cfg(target_os = "linux")]
    app::egl::select_vendor();
    // --version / --help answer before any window, mpv or thread starts.
    let (cli, links) = match app::cli::parse(std::env::args().skip(1)) {
        app::cli::Cli::Version => {
            println!("{}", app::cli::version());
            return Ok(());
        }
        app::cli::Cli::Help => {
            print!("{}", app::cli::USAGE);
            return Ok(());
        }
        app::cli::Cli::Run {
            files,
            links,
            unknown,
        } => {
            for u in unknown {
                eprintln!("vivi: unknown option {u} (ignored; see vivi --help)");
            }
            (files, links)
        }
    };
    // A Vivi with a tray icon is running: it shows its window and opens the files.
    if links.is_empty() && tray::forward(&cli) {
        return Ok(());
    }
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
    if let Some(id) = links.iter().find_map(|a| plugins::packages::deep_link(a)) {
        open_plugin_link(&app, &id);
    }
    let p = player.clone();
    let interop = {
        let c = cfg.borrow();
        (c.playback.hwdec_interop.clone(), c.playback.hwdec.clone())
    };
    render::attach(&app, player.mpv.clone(), interop, move || {
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

    // The loop runs until Quit (not until the last window closes), so closing can hide
    // the window to the tray while playback continues (src/tray.rs).
    app.show()?;
    slint::run_event_loop_until_quit()?;
    let _ = app.hide();
    player.save_position();
    // Scripts get their shutdown event and their download tools are stopped.
    player.shutdown(std::time::Duration::from_secs(3));
    ipc::cleanup();
    // mpv waits for every script to finish; one stuck in a loop must not keep Vivi running.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(5));
        std::process::exit(0);
    });
    Ok(())
}

pub(crate) fn playlist_items() -> slint::ModelRc<Item> {
    items(
        store::playlists()
            .into_iter()
            .map(|(n, p)| (p.to_string_lossy().into_owned(), n, false)),
    )
}
