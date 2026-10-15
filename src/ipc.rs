//! Local control socket for external tools such as `vivi-mcp` (the MCP server for AI
//! agents, issue #68). Off by default: enable "Remote control" in Settings → General
//! (`general.remote_control`) or start Vivi with `VIVI_IPC=1`.
//!
//! Protocol: one JSON object per line, `{"cmd": "...", "args": {...}}`, answered with
//! `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}`. The socket lives at
//! `$VIVI_SOCKET`, else `$XDG_RUNTIME_DIR/vivi.sock`, else `<cache dir>/vivi.sock`, and is
//! created with mode 0600, so only the current user can connect. Unix only.

use serde_json::{json, Value};

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
                            if cmd == "search" {
                                search(&args)
                            } else {
                                let (rtx, rrx) = mpsc::channel();
                                if tx.send((cmd, args, rtx)).is_err() {
                                    break;
                                }
                                rrx.recv_timeout(std::time::Duration::from_secs(5))
                                    .unwrap_or_else(|_| Err("vivi did not answer".into()))
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

fn target(args: &Value) -> Result<String, String> {
    let t = args["target"].as_str().unwrap_or("").trim();
    if t.is_empty() {
        return Err("missing \"target\" (file path or URL)".into());
    }
    Ok(t.strip_prefix("file://").unwrap_or(t).to_string())
}

fn status(a: &crate::AppWindow, p: &crate::player::Player) -> Value {
    let num = |k: &str| p.get(k).and_then(|s| s.parse::<f64>().ok());
    let path = p.get("path").unwrap_or_default();
    let fx = a.global::<crate::FxState>();
    let eq_on = fx.get_eq().row_data(11).unwrap_or(0.0) > 0.5;
    json!({
        "has_media": !path.is_empty(),
        "playing": !path.is_empty() && p.get("pause").as_deref() == Some("no"),
        "path": path,
        "title": a.get_media_title().to_string(),
        "artist": p.get("metadata/by-key/artist").unwrap_or_default(),
        "album": p.get("metadata/by-key/album").unwrap_or_default(),
        "position": num("time-pos").unwrap_or(0.0),
        "duration": num("duration").unwrap_or(0.0),
        "volume": num("volume").unwrap_or(0.0),
        "muted": p.get("mute").as_deref() == Some("yes"),
        "speed": num("speed").unwrap_or(1.0),
        "subtitles": p.get("sub-visibility").as_deref() == Some("yes"),
        "subtitle_track": p.get("sid").unwrap_or_default(),
        "playlist_pos": num("playlist-pos").unwrap_or(-1.0) as i64,
        "playlist_count": num("playlist-count").unwrap_or(0.0) as i64,
        "eq_preset": if eq_on { fx.get_preset().to_string() } else { "Off".into() },
    })
}

use slint::{ComponentHandle, Model};

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
            Ok(json!({ "volume": p.get("volume"), "muted": p.get("mute") }))
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
            .map(|(i, (f, cur))| json!({ "index": i, "path": f, "current": cur }))
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
        "" => Err("missing \"cmd\"".into()),
        _ => Err(format!("unknown command {cmd:?}")),
    }
}
