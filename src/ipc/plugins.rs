//! Control-socket commands: plugins.

use super::*;
use crate::error::Error;
use crate::player::Player;
use crate::AppWindow;

pub(super) fn plugins(_a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, Error> {
    Ok(json!(crate::plugins::states()
        .into_iter()
        .map(
            |(m, on)| json!({ "id": m.id, "name": m.name, "version": m.version,
        "kind": format!("{:?}", m.origin).to_lowercase(), "enabled": on })
        )
        .collect::<Vec<_>>()))
}

pub(super) fn plugin_info(_a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, Error> {
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

pub(super) fn lyrics(a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, Error> {
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

pub(super) fn streams_on(_a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, Error> {
    if crate::plugins::active(crate::plugins::network_streams::ID) {
        ok()
    } else {
        Err("the Network Streams plugin is turned off (Extensions → Network Streams)".into())
    }
}

pub(super) fn subs_ctx(_a: &AppWindow, _p: &Player, _args: &Value) -> Result<Value, Error> {
    crate::plugins::query::<Value>(crate::plugins::subtitle_search::ID, "mcp-context")
        .ok_or_else(|| "the Subtitle Search plugin is turned off".into())
}

pub(super) fn subs_load(_a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, Error> {
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let id = args["file_id"].as_i64().unwrap_or(0).to_string();
    crate::plugins::command(
        crate::plugins::subtitle_search::ID,
        "load-file",
        &[s("path"), id, s("language")],
    );
    ok()
}

pub(super) fn plugin_commands_cmd(
    _a: &AppWindow,
    _p: &Player,
    _args: &Value,
) -> Result<Value, Error> {
    let by: Vec<(&str, Vec<crate::plugins::Contributed>)> = AGENT_POINTS
        .iter()
        .map(|pt| (*pt, crate::plugins::contributions(pt)))
        .collect();
    Ok(json!(plugin_commands(&by)))
}

pub(super) fn run_plugin_command(
    _a: &AppWindow,
    _p: &Player,
    args: &Value,
) -> Result<Value, Error> {
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
            format!("plugin {pid:?} has no command {c:?} (or is off); see list_plugin_commands")
        })?;
    let mut extra = Vec::new();
    if e["takes_target"] == true {
        match args["target"].as_str().filter(|t| !t.trim().is_empty()) {
            Some(t) => extra.push(t.trim().to_string()),
            None if e["points"].as_array().is_some_and(|p| p.len() == 1) => {
                return Err("this command acts on a library item: give \"target\" (path)".into())
            }
            None => {}
        }
    }
    if !crate::plugins::run(pid, c, &extra) {
        return Err(format!("plugin {pid:?} is off").into());
    }
    Ok(json!({ "ran": e["label"] }))
}

pub(super) fn lyrics_offset(a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, Error> {
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

pub(super) fn karaoke(a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, Error> {
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

pub(super) fn visualizer(a: &AppWindow, _p: &Player, args: &Value) -> Result<Value, Error> {
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
