//! Scripted MCP client tests for `vivi-mcp`: a fake Vivi listens on a temporary control
//! socket, and each test drives the real `vivi-mcp` binary over stdio.
#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the fake player reports; tests change it to trigger notifications.
struct State {
    playing: String,
    position: f64,
    plugin_cmds: Value,
    seen: Vec<Value>,
}

struct Fake {
    dir: PathBuf,
    sock: PathBuf,
    state: Arc<Mutex<State>>,
}

fn reply(st: &mut State, dir: &std::path::Path, cmd: &str, args: &Value) -> Value {
    let ok = |r: Value| json!({ "ok": true, "result": r });
    match cmd {
        "search" => ok(json!({ "count": 3, "items": [
            { "path": "/music/song.flac", "title": "Song" },
            { "path": "/music/b.flac" }, { "path": "/music/c.flac" }] })),
        "open" => {
            st.playing = args["target"].as_str().unwrap().into();
            ok(json!("ok"))
        }
        "tracks" => ok(json!([{ "id": 1, "title": args["type"], "selected": true }])),
        "status" => ok(
            json!({ "playing": !st.playing.is_empty(), "path": st.playing,
            "title": "Song", "position": st.position }),
        ),
        "playlist_list" => ok(json!((0..5)
            .map(|i| json!({ "index": i, "path": format!("/q/{i}.mp3"), "current": i == 0 }))
            .collect::<Vec<_>>())),
        "recent" => ok(
            json!([{ "target": "https://radio.example/jazz", "kind": "Stream", "title": "Jazz" }]),
        ),
        "plugin_commands" => ok(st.plugin_cmds.clone()),
        "run_plugin_command" => ok(json!({ "ran": "Downloads: add the playing stream" })),
        "screenshot" => {
            let p = args["path"]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or_else(|| dir.join("shot.png"));
            std::fs::write(&p, b"\x89PNG\r\n\x1a\nfake").unwrap();
            ok(json!({ "path": p.to_string_lossy() }))
        }
        "lyrics" => ok(json!({ "available": false, "lines": [] })),
        "playlist_clear" | "repeat" | "shuffle" | "playlist_move" | "playlist_remove" => {
            ok(json!("ok"))
        }
        c => json!({ "ok": false, "error": format!("unknown command {c:?}") }),
    }
}

fn fake(name: &str) -> Fake {
    let dir = std::env::temp_dir().join(format!("vivi-mcp-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("vivi.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let state = Arc::new(Mutex::new(State {
        playing: String::new(),
        position: 0.0,
        plugin_cmds: json!([{ "plugin": "offline-downloads", "plugin_name": "Offline Downloads",
            "command": "add-current", "label": "Downloads: add the playing stream",
            "points": ["palette.command"], "where": "Command palette (Ctrl+K)", "takes_target": false }]),
        seen: Vec::new(),
    }));
    let (st, d) = (state.clone(), dir.clone());
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let (st, d) = (st.clone(), d.clone());
            std::thread::spawn(move || {
                let mut out = s.try_clone().unwrap();
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let mut g = st.lock().unwrap();
                    g.seen.push(req.clone());
                    let r = reply(&mut g, &d, req["cmd"].as_str().unwrap(), &req["args"]);
                    drop(g);
                    if writeln!(out, "{r}").is_err() {
                        break;
                    }
                }
            });
        }
    });
    Fake { dir, sock, state }
}

