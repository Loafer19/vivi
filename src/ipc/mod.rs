//! Local control socket for external tools such as `vivi-mcp` (the MCP server for AI
//! agents, issue #68). Off by default: enable "Allow AI agents and scripts" in Settings →
//! Controls → Remotes (`general.remote_control`) or start Vivi with `VIVI_IPC=1`.
//!
//! Protocol: one JSON object per line, `{"cmd": "...", "args": {...}}`, answered with
//! `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}`. The socket lives at
//! `$VIVI_SOCKET`, else `$XDG_RUNTIME_DIR/vivi.sock`, else `<cache dir>/vivi.sock`, and is
//! created with mode 0600, so only the current user can connect. Unix only.

#[cfg(unix)]
use serde_json::{json, Value};

#[cfg(unix)]
mod bookmarks;
#[cfg(unix)]
mod playback;
#[cfg(unix)]
mod plugins;
#[cfg(unix)]
mod queue;
#[cfg(unix)]
mod ui;

#[cfg(unix)]
/// Path of the control socket (keep in sync with `src/bin/vivi-mcp.rs`).
pub fn socket_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("VIVI_SOCKET") {
        return p.into();
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => std::path::PathBuf::from(d).join("vivi.sock"),
        _ => crate::store::cache_dir().join("vivi.sock"),
    }
}

