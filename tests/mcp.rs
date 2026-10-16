//! Scripted MCP client test for `vivi-mcp`: a fake Vivi listens on a temporary control
//! socket, and the test drives the real `vivi-mcp` binary over stdio
//! (initialize → tools/list → tools/call).
#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

#[test]
fn mcp_client_lists_tools_plays_and_reads_status() {
    let dir = std::env::temp_dir().join(format!("vivi-mcp-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("vivi.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        let mut playing = String::new();
        for s in listener.incoming().flatten() {
            let mut out = s.try_clone().unwrap();
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                let req: Value = serde_json::from_str(&line).unwrap();
                seen2.lock().unwrap().push(req.clone());
                let reply = match req["cmd"].as_str().unwrap() {
                    "search" => json!({ "ok": true, "result": { "count": 1,
                        "items": [{ "path": "/music/song.flac", "title": "Song" }] } }),
                    "open" => {
                        playing = req["args"]["target"].as_str().unwrap().into();
                        json!({ "ok": true, "result": "ok" })
                    }
                    "tracks" => json!({ "ok": true, "result": [{ "id": 1,
                        "title": req["args"]["type"], "selected": true }] }),
                    "status" => json!({ "ok": true, "result": {
                        "playing": !playing.is_empty(), "path": playing, "title": "Song" } }),
                    c => json!({ "ok": false, "error": format!("unknown command {c:?}") }),
                };
                writeln!(out, "{reply}").unwrap();
            }
        }
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_vivi-mcp"))
        .env("VIVI_SOCKET", &sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut rpc = |id: i64, method: &str, params: Value| -> Value {
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(stdin, "{req}").unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id);
        v
    };

    let init = rpc(
        1,
        "initialize",
        json!({ "protocolVersion": "2025-06-18",
        "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "vivi-mcp");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let tools = rpc(2, "tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "play",
        "pause",
        "stop",
        "seek",
        "set_volume",
        "open",
        "playlist_list",
        "playlist_add",
        "next",
        "previous",
        "get_status",
        "search_library",
        "set_subtitles",
        "list_eq_presets",
        "set_eq_preset",
        "list_audio_tracks",
        "set_audio_track",
        "list_subtitle_tracks",
        "set_subtitle_track",
        "get_chapters",
        "seek_chapter",
        "set_speed",
        "screenshot",
        "open_network_stream",
        "search_streams",
        "list_plugins",
        "plugin_info",
        "get_lyrics",
        "search_subtitles",
        "load_subtitle",
        "set_subtitle_offset",
        "sync_subtitles",
    ] {
        assert!(names.contains(&n), "missing tool {n}");
    }

    let found = rpc(
        3,
        "tools/call",
        json!({ "name": "search_library", "arguments": { "query": "song" } }),
    );
    let path = found["result"]["structuredContent"]["items"][0]["path"]
        .as_str()
        .unwrap()
        .to_string();
    let opened = rpc(
        4,
        "tools/call",
        json!({ "name": "open", "arguments": { "target": path } }),
    );
    assert_eq!(opened["result"]["isError"], false);
    let st = rpc(
        5,
        "tools/call",
        json!({ "name": "get_status", "arguments": {} }),
    );
    assert_eq!(st["result"]["structuredContent"]["playing"], true);
    assert_eq!(
        st["result"]["structuredContent"]["path"],
        "/music/song.flac"
    );

    let subs = rpc(
        7,
        "tools/call",
        json!({ "name": "list_subtitle_tracks", "arguments": {} }),
    );
    assert_eq!(
        subs["result"]["content"][0]["text"]
            .as_str()
            .map(|t| t.contains("\"sub\"")),
        Some(true)
    );
    let aud = rpc(8, "tools/call", json!({ "name": "list_audio_tracks" }));
    assert_eq!(aud["result"]["isError"], false);

    let bad = rpc(6, "tools/call", json!({ "name": "nope", "arguments": {} }));
    assert!(bad["error"]["code"].is_number());

    drop(stdin);
    child.wait().unwrap();
    assert!(seen.lock().unwrap().iter().any(|r| r["cmd"] == "open"));
    let _ = std::fs::remove_dir_all(&dir);
}
