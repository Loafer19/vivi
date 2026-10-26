//! Menus per the Figma frames: the right-click main menu with its submenus, library
//! context menus (file / folder / playlist), and the library sort and view options.
//! Slint draws them (ui/menu.slint); this module fills the rows and runs the actions.

use crate::config::Config;
use crate::keymap::{self, Action as A};
use crate::player::Player;
use crate::plugins::{self, contrib};
use crate::{store, AppWindow, MenuItem};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn row(id: &str, label: &str, icon: &str, hint: &str) -> MenuItem {
    MenuItem {
        id: id.into(),
        label: label.into(),
        icon: icon.into(),
        hint: hint.into(),
        ..Default::default()
    }
}
fn sub(id: &str, label: &str, icon: &str) -> MenuItem {
    MenuItem {
        sub: true,
        ..row(id, label, icon, "")
    }
}
fn check(id: &str, label: &str, on: bool) -> MenuItem {
    MenuItem {
        checked: on,
        ..row(id, label, "", "")
    }
}
fn div(mut m: MenuItem) -> MenuItem {
    m.divider = true;
    m
}

/// Sort options in the Figma order. Disc numbers are not in the library, so those rows are
/// left out; "Largest First" keeps Vivi's size sort.
pub const SORTS: &[(&str, &str)] = &[
    ("unwatched", "Unwatched First"),
    ("watched", "Watched First"),
    ("newest", "Newest First"),
    ("oldest", "Oldest First"),
    ("title", "Title A-Z"),
    ("title-desc", "Title Z-A"),
    ("artist", "Artist A-Z"),
    ("artist-desc", "Artist Z-A"),
    ("album", "Album A-Z"),
    ("album-desc", "Album Z-A"),
    ("track", "Track Number Ascending"),
    ("track-desc", "Track Number Descending"),
    ("uri", "URI Ascending"),
    ("uri-desc", "URI Descending"),
    ("size", "Largest First"),
];

/// Legacy config values map onto the new keys.
pub fn sort_key(s: &str) -> &str {
    match s {
        "name" => "title",
        "date" => "newest",
        other => other,
    }
}

/// Rows plugins contribute at `point` (see `plugins::contrib`): id `plug:<plugin>|<command>`.
fn plugin_rows(point: &str, icon: &str) -> Vec<MenuItem> {
    plugins::contributions(point)
        .into_iter()
        .map(|e| row(&format!("plug:{}|{}", e.id, e.command), &e.label, icon, ""))
        .collect()
}

/// `rows` with the first one starting a new group (menu divider).
fn grouped(mut rows: Vec<MenuItem>) -> Vec<MenuItem> {
    if let Some(f) = rows.first_mut() {
        *f = div(f.clone());
    }
    rows
}

fn main_menu(a: &AppWindow, p: &Player) -> Vec<MenuItem> {
    let paused = p.get("pause").as_deref() == Some("yes");
    let rec = p.get("stream-record").is_some_and(|s| !s.is_empty());
    vec![
        row(
            "play",
            if paused { "Play" } else { "Pause" },
            "play",
            &keymap::hint(A::PlayPause),
        ),
        row("stop", "Stop", "stop", &keymap::hint(A::Stop)),
        row(
            "prev",
            "Previous Item",
            "previous-item",
            &keymap::hint_alt(A::QueueStep),
        ),
        row(
            "next",
            "Next Item",
            "next-item",
            &keymap::hint(A::QueueStep),
        ),
        row(
            "record",
            if rec { "Stop Recording" } else { "Record" },
            "record",
            &keymap::hint(A::Record),
        ),
        div(row(
            "hide-ui",
            if a.get_ui_hidden() {
                "Show UI"
            } else {
                "Hide UI"
            },
            "hide-ui",
            &keymap::hint(A::HideUi),
        )),
        div(sub("playback", "Playback", "playback")),
        sub("plugins", "Plugins", "plugins"),
        sub("video", "Video", "video"),
        sub("audio", "Audio", "audio"),
        sub("subtitle", "Subtitle", "subtitle"),
        div(sub("more", "More Settings", "more-settings")),
        sub("playlist", "Playlist", "playlist"),
        sub("open", "Open Media", "open-media"),
        sub("recent", "Open Recent", "recent"),
        row("quit", "Quit", "quit-vlc", &keymap::hint(A::Quit)),
    ]
}

