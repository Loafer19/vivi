//! Local control socket for external tools such as `vivi-mcp` (the MCP server for AI
//! agents, issue #68). Off by default: enable "Remote control" in Settings → General
//! (`general.remote_control`) or start Vivi with `VIVI_IPC=1`.
//!
//! Protocol: one JSON object per line, `{"cmd": "...", "args": {...}}`, answered with
//! `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}`. The socket lives at
//! `$VIVI_SOCKET`, else `$XDG_RUNTIME_DIR/vivi.sock`, else `<cache dir>/vivi.sock`, and is
//! created with mode 0600, so only the current user can connect. Unix only.

#[cfg(unix)]
use serde_json::{json, Value};

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
        .filter(|m| kind.is_empty() || m.kind == kind)
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
                "path": m.path, "kind": m.kind, "title": m.title, "artist": m.artist,
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

    if !(enabled || std::env::var_os("VIVI_IPC").is_some_and(|v| v != "0")) {
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
            let tx = tx.clone();
            std::thread::spawn(move || {
                let Ok(mut out) = stream.try_clone() else {
                    return;
                };
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
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
    let timer = Box::leak(Box::new(slint::Timer::default()));
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(40),
        move || {
            let Some(a) = w.upgrade() else { return };
            while let Ok((cmd, args, reply)) = rx.try_recv() {
                let _ = reply.send(handle(&a, &p, &cmd, &args));
            }
        },
    );
}

#[cfg(not(unix))]
pub fn setup(_: &crate::AppWindow, _: &std::rc::Rc<crate::player::Player>, _: bool) {}

#[cfg(unix)]
/// This instance owns the control socket (removed again on exit).
static BOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this instance owns the control socket (top-bar remote indicator).
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
fn handle(
    a: &crate::AppWindow,
    p: &crate::player::Player,
    cmd: &str,
    args: &Value,
) -> Result<Value, String> {
    let ok = || Ok(json!("ok"));
    match cmd {
        "ping" => Ok(json!("pong")),
        "status" => Ok(status(a, p)),
        "play" => {
            p.set("pause", "no");
            ok()
        }
        "pause" => {
            p.set("pause", "yes");
            ok()
        }
        "toggle" => {
            p.toggle_pause();
            ok()
        }
        "stop" => {
            p.set("pause", "yes");
            p.seek_abs(0.0);
            ok()
        }
        "seek" => {
            if let Some(t) = args["position"].as_f64() {
                p.seek_abs(t.max(0.0));
            } else if let Some(d) = args["offset"].as_f64() {
                p.seek_rel(d);
            } else {
                return Err("give \"position\" (seconds) or \"offset\" (seconds)".into());
            }
            ok()
        }
        "volume" => {
            if let Some(v) = args["level"].as_f64() {
                p.set_volume(v);
            }
            if let Some(m) = args["muted"].as_bool() {
                p.set("mute", if m { "yes" } else { "no" });
            }
            Ok(json!({
                "volume": p.get("volume").and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0),
                "muted": p.get("mute").as_deref() == Some("yes"),
            }))
        }
        "open" => {
            let t = target(args)?;
            p.open(&t);
            ok()
        }
        "playlist_add" => {
            let t = target(args)?;
            p.enqueue(&t);
            ok()
        }
        "playlist_list" => Ok(json!(p
            .queue()
            .into_iter()
            .enumerate()
            .map(
                |(i, (f, cur))| json!({ "index": i, "path": f, "current": cur,
                "title": p.get(&format!("playlist/{i}/title")).unwrap_or_default() })
            )
            .collect::<Vec<_>>())),
        "playlist_play" => {
            let i = args["index"].as_i64().ok_or("missing \"index\"")?;
            p.play_index(i as i32);
            ok()
        }
        "next" => {
            p.playlist_step(true);
            ok()
        }
        "prev" => {
            p.playlist_step(false);
            ok()
        }
        "subtitles" => {
            let on = args["enabled"].as_bool().ok_or("missing \"enabled\"")?;
            p.set("sub-visibility", if on { "yes" } else { "no" });
            if on && p.get("sid").as_deref() == Some("no") {
                p.set("sid", "auto");
            }
            ok()
        }
        "eq_presets" => {
            let fx = a.global::<crate::FxState>();
            let names = |m: slint::ModelRc<slint::SharedString>| {
                m.iter().map(|s| s.to_string()).collect::<Vec<_>>()
            };
            Ok(json!({
                "current": fx.get_preset().to_string(),
                "enabled": fx.get_eq().row_data(11).unwrap_or(0.0) > 0.5,
                "builtin": names(fx.get_builtin()),
                "custom": names(fx.get_custom()),
            }))
        }
        "eq_preset" => {
            let name = args["name"].as_str().ok_or("missing \"name\"")?;
            let fx = a.global::<crate::FxState>();
            if name.eq_ignore_ascii_case("off") {
                fx.invoke_set("eq".into(), 11, 0.0);
                return ok();
            }
            let known = fx
                .get_builtin()
                .iter()
                .chain(fx.get_custom().iter())
                .any(|s| s == name);
            if !known {
                return Err(format!("unknown preset {name:?}; see eq_presets"));
            }
            fx.invoke_preset_action("select".into(), name.into(), "".into());
            ok()
        }
        "tracks" => {
            let kind = match args["type"].as_str().unwrap_or("audio") {
                "sub" | "subtitle" | "subtitles" => "sub",
                "video" => "video",
                _ => "audio",
            };
            Ok(json!(p
                .tracks(kind)
                .into_iter()
                .map(|(id, label, sel)| json!({ "id": id, "title": label, "selected": sel }))
                .collect::<Vec<_>>()))
        }
        "set_track" => {
            let (prop, kind) = match args["type"].as_str().unwrap_or("audio") {
                "sub" | "subtitle" | "subtitles" => ("sid", "sub"),
                _ => ("aid", "audio"),
            };
            let v = match &args["id"] {
                Value::Number(n) => {
                    let id = n.as_i64().ok_or("\"id\" must be an integer")?;
                    if !p.tracks(kind).iter().any(|t| t.0 == id) {
                        return Err(format!("no {kind} track {id}; see the track list"));
                    }
                    id.to_string()
                }
                Value::String(s) if s == "no" || s == "off" => "no".into(),
                Value::String(s) if s == "auto" => "auto".into(),
                _ => return Err("give \"id\" (track id, \"auto\" or \"no\")".into()),
            };
            p.set(prop, &v);
            if prop == "sid" {
                p.set("sub-visibility", if v == "no" { "no" } else { "yes" });
            }
            ok()
        }
        "chapters" => {
            let cur = p
                .get("chapter")
                .and_then(|c| c.parse::<i64>().ok())
                .unwrap_or(-1);
            Ok(
                json!({ "current": cur, "chapters": p.chapters().into_iter().enumerate()
                .map(|(i, (t, s))| json!({ "index": i, "title": t, "start": s }))
                .collect::<Vec<_>>() }),
            )
        }
        "seek_chapter" => {
            let n = p.chapters().len() as i64;
            if n == 0 {
                return Err("this item has no chapters".into());
            }
            if let Some(i) = args["index"].as_i64() {
                if !(0..n).contains(&i) {
                    return Err(format!("chapter index out of range (0..{n})"));
                }
                p.set("chapter", &i.to_string());
            } else if let Some(d) = args["offset"].as_i64() {
                p.cmd("add", &["chapter", &d.to_string()]);
            } else {
                return Err("give \"index\" (0-based) or \"offset\" (+1 / -1)".into());
            }
            ok()
        }
        "speed" => {
            let s = args["speed"].as_f64().ok_or("missing \"speed\"")?;
            if !(0.1..=4.0).contains(&s) {
                return Err("speed must be between 0.1 and 4".into());
            }
            p.set("speed", &format!("{s}"));
            Ok(json!({ "speed": s }))
        }
        "screenshot" => {
            if p.get("path").unwrap_or_default().is_empty() {
                return Err("nothing is playing".into());
            }
            if p.get("vid").as_deref().is_none_or(|v| v == "no") {
                return Err("no video to capture (audio-only item)".into());
            }
            let mode = match args["mode"].as_str().unwrap_or("subtitles") {
                "video" => "video",
                "window" => "window",
                _ => "subtitles",
            };
            let path = match args["path"].as_str().filter(|s| !s.trim().is_empty()) {
                Some(s) => screenshot_path(s, args["overwrite"].as_bool() == Some(true))?,
                None => {
                    let d = p.get("screenshot-directory").unwrap_or_default();
                    let d = match d.strip_prefix("~/") {
                        Some(r) => crate::store::home().join(r),
                        None if d.is_empty() => crate::store::home().join("Pictures"),
                        None => d.into(),
                    };
                    let ext = p
                        .get("screenshot-format")
                        .filter(|f| !f.is_empty())
                        .unwrap_or_else(|| "png".into());
                    let t = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis())
                        .unwrap_or(0);
                    d.join(format!("vivi-{t}.{ext}"))
                }
            };
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let ps = path.to_string_lossy().into_owned();
            p.try_cmd("screenshot-to-file", &[&ps, mode])?;
            Ok(json!({ "path": ps }))
        }
        "open_stream" => {
            let t = target(args)?;
            if !t.contains("://") {
                return Err(
                    "\"target\" must be a stream URL (http(s)://, rtmp://, udp://, …)".into(),
                );
            }
            if let Some(n) = args["name"].as_str().filter(|n| !n.trim().is_empty()) {
                crate::loading::set_name(&t, n.trim());
            }
            if args["queue"].as_bool() == Some(true) {
                p.enqueue(&t);
            } else {
                p.open(&t);
            }
            ok()
        }
        "now_playing" => {
            let t = crate::nowplaying::current();
            Ok(
                json!({ "id": crate::nowplaying::current_id(), "title": t.title,
                "artist": t.artist, "album": t.album, "album_artist": t.album_artist,
                "genre": t.genre, "track": t.track, "duration": t.duration, "cover": t.cover,
                "kind": t.kind, "source": t.source, "path": t.path, "mbid": t.mbid,
                "service": t.service }),
            )
        }
        "plugins" => Ok(json!(crate::plugins::states()
            .into_iter()
            .map(
                |(m, on)| json!({ "id": m.id, "name": m.name, "version": m.version,
                "kind": format!("{:?}", m.origin).to_lowercase(), "enabled": on })
            )
            .collect::<Vec<_>>())),
        "plugin_info" => {
            let id = args["id"].as_str().ok_or("missing \"id\"")?;
            let (m, on) = crate::plugins::states()
                .into_iter()
                .find(|(m, _)| m.id == id)
                .ok_or_else(|| format!("unknown plugin {id:?}; see list_plugins"))?;
            Ok(
                json!({ "id": m.id, "name": m.name, "description": m.description,
                "version": m.version, "author": m.author,
                "kind": format!("{:?}", m.origin).to_lowercase(), "enabled": on,
                "settings": m.settings.iter().map(|s| json!({ "key": s.key, "title": s.title, "info": s.info }))
                    .collect::<Vec<_>>() }),
            )
        }
        "lyrics" => {
            let lines: Vec<String> = a
                .global::<crate::Lyrics>()
                .get_lines()
                .iter()
                .map(|s| s.to_string())
                .collect();
            Ok(
                json!({ "available": !lines.is_empty(), "synced": !lines.is_empty() && !a.global::<crate::Lyrics>().get_plain(),
                "current_line": a.global::<crate::Lyrics>().get_index(), "lines": lines,
                "source": a.global::<crate::Lyrics>().get_source().as_str(), "offset_seconds": a.global::<crate::Lyrics>().get_offset(),
                "karaoke": a.global::<crate::Overlays>().get_karaoke_open() }),
            )
        }
        "sleep" => {
            // Sleep timer: {"timer": "off" | "min:<n>" | "eof" | "chapter" | "items:<n>"}
            // or {"minutes": n}; no arguments returns the state.
            let spec = match (&args["timer"], &args["minutes"]) {
                (Value::String(s), _) => Some(s.clone()),
                (_, Value::Number(n)) => Some(format!("min:{}", n.as_f64().unwrap_or(0.0).round() as i64)),
                (Value::Null, Value::Null) => None,
                _ => return Err("\"timer\" is a string (off, min:<n>, eof, chapter, items:<n>) or \"minutes\" a number".into()),
            };
            if let Some(s) = spec {
                crate::sleep::set_spec(&s)?;
            }
            Ok(crate::sleep::state_json())
        }
        "bookmarks" => {
            // Bookmarks of the playing file; {"add": true} bookmarks the position first.
            if args["add"].as_bool() == Some(true) {
                crate::bookmarks::add_current(p, false);
            }
            Ok(json!(crate::bookmarks::current_marks()))
        }
        "drawer" => {
            // Open / close / toggle the plugin side drawer (screenshots, scripts); it
            // shows while media plays.
            let open_it = drawer_open_arg(&args["open"])?;
            let g = a.global::<crate::Drawer>();
            let id = match args["plugin"].as_str() {
                Some(id) => id.to_string(),
                None if a.global::<crate::Overlays>().get_drawer_open() => g.get_tab().into(),
                None => String::new(),
            };
            if !id.is_empty()
                && !crate::plugins::contributions(crate::plugins::contrib::DRAWER_PANEL)
                    .iter()
                    .any(|e| e.id == id)
            {
                return Err(format!(
                    "plugin {id:?} has no drawer panel (or is turned off)"
                ));
            }
            let open = crate::drawer::request(a, &id, open_it);
            let tabs: Vec<String> = g.get_tabs().iter().map(|t| t.id.to_string()).collect();
            Ok(
                json!({ "open": open, "tab": g.get_tab().as_str(), "tabs": tabs,
                "shown": open && a.global::<crate::Playback>().get_has_media() }),
            )
        }
        "page" => {
            // Open a Streams hub place (#86): {"page": "streams:radio"} etc. (scripts,
            // screenshots; see src/streams/route.rs).
            let page = args["page"].as_str().unwrap_or("streams");
            if crate::streams::route::parse(page)
                .or_else(|| crate::streams::route::alias(page))
                .is_none()
            {
                return Err(format!("unknown page {page:?}"));
            }
            crate::streams::open_page(a, page);
            Ok(json!({ "page": page }))
        }
        "__streams_on" => {
            if crate::plugins::active(crate::plugins::network_streams::ID) {
                ok()
            } else {
                Err(
                    "the Network Streams plugin is turned off (Extensions → Network Streams)"
                        .into(),
                )
            }
        }
        "__subs_ctx" => {
            crate::plugins::query::<Value>(crate::plugins::subtitle_search::ID, "mcp-context")
                .ok_or_else(|| "the Subtitle Search plugin is turned off".to_string())
        }
        "__subs_load" => {
            let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
            let id = args["file_id"].as_i64().unwrap_or(0).to_string();
            crate::plugins::command(
                crate::plugins::subtitle_search::ID,
                "load-file",
                &[s("path"), id, s("language")],
            );
            ok()
        }
        "sub_offset" => {
            let arg = if args["reset"].as_bool() == Some(true) {
                "reset".to_string()
            } else if let Some(v) = args["seconds"].as_f64() {
                format!("set:{v}")
            } else if let Some(d) = args["delta"].as_f64() {
                d.to_string()
            } else {
                return Err("give \"seconds\", \"delta\" or \"reset\": true".into());
            };
            crate::suboffset::command(a, p, &arg);
            let o = crate::suboffset::current(p);
            Ok(
                json!({ "offset_seconds": o, "saved_for_file": !p.get("path").unwrap_or_default().is_empty() }),
            )
        }
        "sub_sync" => {
            if crate::plugins::package_script(crate::suboffset::SYNC_PACKAGE).is_none() {
                return Err("install and enable the Subtitle Auto-Sync plugin (subtitle-sync) to align subtitles to the audio".into());
            }
            crate::suboffset::sync(a, p);
            Ok(
                json!({ "started": true, "note": "alignment runs in the background; the synced copy is loaded when done" }),
            )
        }
        "playlist_remove" => {
            let n = p.queue().len() as i64;
            let i = args["index"].as_i64().ok_or("missing \"index\"")?;
            if !(0..n).contains(&i) {
                return Err(format!(
                    "index out of range (queue has {n} entries, 0-based)"
                ));
            }
            p.remove_index(i as i32);
            Ok(json!({ "removed": i, "count": n - 1 }))
        }
        "playlist_move" => {
            let n = p.queue().len() as i64;
            let from = args["from"].as_i64().ok_or("missing \"from\"")?;
            let to = args["to"].as_i64().ok_or("missing \"to\"")?;
            let t = move_target(from, to, n)?;
            if from != to {
                p.try_cmd("playlist-move", &[&from.to_string(), &t.to_string()])?;
            }
            ok()
        }
        "playlist_clear" => {
            let n = p.queue().len();
            if args["keep_current"].as_bool() == Some(false) {
                p.run("stop");
            } else {
                p.run("playlist-clear"); // mpv keeps the playing entry
            }
            Ok(json!({ "removed": n.saturating_sub(p.queue().len()) }))
        }
        "repeat" => {
            let m = args["mode"]
                .as_str()
                .ok_or("missing \"mode\" (off, one, all)")?;
            let (file, list) = match m {
                "off" => ("no", "no"),
                "one" => ("inf", "no"),
                "all" => ("no", "inf"),
                _ => return Err("\"mode\" must be off, one or all".into()),
            };
            p.set("loop-file", file);
            p.set("loop-playlist", list);
            a.global::<crate::Playback>().set_repeat_on(m == "one");
            Ok(json!({ "repeat": m }))
        }
        "shuffle" => {
            let on = args["enabled"].as_bool().ok_or("missing \"enabled\"")?;
            crate::loopsync::set_shuffle(a, p, on);
            Ok(json!({ "shuffle": on }))
        }
        "open_saved_playlist" => {
            let name = args["name"].as_str().ok_or("missing \"name\"")?;
            let (_, path) = crate::store::playlists()
                .into_iter()
                .find(|(n, _)| n == name)
                .ok_or_else(|| format!("no saved playlist {name:?}; see list_saved_playlists"))?;
            let t = path.to_string_lossy().into_owned();
            if args["queue"].as_bool() == Some(true) {
                p.enqueue(&t);
            } else {
                p.open(&t);
            }
            ok()
        }
        "save_queue" => {
            let name = args["name"]
                .as_str()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .ok_or("missing \"name\"")?;
            let files: Vec<String> = p.queue().into_iter().map(|(f, _)| f).collect();
            if files.is_empty() {
                return Err("the queue is empty".into());
            }
            let exists = crate::store::playlists().iter().any(|(n, _)| n == name);
            if exists && args["overwrite"].as_bool() != Some(true) {
                return Err(format!(
                    "playlist {name:?} exists; pass \"overwrite\": true to replace it"
                ));
            }
            let path = crate::store::save_playlist(name, &files).map_err(|e| e.to_string())?;
            Ok(json!({ "path": path.to_string_lossy(), "entries": files.len() }))
        }
        "plugin_commands" => {
            let by: Vec<(&str, Vec<crate::plugins::Contributed>)> = AGENT_POINTS
                .iter()
                .map(|pt| (*pt, crate::plugins::contributions(pt)))
                .collect();
            Ok(json!(plugin_commands(&by)))
        }
        "run_plugin_command" => {
            let pid = args["plugin"].as_str().ok_or("missing \"plugin\"")?;
            let c = args["command"].as_str().unwrap_or("");
            let by: Vec<(&str, Vec<crate::plugins::Contributed>)> = AGENT_POINTS
                .iter()
                .map(|pt| (*pt, crate::plugins::contributions(pt)))
                .collect();
            let all = plugin_commands(&by);
            let e = all
                .iter()
                .find(|e| e["plugin"] == pid && e["command"] == c)
                .ok_or_else(|| {
                    format!(
                        "plugin {pid:?} has no command {c:?} (or is off); see list_plugin_commands"
                    )
                })?;
            let mut extra = Vec::new();
            if e["takes_target"] == true {
                match args["target"].as_str().filter(|t| !t.trim().is_empty()) {
                    Some(t) => extra.push(t.trim().to_string()),
                    None if e["points"].as_array().is_some_and(|p| p.len() == 1) => {
                        return Err(
                            "this command acts on a library item: give \"target\" (path)".into(),
                        )
                    }
                    None => {}
                }
            }
            if !crate::plugins::run(pid, c, &extra) {
                return Err(format!("plugin {pid:?} is off"));
            }
            Ok(json!({ "ran": e["label"] }))
        }
        "lyrics_offset" => {
            let l = a.global::<crate::Lyrics>();
            if l.get_lines().row_count() == 0 {
                return Err("no lyrics are loaded for this track".into());
            }
            let cur = l.get_offset() as f64;
            let arg = if args["reset"].as_bool() == Some(true) {
                "reset".to_string()
            } else if let Some(v) = args["seconds"].as_f64() {
                (v - cur).to_string()
            } else if let Some(d) = args["delta"].as_f64() {
                d.to_string()
            } else {
                return Err("give \"seconds\", \"delta\" or \"reset\": true".into());
            };
            crate::plugins::command(crate::plugins::lyrics::ID, "offset", &[arg]);
            Ok(json!({ "offset_seconds": l.get_offset() }))
        }
        "karaoke" => {
            let on = args["enabled"].as_bool().ok_or("missing \"enabled\"")?;
            if on && a.global::<crate::Lyrics>().get_lines().row_count() == 0 {
                return Err("no lyrics are loaded for this track".into());
            }
            crate::plugins::command(
                crate::plugins::lyrics::ID,
                "karaoke",
                &[if on { "on" } else { "off" }.into()],
            );
            Ok(json!({ "karaoke": a.global::<crate::Overlays>().get_karaoke_open() }))
        }
        "visualizer" => {
            match args["mode"].as_str().map(str::trim) {
                None | Some("") => {}
                Some(m) if m.eq_ignore_ascii_case("next") || m.eq_ignore_ascii_case("prev") => {
                    if !crate::viz::step(
                        a,
                        if m.eq_ignore_ascii_case("next") {
                            1
                        } else {
                            -1
                        },
                    ) {
                        return Err("the visualizer is not showing: it needs music or radio without video, and the Music Visualizer plugin on".into());
                    }
                }
                Some(m) => crate::viz::select(a, m)?,
            }
            let (mode, modes, showing) = crate::viz::state(a);
            Ok(json!({ "mode": mode, "modes": modes, "showing": showing }))
        }
        "" => Err("missing \"cmd\"".into()),
        _ => Err(format!("unknown command {cmd:?}")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
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