impl Fake {
    fn seen(&self, cmd: &str) -> Vec<Value> {
        let g = self.state.lock().unwrap();
        g.seen
            .iter()
            .filter(|r| r["cmd"] == cmd)
            .map(|r| r["args"].clone())
            .collect()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Value>,
    notes: Vec<Value>,
    id: i64,
}

impl Client {
    fn start(f: &Fake) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vivi-mcp"))
            .env("VIVI_SOCKET", &f.sock)
            .env("VIVI_MCP_POLL_MS", "100")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for l in stdout.lines().map_while(Result::ok) {
                if tx.send(serde_json::from_str::<Value>(&l).unwrap()).is_err() {
                    break;
                }
            }
        });
        Client {
            stdin: child.stdin.take(),
            child,
            rx,
            notes: Vec::new(),
            id: 0,
        }
    }
    fn send(&mut self, v: Value) {
        writeln!(self.stdin.as_mut().unwrap(), "{v}").unwrap();
    }
    /// Request and wait for its response; notifications that arrive meanwhile are kept.
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let v = self
                .rx
                .recv_timeout(Duration::from_secs(20))
                .expect("no reply");
            if v["id"] == id {
                return v;
            }
            self.notes.push(v);
        }
    }
    fn call(&mut self, name: &str, args: Value) -> Value {
        self.rpc("tools/call", json!({ "name": name, "arguments": args }))["result"].clone()
    }
    /// Next notification with `method` within `ms`.
    fn note(&mut self, method: &str, ms: u64) -> Option<Value> {
        if let Some(i) = self.notes.iter().position(|n| n["method"] == method) {
            return Some(self.notes.remove(i));
        }
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
            match self.rx.recv_timeout(left) {
                Ok(v) if v["method"] == method => return Some(v),
                Ok(v) => self.notes.push(v),
                Err(_) => break,
            }
        }
        None
    }
    fn init(&mut self, version: &str) -> Value {
        let r = self.rpc(
            "initialize",
            json!({ "protocolVersion": version,
            "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
        );
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        r
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

fn modern() -> Value {
    json!({ "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {} })
}

#[test]
fn legacy_client_lists_tools_plays_and_reads_status() {
    let f = fake("legacy");
    let mut c = Client::start(&f);
    let init = c.init("2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "vivi-mcp");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        init["result"]["capabilities"]["resources"]["subscribe"],
        true
    );
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);

    let tools = c.rpc("tools/list", json!({}));
    let list = tools["result"]["tools"].as_array().unwrap().clone();
    let get = |n: &str| {
        list.iter()
            .find(|t| t["name"] == n)
            .unwrap_or_else(|| panic!("missing tool {n}"))
            .clone()
    };
    for n in [
        "play",
        "pause",
        "toggle_pause",
        "stop",
        "seek",
        "set_volume",
        "set_speed",
        "set_repeat",
        "set_shuffle",
        "open",
        "open_network_stream",
        "search_streams",
        "search_library",
        "list_recent",
        "playlist_list",
        "playlist_add",
        "playlist_play",
        "playlist_move",
        "playlist_remove",
        "playlist_clear",
        "next",
        "previous",
        "list_saved_playlists",
        "open_saved_playlist",
        "save_queue_as_playlist",
        "list_audio_tracks",
        "set_audio_track",
        "list_subtitle_tracks",
        "set_subtitle_track",
        "set_subtitles",
        "set_subtitle_offset",
        "search_subtitles",
        "load_subtitle",
        "sync_subtitles",
        "get_chapters",
        "seek_chapter",
        "list_eq_presets",
        "set_eq_preset",
        "get_lyrics",
        "set_lyrics_offset",
        "set_karaoke",
        "screenshot",
        "list_plugins",
        "plugin_info",
        "list_plugin_commands",
        "run_plugin_command",
        "get_status",
    ] {
        let t = get(n);
        assert!(t["annotations"]["title"].is_string(), "{n} has no title");
        assert_eq!(t["inputSchema"]["type"], "object", "{n}");
    }
    assert_eq!(get("get_status")["annotations"]["readOnlyHint"], true);
    assert!(get("get_status")["outputSchema"]["properties"]["repeat"].is_object());
    assert_eq!(
        get("playlist_clear")["annotations"]["destructiveHint"],
        true
    );
    assert_eq!(get("playlist_add")["annotations"]["destructiveHint"], false);
    assert_eq!(get("search_streams")["annotations"]["openWorldHint"], true);
    // the plugin's palette command became a tool of its own
    let p = get("plugin_offline-downloads_add-current");
    assert_eq!(
        p["annotations"]["title"],
        "Downloads: add the playing stream"
    );

    let found = c.call("search_library", json!({ "query": "song" }));
    let path = found["structuredContent"]["items"][0]["path"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(c.call("open", json!({ "target": path }))["isError"], false);
    let st = c.call("get_status", json!({}));
    assert_eq!(st["structuredContent"]["playing"], true);
    assert_eq!(st["structuredContent"]["path"], "/music/song.flac");

    let subs = c.call("list_subtitle_tracks", json!({}));
    assert!(subs["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("\"sub\""));
    assert_eq!(subs["structuredContent"]["items"][0]["id"], 1);
    assert_eq!(
        c.rpc("tools/call", json!({ "name": "list_audio_tracks" }))["result"]["isError"],
        false
    );
    assert_eq!(f.seen("tracks")[1]["type"], "audio");

    let bad = c.rpc("tools/call", json!({ "name": "nope", "arguments": {} }));
    assert!(bad["error"]["code"].is_number());
    // legacy resource-not-found code
    assert_eq!(
        c.rpc("resources/read", json!({ "uri": "vivi://nope" }))["error"]["code"],
        -32002
    );
    // unknown handshake versions get the newest handshake revision
    let mut c2 = Client::start(&f);
    assert_eq!(
        c2.init("1999-01-01")["result"]["protocolVersion"],
        "2025-11-25"
    );
}

#[test]
fn validation_pagination_and_queue_tools() {
    let f = fake("valid");
    let mut c = Client::start(&f);
    c.init("2025-11-25");
    let r = c.call("set_speed", json!({ "speed": 10 }));
    assert_eq!(r["isError"], true);
    assert!(r["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("between"));
    let r = c.call("set_repeat", json!({ "mode": "twice" }));
    assert_eq!(r["isError"], true);
    let r = c.call("play", json!({ "volume": 3 }));
    assert!(r["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("no arguments"));
    assert!(f.seen("speed").is_empty() && f.seen("repeat").is_empty() && f.seen("play").is_empty());

    let q = c.call("playlist_list", json!({ "offset": 1, "limit": 2 }));
    let s = &q["structuredContent"];
    assert_eq!(s["total"], 5);
    assert_eq!(s["items"].as_array().unwrap().len(), 2);
    assert_eq!(s["items"][0]["index"], 1);
    assert_eq!(s["next_offset"], 3);
    // offset/limit stay in vivi-mcp; library search asks Vivi for enough rows
    assert!(f.seen("playlist_list")[0].get("offset").is_none());
    let lib = c.call(
        "search_library",
        json!({ "query": "x", "offset": 2, "limit": 5 }),
    );
    assert_eq!(
        lib["structuredContent"]["items"][0]["path"],
        "/music/c.flac"
    );
    assert_eq!(f.seen("search")[0]["limit"], 7);

    let r = c.call("list_recent", json!({ "query": "jazz", "kind": "stream" }));
    assert_eq!(r["structuredContent"]["items"][0]["title"], "Jazz");
    assert_eq!(f.seen("recent")[0]["query"], "jazz");
    assert_eq!(
        c.call("playlist_move", json!({ "from": 0, "to": 3 }))["isError"],
        false
    );
    assert_eq!(f.seen("playlist_move")[0]["to"], 3);
    assert_eq!(c.call("playlist_clear", json!({}))["isError"], false);
    assert_eq!(
        c.call("set_repeat", json!({ "mode": "all" }))["isError"],
        false
    );
    assert_eq!(f.seen("repeat")[0]["mode"], "all");
}

#[test]
fn modern_requests_are_stateless() {
    let f = fake("modern");
    let mut c = Client::start(&f);
    let d = c.rpc("server/discover", json!({ "_meta": modern() }));
    let r = &d["result"];
    assert_eq!(r["resultType"], "complete");
    assert!(r["supportedVersions"]
        .as_array()
        .unwrap()
        .contains(&json!("2026-07-28")));
    assert!(r["supportedVersions"]
        .as_array()
        .unwrap()
        .contains(&json!("2025-11-25")));
    assert_eq!(
        r["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "vivi-mcp"
    );
    assert!(r["ttlMs"].is_number() && r["cacheScope"].is_string());

    let bad = c.rpc("tools/list", json!({ "_meta": {
        "io.modelcontextprotocol/protocolVersion": "1900-01-01", "io.modelcontextprotocol/clientCapabilities": {} } }));
    assert_eq!(bad["error"]["code"], -32022);
    assert_eq!(bad["error"]["data"]["requested"], "1900-01-01");
    assert!(bad["error"]["data"]["supported"]
        .as_array()
        .unwrap()
        .contains(&json!("2026-07-28")));

    // no initialize needed
    let t = c.rpc("tools/list", json!({ "_meta": modern() }));
    assert_eq!(t["result"]["resultType"], "complete");
    assert_eq!(t["result"]["cacheScope"], "private");
    assert!(t["result"]["tools"].as_array().unwrap().len() > 40);
    let call = c.rpc(
        "tools/call",
        json!({ "_meta": modern(), "name": "get_status", "arguments": {} }),
    );
    assert_eq!(call["result"]["resultType"], "complete");
    assert_eq!(call["result"]["isError"], false);
    let nf = c.rpc(
        "resources/read",
        json!({ "_meta": modern(), "uri": "vivi://nope" }),
    );
    assert_eq!(nf["error"]["code"], -32602);
    let res = c.rpc("resources/list", json!({ "_meta": modern() }));
    let uris: Vec<&str> = res["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "vivi://status",
            "vivi://playlist",
            "vivi://recent",
            "vivi://lyrics",
            "vivi://plugin-commands"
        ]
    );
    let rd = c.rpc(
        "resources/read",
        json!({ "_meta": modern(), "uri": "vivi://recent" }),
    );
    assert!(rd["result"]["contents"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Jazz"));
    assert_eq!(rd["result"]["ttlMs"], 0);
}

#[test]
fn subscriptions_notify_on_changes() {
    let f = fake("subs");
    // legacy: resources/subscribe
    let mut c = Client::start(&f);
    c.init("2025-11-25");
    assert!(c.rpc("resources/subscribe", json!({ "uri": "vivi://status" }))["result"].is_object());
    std::thread::sleep(Duration::from_millis(400));
    f.state.lock().unwrap().position = 42.0; // position alone is not a change
    assert!(c.note("notifications/resources/updated", 600).is_none());
    f.state.lock().unwrap().playing = "/music/new.flac".into();
    let n = c
        .note("notifications/resources/updated", 3000)
        .expect("status update");
    assert_eq!(n["params"]["uri"], "vivi://status");
    assert!(n["params"].get("_meta").is_none());

    // modern: subscriptions/listen, acknowledged, notifications tagged with its id
    let mut m = Client::start(&f);
    m.send(
        json!({ "jsonrpc": "2.0", "id": "listen-1", "method": "subscriptions/listen", "params": {
        "_meta": modern(), "notifications": { "toolsListChanged": true, "promptsListChanged": true,
        "resourceSubscriptions": ["vivi://playlist", "vivi://unknown"] } } }),
    );
    let ack = m
        .note("notifications/subscriptions/acknowledged", 3000)
        .expect("ack");
    assert_eq!(
        ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        "listen-1"
    );
    assert_eq!(
        ack["params"]["notifications"],
        json!({ "toolsListChanged": true, "resourceSubscriptions": ["vivi://playlist"] })
    );
    std::thread::sleep(Duration::from_millis(400));
    f.state.lock().unwrap().plugin_cmds = json!([]); // a plugin was turned off
    let n = m
        .note("notifications/tools/list_changed", 3000)
        .expect("tools changed");
    assert_eq!(
        n["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        "listen-1"
    );
    // the stream never got a response, and other requests still work meanwhile
    assert_eq!(m.rpc("ping", json!({}))["result"], json!({}));
    let tools = m.rpc("tools/list", json!({ "_meta": modern() }));
    assert!(!tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"].as_str().unwrap().starts_with("plugin_offline")));
}

#[test]
fn screenshot_returns_the_image() {
    let f = fake("shot");
    let mut c = Client::start(&f);
    c.init("2025-11-25");
    let r = c.call("screenshot", json!({}));
    assert_eq!(r["isError"], false);
    let img = r["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["type"] == "image")
        .expect("image content");
    assert_eq!(img["mimeType"], "image/png");
    assert_eq!(img["data"], "iVBORw0KGgpmYWtl");
    let saved = PathBuf::from(r["structuredContent"]["path"].as_str().unwrap());
    assert!(saved.exists());

    let r = c.call("screenshot", json!({ "save": false, "mode": "video" }));
    let a = f.seen("screenshot");
    let tmp = PathBuf::from(a[1]["path"].as_str().unwrap());
    assert!(
        tmp.extension().unwrap() == "jpg" && a[1].get("save").is_none() && a[1]["mode"] == "video"
    );
    assert!(r["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["type"] == "image" && x["mimeType"] == "image/jpeg"));
    assert!(!tmp.exists(), "temporary screenshot is deleted");
    assert_eq!(r["structuredContent"]["saved"], false);
}

#[test]
fn plugin_commands_and_prompts() {
    let f = fake("plug");
    let mut c = Client::start(&f);
    c.init("2025-11-25");
    let r = c.call("plugin_offline-downloads_add-current", json!({}));
    assert_eq!(r["isError"], false);
    let a = &f.seen("run_plugin_command")[0];
    assert_eq!(
        (a["plugin"].as_str(), a["command"].as_str()),
        (Some("offline-downloads"), Some("add-current"))
    );
    let r = c.call("plugin_offline-downloads_add-current", json!({ "evil": 1 }));
    assert_eq!(r["isError"], true);
    let l = c.call("list_plugin_commands", json!({}));
    assert_eq!(l["structuredContent"]["total"], 1);

    let p = c.rpc("prompts/list", json!({}));
    let names: Vec<&str> = p["result"]["prompts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"play_radio") && names.contains(&"fix_subtitles"));
    let g = c.rpc(
        "prompts/get",
        json!({ "name": "play_radio", "arguments": { "query": "lofi" } }),
    );
    assert!(g["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .contains("lofi"));
    assert_eq!(
        c.rpc("prompts/get", json!({ "name": "play_radio" }))["error"]["code"],
        -32602
    );
}