fn submenu(a: &AppWindow, p: &Player, id: &str) -> Vec<MenuItem> {
    let tracks = |kind: &str, prefix: &str| -> Vec<MenuItem> {
        p.tracks(kind)
            .into_iter()
            .map(|(i, label, on)| check(&format!("{prefix}:{i}"), &label, on))
            .collect()
    };
    match id {
        "playback" => {
            let speed: f64 = p.get("speed").and_then(|s| s.parse().ok()).unwrap_or(1.0);
            let mut v: Vec<MenuItem> = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0]
                .iter()
                .map(|s| {
                    let l = if *s == 1.0 {
                        "Normal speed".into()
                    } else {
                        format!("{s}×")
                    };
                    check(&format!("speed:{s}"), &l, (speed - s).abs() < 0.01)
                })
                .collect();
            v.push(div(check(
                "loop",
                "Loop File",
                p.get("loop-file").is_some_and(|s| s != "no"),
            )));
            v.push(check(
                "repeat",
                "Repeat Queue",
                a.global::<crate::Playback>().get_repeat_on(),
            ));
            v.push(check(
                "shuffle",
                "Shuffle",
                a.global::<crate::Playback>().get_shuffle_on(),
            ));
            v.push(div(row("panel:3", "Speed & Chapters…", "speed", "")));
            v
        }
        "video" => {
            let asp = p.get("video-aspect-override").unwrap_or_default();
            let mut v: Vec<MenuItem> = [
                ("-1", "Default aspect"),
                ("16:9", "16:9"),
                ("4:3", "4:3"),
                ("2.35:1", "2.35:1"),
                ("1:1", "1:1"),
            ]
            .iter()
            .map(|(k, l)| {
                let on = if *k == "-1" {
                    asp.starts_with('-') || asp.is_empty()
                } else {
                    aspect_eq(&asp, k)
                };
                check(&format!("aspect:{k}"), l, on)
            })
            .collect();
            let vt = tracks("video", "vid");
            if vt.len() > 1 {
                let mut first = true;
                for t in vt {
                    v.push(if std::mem::take(&mut first) {
                        div(t)
                    } else {
                        t
                    });
                }
            }
            v.push(div(row("rotate", "Rotate 90°", "rotate", "")));
            v.push(row("snapshot", "Take Snapshot", "snapshot", ""));
            v.push(row(
                "fullscreen",
                "Fullscreen",
                "fullscreen",
                &keymap::hint(A::Fullscreen),
            ));
            v.extend(grouped(plugin_rows(contrib::MENU_VIDEO, "video")));
            v.push(row("panel:2", "Video Adjustments…", "paint", ""));
            v
        }
        "audio" => {
            let mut v = tracks("audio", "aid");
            v.push(div(check(
                "mute",
                "Mute",
                p.get("mute").as_deref() == Some("yes"),
            )));
            v.push(row("add-audio", "Add Audio Track…", "add-audio-tracks", ""));
            v.extend(plugin_rows(contrib::MENU_AUDIO, "audio"));
            v.push(row("panel:1", "Audio Settings…", "audio", ""));
            v
        }
        "subtitle" => {
            let mut v = vec![check(
                "sid:no",
                "Off",
                p.get("sid").as_deref() == Some("no"),
            )];
            v.extend(tracks("sub", "sid"));
            v.push(div(row(
                "add-sub",
                "Add Subtitle File…",
                "add-subtitle-tracks",
                "",
            )));
            v.extend(plugin_rows(contrib::MENU_SUBTITLES, "subtitle"));
            v.push(row("panel:0", "Subtitle Settings…", "subtitle", ""));
            v
        }
        "more" => vec![
            row("mini", "Miniplayer", "pip", &keymap::hint(A::Miniplayer)),
            row("customize", "Customize Player", "paint", ""),
            row(
                "shortcuts",
                "Keyboard Shortcuts",
                "hotkeys",
                &keymap::hint(A::Shortcuts),
            ),
            row(
                "settings",
                "Settings",
                "settings",
                &keymap::hint(A::Settings),
            ),
        ],
        // Right-click on the player: plugin actions (`context.player`), then Extensions.
        "plugins" => {
            let mut v = plugin_rows(contrib::CONTEXT_PLAYER, "plugins");
            if !plugins::contributions(contrib::DRAWER_PANEL).is_empty() {
                v.push(div(row("drawer", "Side drawer", "plugins", "")));
            }
            v.push(div(row(
                "extensions",
                "Extensions…",
                "plugins",
                &keymap::hint(A::Extensions),
            )));
            v
        }
        "playlist" => vec![
            row("panel:4", "Show Queue", "playlist", ""),
            row("shuffle-now", "Shuffle Queue", "shuffle", ""),
            row("save-queue", "Save as Playlist", "export-playlist", ""),
            row("clear-queue", "Clear Queue", "remove", ""),
        ],
        "open" => vec![
            row(
                "open-file",
                "Open File(s)",
                "file",
                &keymap::hint(A::OpenFile),
            ),
            row("open-folder", "Open Folder", "folder", ""),
            row("open-network", "Open Stream / URL…", "streaming", ""),
            row("library", "Library", "recent", ""),
        ],
        "recent" => recent_menu(),
        "lib:addpl" => {
            let mut v: Vec<MenuItem> = store::playlists()
                .into_iter()
                .map(|(n, path)| {
                    row(
                        &format!("lib:addpl:{}", path.to_string_lossy()),
                        &n,
                        "playlist",
                        "",
                    )
                })
                .collect();
            v.push(div(row("lib:newpl", "New Playlist…", "add", "")));
            v
        }
        _ => Vec::new(),
    }
}

