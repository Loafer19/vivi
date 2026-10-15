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

/// Radio + TV directory search (network; runs on the connection thread).
fn search_streams(args: &Value) -> Result<Value, String> {
    let q = args["query"].as_str().unwrap_or("").trim();
    if q.is_empty() {
        return Err("missing \"query\"".into());
    }
    let limit = args["limit"].as_u64().unwrap_or(25).clamp(1, 200) as usize;
    let kind = args["kind"].as_str().unwrap_or("");
    let v = crate::plugins::streams::search_all(q)?;
    let items: Vec<Value> = v
        .into_iter()
        .filter(|s| match kind {
            "radio" => s.source == "radio-browser.info",
            "tv" => s.source != "radio-browser.info",
            _ => true,
        })
        .take(limit)
        .map(|s| {
            json!({ "name": crate::plugins::streams::display_name(&s.name), "url": s.url,
                "group": s.group, "country": s.country, "codec": s.codec, "bitrate": s.bitrate,
                "logo": s.logo, "source": s.source, "tvg_id": s.tvg_id })
        })
        .collect();
    Ok(json!({ "count": items.len(), "items": items }))
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
                            } else if cmd == "search_streams" {
                                search_streams(&args)
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
                Some(s) => std::path::PathBuf::from(s.trim()),
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
            let lines: Vec<String> = a.get_lyrics_lines().iter().map(|s| s.to_string()).collect();
            Ok(
                json!({ "available": !lines.is_empty(), "synced": !a.get_lyrics_plain(),
                "current_line": a.get_lyrics_index(), "lines": lines }),
            )
        }
        "" => Err("missing \"cmd\"".into()),
        _ => Err(format!("unknown command {cmd:?}")),
    }
}