#[cfg(unix)]
/// Library search (runs on the connection thread, not the UI thread).
fn search(args: &Value) -> Result<Value, String> {
    let q = args["query"].as_str().unwrap_or("").to_lowercase();
    let limit = args["limit"].as_u64().unwrap_or(25).clamp(1, 500) as usize;
    let kind = args["kind"].as_str().unwrap_or("");
    let conn = crate::library::open_db(&crate::library::db_path()).map_err(|e| e.to_string())?;
    let words: Vec<&str> = q.split_whitespace().collect();
    let hits: Vec<Value> = crate::library::all(&conn)
        .into_iter()
        .filter(|m| kind.is_empty() || m.kind.as_str() == kind)
        .filter(|m| {
            let hay = format!(
                "{} {} {} {} {}",
                m.title, m.artist, m.album, m.genre, m.path
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .take(limit)
        .map(|m| {
            json!({
                "path": m.path, "kind": m.kind.as_str(), "title": m.title, "artist": m.artist,
                "album": m.album, "genre": m.genre, "duration": m.duration,
            })
        })
        .collect();
    Ok(json!({ "count": hits.len(), "items": hits }))
}

/// Radio + TV directory search (network; runs on the connection thread). `kind` radio
/// asks only the radio directories (SomaFM, radio-browser) and tv only iptv-org; your
/// favorites and playlists are split by [`network_streams::stream_kind`].
#[cfg(unix)]
fn search_streams(args: &Value) -> Result<Value, String> {
    use crate::plugins::network_streams as ns;
    let q = args["query"].as_str().unwrap_or("").trim();
    if q.is_empty() {
        return Err("missing \"query\"".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(25).clamp(1, 200) as usize;
    let kind = match args["kind"].as_str().unwrap_or("") {
        "" => None,
        k => Some(ns::Kind::parse(k).ok_or_else(|| format!("unknown kind {k:?} (radio or tv)"))?),
    };
    let items = stream_items(ns::search_kind(q, kind)?, limit);
    Ok(json!({ "count": items.len(), "items": items }))
}

/// `search_streams` result rows.
#[cfg(unix)]
pub fn stream_items(v: Vec<crate::plugins::network_streams::Stream>, limit: usize) -> Vec<Value> {
    use crate::plugins::network_streams as ns;
    v.into_iter()
        .take(limit)
        .map(|s| {
            json!({ "name": ns::display_name(&s.name), "url": s.url, "kind": ns::stream_kind(&s).label(),
                "group": s.group, "country": s.country, "codec": s.codec, "bitrate": s.bitrate,
                "logo": s.logo, "source": s.source, "tvg_id": s.tvg_id })
        })
        .collect()
}

/// Recent list entries matching `args.query` (words in title, subtitle or target) and
/// `args.kind` (video, audio, folder, playlist, stream, web, music), newest first.
#[cfg(unix)]
pub fn recent_items(v: &[crate::store::RecentEntry], args: &Value) -> Value {
    let q = args["query"].as_str().unwrap_or("").to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    let kind = args["kind"].as_str().unwrap_or("").to_lowercase();
    let items: Vec<Value> = v
        .iter()
        .filter(|e| {
            let k = e.kind.label().to_lowercase();
            kind.is_empty() || k == kind || k.starts_with(&kind)
        })
        .filter(|e| {
            let hay = format!("{} {} {}", e.title, e.sub, e.target).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .map(|e| {
            json!({ "target": e.target, "kind": e.kind.label(), "title": e.title,
                "subtitle": e.sub, "image": e.image })
        })
        .collect();
    json!(items)
}

/// Saved playlists (Library → Playlists): name, path and entry count.
#[cfg(unix)]
fn saved_playlists() -> Value {
    json!(crate::store::playlists()
        .into_iter()
        .map(|(name, path)| {
            let n = std::fs::read_to_string(&path)
                .map(|t| {
                    t.lines()
                        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                        .count()
                })
                .unwrap_or(0);
            json!({ "name": name, "path": path.to_string_lossy(), "entries": n })
        })
        .collect::<Vec<_>>())
}

/// mpv `playlist-move` target that puts entry `from` at final index `to` in a queue of `n`.
#[cfg(unix)]
pub fn move_target(from: i64, to: i64, n: i64) -> Result<i64, String> {
    if !(0..n).contains(&from) || !(0..n).contains(&to) {
        return Err(format!(
            "index out of range (queue has {n} entries, 0-based)"
        ));
    }
    // mpv inserts before the target index, counted before the entry is removed.
    Ok(if to > from { to + 1 } else { to })
}

/// Contribution points whose entries agents may run (UI-only points are left out).
#[cfg(unix)]
pub const AGENT_POINTS: [&str; 9] = [
    crate::plugins::contrib::PALETTE_COMMAND,
    crate::plugins::contrib::CONTEXT_PLAYER,
    crate::plugins::contrib::CONTEXT_ITEM,
    crate::plugins::contrib::MENU_SUBTITLES,
    crate::plugins::contrib::MENU_AUDIO,
    crate::plugins::contrib::MENU_VIDEO,
    crate::plugins::contrib::TRAY_ITEM,
    crate::plugins::contrib::CONTROLBAR_BUTTON,
    crate::plugins::contrib::SIDEBAR_SOURCE,
];

/// Plugin commands from contributions, one per (plugin, command): the first label wins and
/// `points` lists every place it shows up. `takes_target` = it is a library-item action.
#[cfg(unix)]
pub fn plugin_commands(by_point: &[(&str, Vec<crate::plugins::Contributed>)]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for (point, list) in by_point {
        for c in list {
            if let Some(e) = out
                .iter_mut()
                .find(|e| e["plugin"] == c.id.as_str() && e["command"] == c.command.as_str())
            {
                if let Some(p) = e["points"].as_array_mut() {
                    p.push(json!(point));
                }
                if *point == crate::plugins::contrib::CONTEXT_ITEM {
                    e["takes_target"] = json!(true);
                }
                continue;
            }
            out.push(json!({
                "plugin": c.id, "plugin_name": c.name, "command": c.command, "label": c.label,
                "points": [point],
                "where": crate::plugins::contrib::describe(point),
                "takes_target": *point == crate::plugins::contrib::CONTEXT_ITEM,
            }));
        }
    }
    out
}

/// An explicit screenshot path: an image file that does not exist yet.
#[cfg(unix)]
pub fn screenshot_path(s: &str, overwrite: bool) -> Result<std::path::PathBuf, String> {
    let p = std::path::PathBuf::from(s.trim());
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) {
        return Err("\"path\" must end in .png, .jpg, .jpeg or .webp".into());
    }
    if !p.is_absolute() {
        return Err("\"path\" must be absolute".into());
    }
    if p.exists() && !overwrite {
        return Err(format!(
            "{} exists; pass \"overwrite\": true to replace it",
            p.display()
        ));
    }
    Ok(p)
}

#[cfg(unix)]
pub fn setup(app: &crate::AppWindow, player: &std::rc::Rc<crate::player::Player>, enabled: bool) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::mpsc;

    let allowed = enabled || std::env::var_os("VIVI_IPC").is_some_and(|v| v != "0");
    ALLOWED.store(allowed, std::sync::atomic::Ordering::Relaxed);
    // Off, or already listening (turned on again in Settings).
    if !allowed || bound() {
        return;
    }
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        eprintln!(
            "vivi: control socket {} is used by another instance",
            path.display()
        );
        return;
    }
    let _ = std::fs::remove_file(&path);
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("vivi: control socket {}: {e}", path.display());
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    BOUND.store(true, std::sync::atomic::Ordering::Relaxed);

    type Req = (String, Value, mpsc::Sender<Result<Value, String>>);
    let (tx, rx) = mpsc::channel::<Req>();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            // Turned off in Settings: the socket stays (it is removed on exit) but
            // nobody gets in (#94).
            if !allowed_now() {
                drop(stream);
                continue;
            }
            let tx = tx.clone();
            std::thread::spawn(move || {
                let Ok(mut out) = stream.try_clone() else {
                    return;
                };
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if !allowed_now() {
                        let _ = writeln!(
                            out,
                            "{}",
                            json!({ "ok": false, "error": "remote control is off in Vivi's settings" })
                        );
                        break;
                    }
                    if line.trim().is_empty() {
                        continue;
                    }
                    let res = match serde_json::from_str::<Value>(&line) {
                        Err(e) => Err(format!("bad request: {e}")),
                        Ok(req) => {
                            let cmd = req["cmd"].as_str().unwrap_or("").to_string();
                            let args = req.get("args").cloned().unwrap_or(json!({}));
                            // Ask the UI thread (5 s timeout).
                            let ui = |cmd: &str, args: Value| {
                                let (rtx, rrx) = mpsc::channel();
                                if tx.send((cmd.to_string(), args, rtx)).is_err() {
                                    return Err("vivi is closing".to_string());
                                }
                                rrx.recv_timeout(std::time::Duration::from_secs(5))
                                    .unwrap_or_else(|_| Err("vivi did not answer".into()))
                            };
                            if cmd == "search" {
                                search(&args)
                            } else if cmd == "recent" {
                                Ok(recent_items(&crate::store::recent_entries(), &args))
                            } else if cmd == "saved_playlists" {
                                Ok(saved_playlists())
                            } else if cmd == "search_streams" {
                                // Only while Network Streams is on (asked on the UI thread).
                                ui("__streams_on", json!({})).and_then(|_| search_streams(&args))
                            } else if cmd.starts_with("__") {
                                // Internal UI-thread requests (they carry the API key).
                                Err(format!("unknown command {cmd:?}"))
                            } else if cmd == "search_subtitles" {
                                // Network on this thread; the key and file come from the UI.
                                ui("__subs_ctx", json!({})).and_then(|c| {
                                    crate::plugins::subtitle_search::mcp_search(&c, &args)
                                })
                            } else if cmd == "load_subtitle" {
                                ui("__subs_ctx", json!({}))
                                    .and_then(|c| crate::plugins::subtitle_search::mcp_download(&c, &args))
                                    .and_then(|(p, id, left)| {
                                        let path = p.to_string_lossy().into_owned();
                                        let lang = args["language"].as_str().unwrap_or("");
                                        ui("__subs_load", json!({ "path": path, "file_id": id, "language": lang }))?;
                                        Ok(json!({ "loaded": true, "file_id": id, "downloads_left": left }))
                                    })
                            } else {
                                ui(&cmd, args)
                            }
                        }
                    };
                    let reply = match res {
                        Ok(r) => json!({ "ok": true, "result": r }),
                        Err(e) => json!({ "ok": false, "error": e }),
                    };
                    if writeln!(out, "{reply}").is_err() {
                        break;
                    }
                }
            });
        }
    });

    let w = app.as_weak();
    let p = player.clone();
    crate::ticker::every(std::time::Duration::from_millis(40), move || {
        let Some(a) = w.upgrade() else { return };
        while let Ok((cmd, args, reply)) = rx.try_recv() {
            let _ = reply.send(handle(&a, &p, &cmd, &args));
        }
    });
}