fn aspect_eq(cur: &str, k: &str) -> bool {
    let (a, b) = k.split_once(':').unwrap_or((k, "1"));
    let want = a.parse::<f64>().unwrap_or(0.0) / b.parse::<f64>().unwrap_or(1.0);
    cur.parse::<f64>().is_ok_and(|c| (c - want).abs() < 0.01)
}

/// "Open Recent": the newest entries with their type on the right, then Clear.
fn recent_menu() -> Vec<MenuItem> {
    let mut v: Vec<MenuItem> = store::recent_entries()
        .into_iter()
        .take(10)
        .map(|e| {
            row(
                &format!("recent:{}", e.target),
                &e.display_title(),
                e.kind.menu_icon(),
                e.kind.label(),
            )
        })
        .collect();
    if v.is_empty() {
        v.push(row("recent-none", "Nothing yet", "recent", ""));
    } else {
        v.push(div(row("recent-clear", "Clear Recent", "remove", "")));
    }
    v
}

/// Context menu rows for a library item; Recent entries add "Remove from Recent".
fn lib_menu_for(
    kind: &str,
    pinned: bool,
    watched: bool,
    progress: bool,
    recent: bool,
) -> Vec<MenuItem> {
    let mut v = lib_menu(kind, pinned, watched, progress);
    if recent {
        v.push(div(row(
            "lib:forget-recent",
            "Remove from Recent",
            "remove",
            "",
        )));
    }
    v
}

/// "Sync" and "Open Source" after Play / Shuffle for a playlist an extension keeps in
/// sync.
fn synced_rows(v: &mut Vec<MenuItem>, synced: bool) {
    if synced {
        let at = v.len().min(2);
        v.insert(at, row("lib:playlist-sync", "Sync", "resync", ""));
        v.insert(
            at + 1,
            row("lib:playlist-open", "Open Source", "go-to-source", ""),
        );
    }
}

