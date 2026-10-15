//! `vivi-mcp`: Model Context Protocol server (stdio transport) that lets AI agents control
//! a running Vivi over its local control socket (see `src/ipc.rs`, issue #68).
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout; logs go to stderr. Uses only
//! `std` and `serde_json`, so it stays tiny and does not link the player or the UI.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const PROTOCOL: &str = "2025-06-18";
const SUPPORTED: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Same lookup as `ipc::socket_path` in the player.
fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("VIVI_SOCKET") {
        return p.into();
    }
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("vivi.sock");
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    cache.join("vivi").join("vivi.sock")
}

/// Send one request to Vivi and return its result.
#[cfg(unix)]
fn call_vivi(cmd: &str, args: Value) -> Result<Value, String> {
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;
    let path = socket_path();
    let mut s = UnixStream::connect(&path).map_err(|e| {
        format!(
            "cannot reach Vivi at {} ({e}). Is Vivi running with remote control enabled \
             (Settings → General → Remote control, or VIVI_IPC=1)?",
            path.display()
        )
    })?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .ok();
    writeln!(s, "{}", json!({ "cmd": cmd, "args": args })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply from Vivi: {e}"))?;
    if v["ok"].as_bool() == Some(true) {
        Ok(v["result"].clone())
    } else {
        Err(v["error"].as_str().unwrap_or("unknown error").to_string())
    }
}

#[cfg(not(unix))]
fn call_vivi(_: &str, _: Value) -> Result<Value, String> {
    Err("vivi-mcp currently supports Linux/macOS only (Unix socket)".into())
}

/// (tool name, description, input schema, vivi command)
fn tools() -> Vec<(&'static str, &'static str, Value, &'static str)> {
    let none = json!({ "type": "object", "properties": {} });
    let tgt = |d: &str| {
        json!({ "type": "object", "required": ["target"],
            "properties": { "target": { "type": "string", "description": d } } })
    };
    vec![
        ("get_status", "Now playing: title, artist, path, position/duration (s), volume, playing state, subtitles, playlist position, EQ preset.", none.clone(), "status"),
        ("play", "Resume playback.", none.clone(), "play"),
        ("pause", "Pause playback.", none.clone(), "pause"),
        ("toggle_pause", "Toggle play/pause.", none.clone(), "toggle"),
        ("stop", "Stop: pause and rewind to the start.", none.clone(), "stop"),
        ("seek", "Seek to an absolute position or by a relative offset, in seconds.",
            json!({ "type": "object", "properties": {
                "position": { "type": "number", "description": "Absolute position in seconds" },
                "offset": { "type": "number", "description": "Relative offset in seconds (negative = back)" } } }), "seek"),
        ("set_volume", "Set the volume (0-100, up to the configured maximum) and/or mute.",
            json!({ "type": "object", "properties": {
                "level": { "type": "number", "minimum": 0 },
                "muted": { "type": "boolean" } } }), "volume"),
        ("open", "Open and play a local file path or a URL (http(s), YouTube via yt-dlp, …), replacing the current item.", tgt("File path or URL"), "open"),
        ("playlist_list", "List the play queue (index, path, current).", none.clone(), "playlist_list"),
        ("playlist_add", "Append a file path or URL to the play queue.", tgt("File path or URL"), "playlist_add"),
        ("playlist_play", "Play the queue entry at an index (0-based).",
            json!({ "type": "object", "required": ["index"], "properties": { "index": { "type": "integer", "minimum": 0 } } }), "playlist_play"),
        ("next", "Next item in the queue.", none.clone(), "next"),
        ("previous", "Previous item in the queue.", none.clone(), "prev"),
        ("search_library", "Search the scanned media library by words in title, artist, album, genre or path. Returns paths usable with open/playlist_add.",
            json!({ "type": "object", "required": ["query"], "properties": {
                "query": { "type": "string" },
                "kind": { "type": "string", "enum": ["audio", "video"] },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 } } }), "search"),
        ("set_subtitles", "Show or hide subtitles.",
            json!({ "type": "object", "required": ["enabled"], "properties": { "enabled": { "type": "boolean" } } }), "subtitles"),
        ("list_eq_presets", "List equalizer presets (built-in and custom) and the current one.", none.clone(), "eq_presets"),
        ("set_eq_preset", "Select an equalizer preset by name (see list_eq_presets), or \"Off\" to disable the equalizer.",
            json!({ "type": "object", "required": ["name"], "properties": { "name": { "type": "string" } } }), "eq_preset"),
        ("list_audio_tracks", "Audio tracks of the current item (id, title/language, selected).",
            json!({ "type": "object", "properties": { "type": { "type": "string", "const": "audio", "default": "audio" } } }), "tracks"),
        ("set_audio_track", "Select an audio track by id (see list_audio_tracks), or \"auto\" / \"no\".",
            json!({ "type": "object", "required": ["id"], "properties": {
                "id": { "type": ["integer", "string"], "description": "Track id, \"auto\" or \"no\"" },
                "type": { "type": "string", "const": "audio", "default": "audio" } } }), "set_track"),
        ("list_subtitle_tracks", "Subtitle tracks of the current item (id, title/language, external, selected).",
            json!({ "type": "object", "properties": { "type": { "type": "string", "const": "sub", "default": "sub" } } }), "tracks"),
        ("set_subtitle_track", "Select a subtitle track by id (see list_subtitle_tracks), \"auto\", or \"no\" to turn subtitles off.",
            json!({ "type": "object", "required": ["id"], "properties": {
                "id": { "type": ["integer", "string"], "description": "Track id, \"auto\" or \"no\"" },
                "type": { "type": "string", "const": "sub", "default": "sub" } } }), "set_track"),
        ("get_chapters", "Chapters of the current item (index, title, start in seconds) and the current chapter.", none.clone(), "chapters"),
        ("seek_chapter", "Jump to a chapter by index (0-based), or by offset (+1 next, -1 previous).",
            json!({ "type": "object", "properties": {
                "index": { "type": "integer", "minimum": 0 },
                "offset": { "type": "integer" } } }), "seek_chapter"),
        ("set_speed", "Set the playback speed (0.1-4, 1 = normal).",
            json!({ "type": "object", "required": ["speed"], "properties": { "speed": { "type": "number", "minimum": 0.1, "maximum": 4 } } }), "speed"),
        ("screenshot", "Save a screenshot of the current video frame to a file and return its path (default: the screenshot folder from Settings → Video).",
            json!({ "type": "object", "properties": {
                "path": { "type": "string", "description": "Output file (.png / .jpg); optional" },
                "mode": { "type": "string", "enum": ["subtitles", "video", "window"], "description": "subtitles (default): video + subtitles; video: video only; window: as shown" } } }), "screenshot"),
        ("open_network_stream", "Play a network stream URL now (Internet radio, IPTV .m3u8/HLS, DASH, RTMP/RTSP/UDP, Icecast/SHOUTcast), or append it to the queue. Use search_streams to find stations and TV channels.",
            json!({ "type": "object", "required": ["target"], "properties": {
                "target": { "type": "string", "description": "Stream URL" },
                "name": { "type": "string", "description": "Display name while it loads (optional)" },
                "queue": { "type": "boolean", "description": "Append to the queue instead of playing now" } } }), "open_stream"),
        ("search_streams", "Search Internet radio (radio-browser.info), iptv-org TV channels, your favorites and your own playlists by name. Returns stream URLs for open_network_stream.",
            json!({ "type": "object", "required": ["query"], "properties": {
                "query": { "type": "string" },
                "kind": { "type": "string", "enum": ["radio", "tv"] },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 } } }), "search_streams"),
        ("list_plugins", "Installed plugins (id, name, version, kind, enabled). Read-only.", none.clone(), "plugins"),
        ("plugin_info", "Details of one plugin: description, version, author, settings (keys and titles, no values). Read-only.",
            json!({ "type": "object", "required": ["id"], "properties": { "id": { "type": "string" } } }), "plugin_info"),
        ("get_lyrics", "Lyrics shown for the current track (from the Lyrics plugin: lines, synced flag, current line index). Empty when none are loaded.", none.clone(), "lyrics"),
    ]
}