#[cfg(not(unix))]
pub fn setup(_: &crate::AppWindow, _: &std::rc::Rc<crate::player::Player>, _: bool) {}

#[cfg(unix)]
/// This instance owns the control socket (removed again on exit).
static BOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// "Allow AI agents and scripts" (or `VIVI_IPC=1`) is on: the socket answers.
static ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Settings → Controls → Remotes → *Allow AI agents and scripts* changed while running.
/// Off refuses new connections and requests at once; mpv-independent, so no restart.
pub fn set_allowed(on: bool) {
    ALLOWED.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn allowed_now() -> bool {
    ALLOWED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether agents and scripts can reach this instance now: allowed and the socket is
/// ours (the top-bar remote chip, #94).
pub fn listening() -> bool {
    allowed_now() && bound()
}

/// Whether this instance owns the control socket.
pub fn bound() -> bool {
    #[cfg(unix)]
    {
        BOUND.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// On exit: remove our control socket and the mpv IPC socket the Remote Control
/// plugin opened (`$XDG_RUNTIME_DIR/vivi-mpv-<pid>.sock`).
pub fn cleanup() {
    // The control socket exists only on Unix (`setup` is a no-op elsewhere).
    #[cfg(unix)]
    if BOUND.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = std::fs::remove_file(socket_path());
    }
    if let Some(d) = runtime_dir() {
        let _ = std::fs::remove_file(d.join(format!("vivi-mpv-{}.sock", std::process::id())));
    }
}

fn runtime_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
}

/// `vivi-mpv-<pid>.sock` files left by instances that were killed (no exit cleanup).
pub fn sweep_stale() {
    let Some(d) = runtime_dir() else { return };
    let Ok(rd) = std::fs::read_dir(&d) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("vivi-mpv-"))
            .and_then(|n| n.strip_suffix(".sock"))
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if !stale_pid(pid) {
            continue;
        }
        let _ = std::fs::remove_file(e.path());
    }
}