/// Context menu rows for a library item kind.
fn lib_menu(kind: &str, pinned: bool, watched: bool, progress: bool) -> Vec<MenuItem> {
    let fav = if pinned {
        "Remove from Favorites"
    } else {
        "Favorite"
    };
    match kind {
        "folder" => vec![
            row("lib:play", "Open", "play", ""),
            row("lib:add-file", "Add File", "add-file", ""),
            row("lib:add-folder", "Add Folder", "add-folder", ""),
            row(
                "lib:edit-subs",
                "Edit Subtitle Tracks",
                "edit-subtitle-tracks",
                "",
            ),
            row(
                "lib:edit-audio",
                "Edit Audio Tracks",
                "edit-audio-tracks",
                "",
            ),
            row("lib:pin", fav, "favorite", ""),
            row(
                "lib:reveal",
                "Show in File Explorer",
                "show-in-file-explorer",
                "",
            ),
            row("lib:resync", "Resync", "resync", ""),
            row("lib:info", "Info", "info", ""),
        ],
        "playlist" | "group" => {
            let mut v = vec![
                row("lib:play", "Play", "play", ""),
                row("lib:shuffle", "Shuffle", "shuffle", ""),
            ];
            if kind == "playlist" {
                v.push(row("lib:export", "Export Playlist", "export-playlist", ""));
            }
            v.push(row("lib:enqueue", "Add to Queue", "add-to-queue", ""));
            v.push(row(
                "lib:info",
                "Properties & Metadata",
                "properties-metadata",
                "",
            ));
            if kind == "playlist" {
                v.push(div(row("lib:delete", "Delete Playlist", "remove", "")));
            }
            v
        }
        "action" => Vec::new(),
        "bookmark" => vec![
            row("lib:play", "Play from Here", "play", ""),
            div(row("lib:delete-bookmark", "Delete Bookmark", "remove", "")),
        ],
        _ => {
            let mut v = vec![
                row("lib:play", "Play", "play", ""),
                row(
                    "lib:watched",
                    if watched {
                        "Mark as Unwatched"
                    } else {
                        "Mark as Watched"
                    },
                    "mark-as-watched",
                    "",
                ),
                row("lib:pin", fav, "favorite", ""),
                row("lib:enqueue", "Add to Queue", "add-to-queue", ""),
                sub("lib:addpl", "Add to Playlist", "add-to-playlist"),
                row(
                    "lib:reveal",
                    "Show in File Explorer",
                    "show-in-file-explorer",
                    "",
                ),
                row("lib:source", "Go to Source", "go-to-source", ""),
                row("lib:sub", "Add Subtitle Tracks", "add-subtitle-tracks", ""),
                row("lib:audio", "Add Audio Tracks", "add-audio-tracks", ""),
                row(
                    "lib:info",
                    "Properties & Metadata",
                    "properties-metadata",
                    "",
                ),
            ];
            if progress {
                v.push(div(row("lib:forget", "Clear Progress", "remove", "")));
            }
            v
        }
    }
}

#[derive(Default)]
struct Ctx {
    key: String,
    kind: String,
}

