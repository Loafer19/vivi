//! Control-socket commands: queue.

use super::*;
use crate::error::Error;
use crate::player::Player;
use crate::AppWindow;

pub(super) fn playlist_add(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
    let t = target(args)?;
    p.enqueue(&t);
    ok()
}

pub(super) fn playlist_list(_a: &AppWindow, p: &Player, _args: &Value) -> Result<Value, Error> {
    Ok(json!(p
        .queue()
        .into_iter()
        .enumerate()
        .map(
            |(i, (f, cur))| json!({ "index": i, "path": f, "current": cur,
        "title": p.get(&format!("playlist/{i}/title")).unwrap_or_default() })
        )
        .collect::<Vec<_>>()))
}

pub(super) fn playlist_play(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
    let i = args["index"].as_i64().ok_or("missing \"index\"")?;
    p.play_index(i as i32);
    ok()
}

pub(super) fn playlist_remove(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
    let n = p.queue().len() as i64;
    let i = args["index"].as_i64().ok_or("missing \"index\"")?;
    if !(0..n).contains(&i) {
        return Err(format!("index out of range (queue has {n} entries, 0-based)").into());
    }
    p.remove_index(i as i32);
    Ok(json!({ "removed": i, "count": n - 1 }))
}

pub(super) fn playlist_move(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
    let n = p.queue().len() as i64;
    let from = args["from"].as_i64().ok_or("missing \"from\"")?;
    let to = args["to"].as_i64().ok_or("missing \"to\"")?;
    let t = move_target(from, to, n)?;
    if from != to {
        p.try_cmd("playlist-move", &[&from.to_string(), &t.to_string()])?;
    }
    ok()
}

pub(super) fn playlist_clear(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
    let n = p.queue().len();
    if args["keep_current"].as_bool() == Some(false) {
        p.run("stop");
    } else {
        p.run("playlist-clear"); // mpv keeps the playing entry
    }
    Ok(json!({ "removed": n.saturating_sub(p.queue().len()) }))
}

pub(super) fn repeat(a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
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

pub(super) fn shuffle(a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
    let on = args["enabled"].as_bool().ok_or("missing \"enabled\"")?;
    crate::loopsync::set_shuffle(a, p, on);
    Ok(json!({ "shuffle": on }))
}

pub(super) fn open_saved_playlist(
    _a: &AppWindow,
    p: &Player,
    args: &Value,
) -> Result<Value, Error> {
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

pub(super) fn save_queue(_a: &AppWindow, p: &Player, args: &Value) -> Result<Value, Error> {
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
        return Err(
            format!("playlist {name:?} exists; pass \"overwrite\": true to replace it").into(),
        );
    }
    let path = crate::store::save_playlist(name, &files).map_err(|e| e.to_string())?;
    Ok(json!({ "path": path.to_string_lossy(), "entries": files.len() }))
}