/// No process with this pid (Linux: /proc); elsewhere nothing is treated as stale.
fn stale_pid(pid: u32) -> bool {
    pid != std::process::id()
        && std::path::Path::new("/proc/self").exists()
        && !std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(unix)]
fn target(args: &Value) -> Result<String, String> {
    let t = args["target"].as_str().unwrap_or("").trim();
    if t.is_empty() {
        return Err("missing \"target\" (file path or URL)".into());
    }
    Ok(t.strip_prefix("file://").unwrap_or(t).to_string())
}

#[cfg(unix)]
fn status(a: &crate::AppWindow, p: &crate::player::Player) -> Value {
    let num = |k: &str| p.get(k).and_then(|s| s.parse::<f64>().ok());
    let path = p.get("path").unwrap_or_default();
    let fx = a.global::<crate::FxState>();
    let eq_on = fx.get_eq().row_data(11).unwrap_or(0.0) > 0.5;
    json!({
        "has_media": !path.is_empty(),
        "playing": !path.is_empty() && p.get("pause").as_deref() == Some("no"),
        "path": path,
        "title": a.global::<crate::Playback>().get_media_title().to_string(),
        "artist": p.get("metadata/by-key/artist").unwrap_or_default(),
        "album": p.get("metadata/by-key/album").unwrap_or_default(),
        "position": num("time-pos").unwrap_or(0.0),
        "duration": num("duration").unwrap_or(0.0),
        "volume": num("volume").unwrap_or(0.0),
        "muted": p.get("mute").as_deref() == Some("yes"),
        "speed": num("speed").unwrap_or(1.0),
        "subtitles": p.get("sub-visibility").as_deref() == Some("yes"),
        "subtitle_track": p.get("sid").unwrap_or_default(),
        "subtitle_offset": p.get("sub-delay").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0),
        "playlist_pos": num("playlist-pos").unwrap_or(-1.0) as i64,
        "playlist_count": num("playlist-count").unwrap_or(0.0) as i64,
        "eq_preset": if eq_on { fx.get_preset().to_string() } else { "Off".into() },
        "is_stream": path.contains("://"),
        "has_video": !path.is_empty() && p.get("vid").is_some_and(|v| v != "no" && !v.is_empty()),
        "audio_track": p.get("aid").unwrap_or_default(),
        "chapter": num("chapter").map(|c| c as i64).unwrap_or(-1),
        "chapter_title": p.get("chapter-metadata/title").unwrap_or_default(),
        "repeat": repeat_mode(p),
        "shuffle": a.global::<crate::Playback>().get_shuffle_on(),
        "lyrics_available": a.global::<crate::Lyrics>().get_lines().row_count() > 0,
        "sleep": crate::sleep::state_json(),
    })
}