pub fn setup(app: &AppWindow, player: &Rc<Player>, cfg: Rc<RefCell<Config>>) {
    let items = Rc::new(VecModel::<MenuItem>::default());
    let subs = Rc::new(VecModel::<MenuItem>::default());
    app.global::<crate::Menus>()
        .set_menu_items(ModelRc::from(items.clone()));
    app.global::<crate::Menus>()
        .set_sub_items(ModelRc::from(subs.clone()));
    let ctx = Rc::new(RefCell::new(Ctx::default()));

    {
        let (w, p, items, subs, ctx, cfg) = (
            app.as_weak(),
            player.clone(),
            items.clone(),
            subs.clone(),
            ctx.clone(),
            cfg.clone(),
        );
        // ctx for "lib": "<key>\n<kind>\n<pinned>\n<watched>\n<progress>"
        app.global::<crate::Menus>()
            .on_menu_request(move |kind, x, y, c| {
                let Some(a) = w.upgrade() else { return };
                let c = c.to_string();
                let v = match kind.as_str() {
                    "main" => {
                        // Player menu: plugin entries get no item argument.
                        *ctx.borrow_mut() = Ctx::default();
                        main_menu(&a, &p)
                    }
                    "recent" => recent_menu(),
                    "lib" => {
                        let f: Vec<&str> = c.split('\n').collect();
                        let g = |i: usize| f.get(i).copied().unwrap_or("");
                        *ctx.borrow_mut() = Ctx {
                            key: g(0).into(),
                            kind: g(1).into(),
                        };
                        let mut v =
                            lib_menu_for(g(1), g(2) == "1", g(3) == "1", g(4) == "1", g(5) == "1");
                        // `context.item`: plugin actions for files and folders.
                        if matches!(g(1), "audio" | "video" | "folder") {
                            v.extend(grouped(plugin_rows(contrib::CONTEXT_ITEM, "plugins")));
                        }
                        if g(1) == "playlist" {
                            let src = crate::store::playlist_source(std::path::Path::new(g(0)));
                            synced_rows(&mut v, src.is_some());
                        }
                        v
                    }
                    "sort" => {
                        let cur = cfg.borrow().library.sort.clone();
                        SORTS
                            .iter()
                            .map(|(k, l)| check(&format!("sort:{k}"), l, sort_key(&cur) == *k))
                            .collect()
                    }
                    "view" => {
                        let grid = cfg.borrow().library.view != "list";
                        vec![
                            MenuItem {
                                checked: grid,
                                ..row("view:grid", "Grid View", "grid-view", "")
                            },
                            MenuItem {
                                checked: !grid,
                                ..row("view:list", "List View", "list-view", "")
                            },
                        ]
                    }
                    _ => Vec::new(),
                };
                if v.is_empty() {
                    return;
                }
                items.set_vec(v);
                subs.set_vec(Vec::new());
                a.global::<crate::Menus>()
                    .set_menu_width(if kind == "main" { 292.0 } else { 265.0 });
                a.global::<crate::Menus>().set_menu_x(x);
                a.global::<crate::Menus>().set_menu_y(y);
                a.global::<crate::Menus>().set_sub_open("".into());
                a.global::<crate::Menus>().set_menu_kind(kind);
            });
    }
    {
        let (w, p, subs) = (app.as_weak(), player.clone(), subs.clone());
        app.global::<crate::Menus>().on_menu_sub(move |id| {
            let Some(a) = w.upgrade() else { return };
            subs.set_vec(submenu(&a, &p, &id));
        });
    }
    {
        let (w, p, ctx) = (app.as_weak(), player.clone(), ctx.clone());
        app.global::<crate::Menus>().on_menu_activate(move |id| {
            let Some(a) = w.upgrade() else { return };
            let id = id.to_string();
            // Rows that only open a submenu do nothing on click.
            if [
                "playback",
                "video",
                "audio",
                "subtitle",
                "more",
                "playlist",
                "open",
                "lib:addpl",
            ]
            .contains(&id.as_str())
            {
                return;
            }
            a.global::<crate::Menus>().set_menu_kind("".into());
            a.global::<crate::Menus>().set_sub_open("".into());
            activate(&a, &p, &ctx.borrow(), &id);
        });
    }
}

