//! Control-socket commands: playback.

use super::*;
use crate::player::Player;
use crate::AppWindow;

pub(super) fn ping(_a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, String> {
    Ok(json!("pong"))
}

pub(super) fn status_cmd(a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    Ok(status(a, p))
}

pub(super) fn play(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    p.set("pause", "no");
    ok()
}

pub(super) fn pause(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    p.set("pause", "yes");
    ok()
}

pub(super) fn toggle(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    p.toggle_pause();
    ok()
}

pub(super) fn stop(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    p.set("pause", "yes");
    p.seek_abs(0.0);
    ok()
}

pub(super) fn seek(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    if let Some(t) = args["position"].as_f64() {
        p.seek_abs(t.max(0.0));
    } else if let Some(d) = args["offset"].as_f64() {
        p.seek_rel(d);
    } else {
        return Err("give \"position\" (seconds) or \"offset\" (seconds)".into());
    }
    ok()
}

pub(super) fn volume(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
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

pub(super) fn open_cmd(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    let t = target(args)?;
    p.open(&t);
    ok()
}

pub(super) fn next(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    p.playlist_step(true);
    ok()
}

pub(super) fn prev(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    p.playlist_step(false);
    ok()
}

pub(super) fn subtitles(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    let on = args["enabled"].as_bool().ok_or("missing \"enabled\"")?;
    p.set("sub-visibility", if on { "yes" } else { "no" });
    if on && p.get("sid").as_deref() == Some("no") {
        p.set("sid", "auto");
    }
    ok()
}

pub(super) fn eq_presets(a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, String> {
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

pub(super) fn eq_preset(a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, String> {
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

pub(super) fn tracks(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
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

pub(super) fn set_track(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
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

pub(super) fn chapters(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
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

pub(super) fn seek_chapter(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
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

pub(super) fn speed(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    let s = args["speed"].as_f64().ok_or("missing \"speed\"")?;
    if !(0.1..=4.0).contains(&s) {
        return Err("speed must be between 0.1 and 4".into());
    }
    p.set("speed", &format!("{s}"));
    Ok(json!({ "speed": s }))
}

pub(super) fn screenshot(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
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

pub(super) fn open_stream(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
    let t = target(args)?;
    if !t.contains("://") {
        return Err("\"target\" must be a stream URL (http(s)://, rtmp://, udp://, …)".into());
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

pub(super) fn now_playing(_a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, String> {
    let t = crate::nowplaying::current();
    Ok(
        json!({ "id": crate::nowplaying::current_id(), "title": t.title,
        "artist": t.artist, "album": t.album, "album_artist": t.album_artist,
        "genre": t.genre, "track": t.track, "duration": t.duration, "cover": t.cover,
        "kind": t.kind, "source": t.source, "path": t.path, "mbid": t.mbid,
        "service": t.service }),
    )
}

pub(super) fn sleep(_a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, String> {
    // Sleep timer: {"timer": "off" | "min:<n>" | "eof" | "chapter" | "items:<n>"}
    // or {"minutes": n}; no arguments returns the state.
    let spec = match (&args["timer"], &args["minutes"]) {
        (Value::String(s), _) => Some(s.clone()),
        (_, Value::Number(n)) => Some(format!("min:{}", n.as_f64().unwrap_or(0.0).round() as i64)),
        (Value::Null, Value::Null) => None,
        _ => return Err(
            "\"timer\" is a string (off, min:<n>, eof, chapter, items:<n>) or \"minutes\" a number"
                .into(),
        ),
    };
    if let Some(s) = spec {
        crate::sleep::set_spec(&s)?;
    }
    Ok(crate::sleep::state_json())
}

pub(super) fn ab_loop(a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    // No arguments: the state. {"set": "a" | "b"} at the playhead,
    // {"a": n, "b": n} (null clears that point), {"clear": true}.
    if args["clear"].as_bool() == Some(true) {
        crate::abloop::set_both(a, p, Some(None), Some(None))?;
    } else if let Some(w) = args["set"].as_str() {
        let which = match w {
            "a" | "A" => crate::abloop::Point::A,
            "b" | "B" => crate::abloop::Point::B,
            _ => return Err("\"set\" is \"a\" or \"b\"".into()),
        };
        if p.get("time-pos").is_none() {
            return Err("nothing is playing".into());
        }
        crate::abloop::set(a, p, which);
    } else {
        let pt = |k: &str| -> Result<Option<Option<f64>>, String> {
            match &args[k] {
                Value::Null if args.get(k).is_none() => Ok(None),
                Value::Null => Ok(Some(None)),
                Value::Number(n) => Ok(Some(n.as_f64())),
                _ => Err(format!("\"{k}\" is seconds or null")),
            }
        };
        let (x, y) = (pt("a")?, pt("b")?);
        if x.is_some() || y.is_some() {
            crate::abloop::set_both(a, p, x, y)?;
        }
    }
    Ok(crate::abloop::state_json(p))
}

pub(super) fn url_history(a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, String> {
    // Show the Streams hub's URL bar with its history list open (scripts,
    // screenshots).
    crate::streams::open_page(a, "streams:url");
    let st = a.global::<crate::Streams>();
    st.set_open_history(st.get_open_history() + 1);
    Ok(json!({ "urls": st.get_history().row_count() }))
}

pub(super) fn sub_offset(a: &AppWindow, p: &Player, args: &Value) -> Result<Value, String> {
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

pub(super) fn sub_sync(a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, String> {
    if crate::plugins::package_script(crate::suboffset::SYNC_PACKAGE).is_none() {
        return Err("install and enable the Subtitle Auto-Sync plugin (subtitle-sync) to align subtitles to the audio".into());
    }
    crate::suboffset::sync(a, p);
    Ok(
        json!({ "started": true, "note": "alignment runs in the background; the synced copy is loaded when done" }),
    )
}