#[cfg(unix)]
fn repeat_mode(p: &crate::player::Player) -> &'static str {
    let on = |k: &str| p.get(k).is_some_and(|v| v != "no" && !v.is_empty());
    if on("loop-file") {
        "one"
    } else if on("loop-playlist") {
        "all"
    } else {
        "off"
    }
}

/// `"open"` of the `drawer` command: true / absent = open, false = close, "toggle".
#[cfg_attr(not(unix), allow(dead_code))]
pub fn drawer_open_arg(v: &serde_json::Value) -> Result<Option<bool>, String> {
    use serde_json::Value;
    match v {
        Value::Null => Ok(Some(true)),
        Value::Bool(b) => Ok(Some(*b)),
        Value::String(s) if s == "toggle" => Ok(None),
        _ => Err("\"open\" is true, false or \"toggle\"".into()),
    }
}

#[cfg(unix)]
use slint::{ComponentHandle, Model};

#[cfg(unix)]
/// A control-socket command, run on the UI thread.
type Handler = fn(&crate::AppWindow, &crate::player::Player, &Value) -> crate::error::Result<Value>;

/// Every command the UI thread answers, by wire name (commands answered on the
/// connection thread, such as `search`, are in [`setup`]). Names starting with `__`
/// are internal requests from the connection thread.
#[cfg(unix)]
const COMMANDS: &[(&str, Handler)] = &[
    // playback.rs
    ("ping", playback::ping),
    ("status", playback::status_cmd),
    ("play", playback::play),
    ("pause", playback::pause),
    ("toggle", playback::toggle),
    ("stop", playback::stop),
    ("seek", playback::seek),
    ("volume", playback::volume),
    ("open", playback::open_cmd),
    ("next", playback::next),
    ("prev", playback::prev),
    ("subtitles", playback::subtitles),
    ("eq_presets", playback::eq_presets),
    ("eq_preset", playback::eq_preset),
    ("tracks", playback::tracks),
    ("set_track", playback::set_track),
    ("chapters", playback::chapters),
    ("seek_chapter", playback::seek_chapter),
    ("speed", playback::speed),
    ("screenshot", playback::screenshot),
    ("open_stream", playback::open_stream),
    ("now_playing", playback::now_playing),
    ("sleep", playback::sleep),
    ("ab_loop", playback::ab_loop),
    ("url_history", playback::url_history),
    ("sub_offset", playback::sub_offset),
    ("sub_sync", playback::sub_sync),
    // queue.rs
    ("playlist_add", queue::playlist_add),
    ("playlist_list", queue::playlist_list),
    ("playlist_play", queue::playlist_play),
    ("playlist_remove", queue::playlist_remove),
    ("playlist_move", queue::playlist_move),
    ("playlist_clear", queue::playlist_clear),
    ("repeat", queue::repeat),
    ("shuffle", queue::shuffle),
    ("open_saved_playlist", queue::open_saved_playlist),
    ("save_queue", queue::save_queue),
    // bookmarks.rs
    ("bookmarks", bookmarks::bookmarks),
    ("bookmark_add", bookmarks::bookmark_add),
    ("bookmark_remove", bookmarks::bookmark_remove),
    ("bookmark_jump", bookmarks::bookmark_jump),
    ("bookmarks_export", bookmarks::bookmarks_export),
    // ui.rs
    ("drawer", ui::drawer),
    ("page", ui::page),
    // plugins.rs
    ("plugins", plugins::plugins),
    ("plugin_info", plugins::plugin_info),
    ("lyrics", plugins::lyrics),
    ("__streams_on", plugins::streams_on),
    ("__subs_ctx", plugins::subs_ctx),
    ("__subs_load", plugins::subs_load),
    ("plugin_commands", plugins::plugin_commands_cmd),
    ("run_plugin_command", plugins::run_plugin_command),
    ("lyrics_offset", plugins::lyrics_offset),
    ("karaoke", plugins::karaoke),
    ("visualizer", plugins::visualizer),
];

#[cfg(unix)]
fn ok() -> crate::error::Result<Value> {
    Ok(json!("ok"))
}