fn activate(a: &AppWindow, p: &Player, ctx: &Ctx, id: &str) {
    let (head, arg) = id.split_once(':').unwrap_or((id, ""));
    match head {
        "lib" => a.global::<crate::Library>().invoke_menu(
            ctx.key.clone().into(),
            ctx.kind.clone().into(),
            arg.into(),
        ),
        "sort" => a.global::<crate::Library>().invoke_set_sort(arg.into()),
        // Plugin contribution: the library item's key is the argument of `context.item`.
        "plug" => {
            let (pid, cmd) = arg.split_once('|').unwrap_or((arg, ""));
            let args = if ctx.key.is_empty() {
                Vec::new()
            } else {
                vec![ctx.key.clone()]
            };
            plugins::run(pid, cmd, &args);
        }
        "recent" => a.global::<crate::Library>().invoke_open_recent(arg.into()),
        "recent-clear" => a.global::<crate::Library>().invoke_clear_recent(),
        "view" => a.global::<crate::Library>().invoke_set_grid(arg == "grid"),
        "speed" => p.set("speed", arg),
        "aspect" => p.set("video-aspect-override", arg),
        "vid" | "aid" | "sid" => p.set(head, arg),
        "panel" => a.invoke_bar_act(
            match arg {
                "0" => "subtitles",
                "1" => "audio",
                "2" => "video",
                "3" => "speed",
                _ => "queue",
            }
            .into(),
        ),
        _ => match id {
            "play" => p.toggle_pause(),
            "stop" => a.global::<crate::Playback>().invoke_stop(),
            "prev" => p.playlist_step(false),
            "next" => p.playlist_step(true),
            "record" => a.global::<crate::Playback>().invoke_toggle_record(),
            "hide-ui" => crate::toggle_ui_hidden(a),
            "quit" => {
                let _ = a.hide();
                let _ = slint::quit_event_loop();
            }
            "loop" => {
                let on = p.get("loop-file").is_some_and(|s| s != "no");
                p.set("loop-file", if on { "no" } else { "inf" });
            }
            "repeat" | "shuffle" | "snapshot" | "fullscreen" | "mute" => {
                a.invoke_bar_act(id.into())
            }
            "rotate" => {
                let r: i64 = p
                    .get("video-rotate")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                p.set("video-rotate", &((r + 90) % 360).to_string());
            }
            "add-sub" if a.global::<crate::Subtitles>().get_plugin() => a.invoke_add_subs(),
            "add-sub" | "add-audio" => {
                let sub = id == "add-sub";
                if let Some(f) = rfd::FileDialog::new()
                    .set_title(if sub {
                        "Add subtitle file"
                    } else {
                        "Add audio track"
                    })
                    .pick_file()
                {
                    let f = f.to_string_lossy().into_owned();
                    p.cmd(if sub { "sub-add" } else { "audio-add" }, &[&f, "select"]);
                }
            }
            "mini" => a.global::<crate::Playback>().invoke_toggle_mini(),
            "customize" => a.global::<crate::Overlays>().set_customize_open(true),
            "shortcuts" => a.global::<crate::Overlays>().set_shortcuts_open(true),
            "extensions" => crate::extwin::open(a, "", false),
            "drawer" => crate::drawer::toggle(a),
            "settings" => {
                a.global::<crate::Overlays>().set_settings_open(true);
                a.global::<crate::Settings>().invoke_refresh(
                    a.global::<crate::Settings>().get_category(),
                    "".into(),
                    false,
                );
            }
            "shuffle-now" => p.run("playlist-shuffle"),
            "save-queue" => {
                let name = queue_name();
                a.global::<crate::Panel>().invoke_playlist_save(name.into());
                a.global::<crate::Notices>()
                    .set_resume_text("Saved the queue as a playlist".into());
            }
            "clear-queue" => a.global::<crate::Panel>().invoke_queue_clear(),
            "open-file" => a.global::<crate::Playback>().invoke_open_file(),
            "open-folder" => a.global::<crate::Library>().invoke_open_folder(),
            "open-network" => crate::streams::open_page(a, "streams:url"),
            "library" => a.global::<crate::Overlays>().set_library_open(true),
            _ => {}
        },
    }
}

/// A free "Queue N" playlist name.
fn queue_name() -> String {
    let names: Vec<String> = store::playlists().into_iter().map(|(n, _)| n).collect();
    (1..)
        .map(|i| format!("Queue {i}"))
        .find(|n| !names.contains(n))
        .unwrap_or_default()
}