fn track_type(tool: &str) -> Option<&'static str> {
    match tool {
        "list_audio_tracks" | "set_audio_track" => Some("audio"),
        "list_subtitle_tracks" | "set_subtitle_track" => Some("sub"),
        _ => None,
    }
}

fn handle(req: &Value) -> Option<Value> {
    let id = req.get("id")?.clone(); // notifications get no reply
    let method = req["method"].as_str().unwrap_or("");
    let params = &req["params"];
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
            let v = if SUPPORTED.contains(&asked) {
                asked
            } else {
                PROTOCOL
            };
            Ok(json!({
                "protocolVersion": v,
                "capabilities": { "tools": {}, "resources": {} },
                "serverInfo": { "name": "vivi-mcp", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Controls the Vivi media player. Use get_status to see what is playing, search_library to find media, then open or playlist_add with a returned path. For radio and TV use search_streams, then open_network_stream with a returned URL.",
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools().into_iter().map(|(n, d, s, _)|
            json!({ "name": n, "description": d, "inputSchema": s })).collect::<Vec<_>>() })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            match tools().into_iter().find(|t| t.0 == name) {
                None => Err((-32602, format!("unknown tool {name:?}"))),
                Some((tool, _, _, cmd)) => {
                    let mut args = params.get("arguments").cloned().unwrap_or(json!({}));
                    if args.is_null() {
                        args = json!({});
                    }
                    // Track tools share one command; the tool name picks the track type.
                    if let (Some(o), Some(t)) = (args.as_object_mut(), track_type(tool)) {
                        o.insert("type".into(), json!(t));
                    }
                    Ok(match call_vivi(cmd, args) {
                        Ok(v) => {
                            let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                            let mut r = json!({ "content": [{ "type": "text", "text": text }], "isError": false });
                            if v.is_object() {
                                r["structuredContent"] = v;
                            }
                            r
                        }
                        Err(e) => {
                            json!({ "content": [{ "type": "text", "text": e }], "isError": true })
                        }
                    })
                }
            }
        }
        "resources/list" => Ok(json!({ "resources": [
            { "uri": "vivi://status", "name": "Now playing", "mimeType": "application/json" },
            { "uri": "vivi://playlist", "name": "Play queue", "mimeType": "application/json" },
        ] })),
        "resources/read" => {
            let uri = params["uri"].as_str().unwrap_or("");
            let cmd = match uri {
                "vivi://status" => Some("status"),
                "vivi://playlist" => Some("playlist_list"),
                _ => None,
            };
            match cmd {
                None => Err((-32002, format!("resource not found: {uri}"))),
                Some(c) => call_vivi(c, json!({})).map_err(|e| (-32603, e)).map(|v| {
                    json!({
                    "contents": [{ "uri": uri, "mimeType": "application/json",
                        "text": serde_json::to_string_pretty(&v).unwrap_or_default() }] })
                }),
            }
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, msg)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
        }
    })
}

fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "vivi-mcp: MCP server (stdio) for the Vivi media player.\nSocket: {}",
            socket_path().display()
        );
        return;
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": format!("parse error: {e}") } })),
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch.iter().filter_map(handle).collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(req) => handle(&req),
        };
        if let Some(r) = reply {
            if writeln!(out, "{r}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
}