#[cfg(unix)]
fn handle(
    a: &crate::AppWindow,
    p: &crate::player::Player,
    cmd: &str,
    args: &Value,
) -> Result<Value, String> {
    if cmd.is_empty() {
        return Err("missing \"cmd\"".into());
    }
    match COMMANDS.iter().find(|(n, _)| *n == cmd) {
        Some((_, h)) => h(a, p, args).map_err(String::from),
        None => Err(format!("unknown command {cmd:?}")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn command_names_are_unique() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.0).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n);
        assert!(n >= 55);
    }
    use crate::plugins::Contributed;

    #[test]
    fn drawer_open_argument() {
        assert_eq!(drawer_open_arg(&Value::Null), Ok(Some(true)));
        assert_eq!(drawer_open_arg(&json!(false)), Ok(Some(false)));
        assert_eq!(drawer_open_arg(&json!("toggle")), Ok(None));
        assert!(drawer_open_arg(&json!(3)).is_err());
    }

    #[test]
    fn move_targets_follow_mpv_insert_semantics() {
        assert_eq!(move_target(0, 2, 3), Ok(3)); // down: insert after the old slot
        assert_eq!(move_target(2, 0, 3), Ok(0));
        assert_eq!(move_target(1, 1, 3), Ok(1));
        assert!(move_target(0, 3, 3).is_err());
        assert!(move_target(-1, 0, 3).is_err());
    }

    #[test]
    fn stream_rows_carry_their_kind() {
        use crate::plugins::network_streams::Stream;
        let v = vec![
            Stream {
                name: "Groove%20Salad".into(),
                url: "http://s".into(),
                source: "SomaFM".into(),
                ..Default::default()
            },
            Stream {
                name: "News".into(),
                url: "http://t".into(),
                source: "iptv-org".into(),
                ..Default::default()
            },
        ];
        let r = stream_items(v, 5);
        assert_eq!(r[0]["name"], "Groove Salad");
        assert_eq!(r[0]["kind"], "radio");
        assert_eq!(r[1]["kind"], "tv");
        assert_eq!(stream_items(vec![Stream::default(); 3], 2).len(), 2);
    }

    #[test]
    fn recent_filters_by_words_and_kind() {
        use crate::store::{RecentEntry, RecentKind};
        let mut a = RecentEntry::new("https://ice.somafm.com/groovesalad");
        a.kind = RecentKind::Stream;
        a.title = "Groove Salad".into();
        let mut b = RecentEntry::new("/films/Alien.mkv");
        b.kind = RecentKind::Video;
        let v = [a, b];
        let all = recent_items(&v, &json!({}));
        assert_eq!(all.as_array().unwrap().len(), 2);
        let r = recent_items(&v, &json!({ "query": "groove salad" }));
        assert_eq!(r[0]["title"], "Groove Salad");
        assert_eq!(r.as_array().unwrap().len(), 1);
        let r = recent_items(&v, &json!({ "kind": "video" }));
        assert_eq!(r[0]["target"], "/films/Alien.mkv");
    }

    #[test]
    fn plugin_commands_dedupe_and_mark_item_actions() {
        use crate::plugins::contrib::{CONTEXT_ITEM, CONTEXT_PLAYER, PALETTE_COMMAND};
        let c = |id: &str, label: &str, cmd: &str| Contributed {
            id: id.into(),
            name: id.into(),
            icon: String::new(),
            label: label.into(),
            command: cmd.into(),
        };
        let v = plugin_commands(&[
            (
                PALETTE_COMMAND,
                vec![c("dl", "Downloads: add the playing stream", "add-current")],
            ),
            (
                CONTEXT_PLAYER,
                vec![c("dl", "Download for offline", "add-current")],
            ),
            (CONTEXT_ITEM, vec![c("tagger", "Tag this file", "tag")]),
        ]);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["label"], "Downloads: add the playing stream");
        assert_eq!(v[0]["points"].as_array().unwrap().len(), 2);
        assert_eq!(v[0]["takes_target"], false);
        assert_eq!(v[1]["takes_target"], true);
    }

    #[test]
    fn screenshot_paths_are_new_image_files() {
        let d = std::env::temp_dir();
        assert!(screenshot_path("/tmp/x.txt", false).is_err());
        assert!(screenshot_path("rel/x.png", false).is_err());
        let f = d.join(format!("vivi-ipc-test-{}.png", std::process::id()));
        assert!(screenshot_path(f.to_str().unwrap(), false).is_ok());
        std::fs::write(&f, b"x").unwrap();
        assert!(screenshot_path(f.to_str().unwrap(), false).is_err());
        assert!(screenshot_path(f.to_str().unwrap(), true).is_ok());
        let _ = std::fs::remove_file(f);
    }
}
