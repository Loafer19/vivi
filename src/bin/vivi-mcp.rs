//! `vivi-mcp`: Model Context Protocol server (stdio transport) that lets AI agents control
//! a running Vivi over its local control socket (see `src/ipc.rs`, issue #68).
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout; logs go to stderr. Uses only
//! `std` and `serde_json`, so it stays tiny and does not link the player or the UI.
//!
//! Dual-era (see the MCP versioning page): a client that opens with `initialize` gets the
//! handshake protocol it asked for (2025-11-25 and older); a request whose `_meta` carries
//! `io.modelcontextprotocol/protocolVersion` is served statelessly as 2026-07-28
//! (`server/discover`, `resultType`, cache hints, `subscriptions/listen`). Change
//! notifications come from a poller thread that compares what Vivi reports once a second.

use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Current (stateless) revision.
const MODERN: &str = "2026-07-28";
/// Handshake revisions answered on `initialize`, newest first.
const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SUB: &str = "io.modelcontextprotocol/subscriptionId";
/// Largest screenshot returned inline as image content.
const MAX_IMAGE: u64 = 4 * 1024 * 1024;
const INSTRUCTIONS: &str = "Controls the Vivi media player. Use get_status to see what is playing (subscribe to vivi://status for changes). Find media with search_library or list_recent, then open or playlist_add with a returned path. For radio and TV use search_streams, then open_network_stream with a returned URL. Plugins add their own commands: list_plugin_commands / run_plugin_command (they are also listed as plugin_* tools). open replaces the queue; ask the user before clearing or removing queue entries.";

fn supported() -> Vec<&'static str> {
    std::iter::once(MODERN)
        .chain(LEGACY.iter().copied())
        .collect()
}

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

fn call_vivi(cmd: &str, args: Value) -> Result<Value, String> {
    call_vivi_t(cmd, args, Duration::from_secs(30))
}

/// Send one request to Vivi and return its result.
#[cfg(unix)]
fn call_vivi_t(cmd: &str, args: Value, timeout: Duration) -> Result<Value, String> {
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
    s.set_read_timeout(Some(timeout)).ok();
    writeln!(s, "{}", json!({ "cmd": cmd, "args": args })).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s)
        .read_line(&mut line)
        .map_err(|e| format!("Vivi did not answer: {e}"))?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply from Vivi: {e}"))?;
    if v["ok"].as_bool() == Some(true) {
        Ok(v["result"].clone())
    } else {
        Err(v["error"].as_str().unwrap_or("unknown error").to_string())
    }
}

#[cfg(not(unix))]
fn call_vivi_t(_: &str, _: Value, _: Duration) -> Result<Value, String> {
    Err("vivi-mcp currently supports Linux/macOS only (Unix socket)".into())
}

// ---------------------------------------------------------------------------------------
// Tools

/// How a tool's result is post-processed.
#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// Pass the result through (objects become structuredContent).
    Plain,
    /// A list: `offset` / `limit` apply here, the result is `{total, offset, items, next_offset}`.
    Paged,
}

#[derive(Clone)]
struct Tool {
    name: String,
    title: String,
    desc: String,
    input: Value,
    cmd: &'static str,
    /// readOnly, destructive, idempotent, openWorld.
    hints: (bool, bool, bool, bool),
    output: Option<Value>,
    shape: Shape,
    /// Fixed arguments merged into every call (track type, plugin id, …).
    fixed: Value,
}

fn tool(
    name: &str,
    title: &str,
    cmd: &'static str,
    desc: &str,
    props: Value,
    req: &[&str],
) -> Tool {
    Tool {
        name: name.into(),
        title: title.into(),
        desc: desc.into(),
        input: json!({ "type": "object", "properties": props, "required": req, "additionalProperties": false }),
        cmd,
        hints: (false, false, true, false),
        output: None,
        shape: Shape::Plain,
        fixed: json!({}),
    }
}

impl Tool {
    /// Only reads.
    fn ro(mut self) -> Self {
        self.hints = (true, false, true, self.hints.3);
        self
    }
    /// May remove or replace something the user has.
    fn destructive(mut self) -> Self {
        self.hints.1 = true;
        self
    }
    /// Repeating it changes things again (next, toggle, append).
    fn repeats(mut self) -> Self {
        self.hints.2 = false;
        self
    }
    /// Talks to the Internet or runs third-party plugin code.
    fn web(mut self) -> Self {
        self.hints.3 = true;
        self
    }
    fn out(mut self, schema: Value) -> Self {
        self.output = Some(schema);
        self
    }
    fn fixed(mut self, v: Value) -> Self {
        self.fixed = v;
        self
    }
    /// List result with `offset` / `limit`.
    fn paged(mut self, what: &str) -> Self {
        self.shape = Shape::Paged;
        let p = self.input["properties"].as_object_mut().unwrap();
        p.insert("offset".into(), json!({ "type": "integer", "minimum": 0, "default": 0, "description": "Skip this many entries (pagination)" }));
        p.insert("limit".into(), json!({ "type": "integer", "minimum": 1, "maximum": 500, "default": 50, "description": "Entries per page" }));
        self.output = Some(
            json!({ "type": "object", "required": ["total", "offset", "items"], "properties": {
            "total": { "type": "integer", "description": format!("All matching {what}") },
            "offset": { "type": "integer" },
            "items": { "type": "array", "items": { "type": "object" } },
            "next_offset": { "type": "integer", "description": "Pass as offset for the next page; absent on the last page" } } }),
        );
        self
    }

    fn json(&self) -> Value {
        let (ro, de, idem, ow) = self.hints;
        let mut ann = json!({ "title": self.title, "readOnlyHint": ro, "openWorldHint": ow });
        if !ro {
            ann["destructiveHint"] = json!(de);
            ann["idempotentHint"] = json!(idem);
        }
        let mut t = json!({ "name": self.name, "title": self.title, "description": self.desc,
            "inputSchema": self.input, "annotations": ann });
        if let Some(o) = &self.output {
            t["outputSchema"] = o.clone();
        }
        t
    }
}

fn status_schema() -> Value {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let n = |d: &str| json!({ "type": "number", "description": d });
    let b = |d: &str| json!({ "type": "boolean", "description": d });
    json!({ "type": "object", "properties": {
        "has_media": b("Something is loaded"), "playing": b("Loaded and not paused"),
        "path": s("File path or URL"), "title": s("Title shown in the player"),
        "artist": s(""), "album": s(""),
        "position": n("Seconds"), "duration": n("Seconds; 0 for live streams"),
        "volume": n("Percent"), "muted": b(""), "speed": n("1 = normal"),
        "subtitles": b("Subtitles visible"), "subtitle_track": s("mpv sid"),
        "subtitle_offset": n("Seconds, positive = later"),
        "audio_track": s("mpv aid"),
        "playlist_pos": { "type": "integer", "description": "Queue index, -1 = none" },
        "playlist_count": { "type": "integer" },
        "eq_preset": s("Equalizer preset or Off"),
        "is_stream": b("Network URL"), "has_video": b("Has a video track"),
        "chapter": { "type": "integer", "description": "Current chapter index, -1 = none" },
        "chapter_title": s(""),
        "repeat": { "type": "string", "enum": ["off", "one", "all"] },
        "shuffle": b(""), "lyrics_available": b("The Lyrics plugin has lines for this track"),
    } })
}

fn tools() -> Vec<Tool> {
    let none = json!({});
    let target =
        |d: &str| json!({ "target": { "type": "string", "minLength": 1, "description": d } });
    let track_id = json!({ "id": { "type": ["integer", "string"], "description": "Track id from the list, \"auto\" or \"no\"" } });
    vec![
        // Now playing and transport
        tool("get_status", "Now playing", "status", "What is playing: title, artist, album, path, position and duration (s), volume, mute, speed, playing state, subtitles and their offset, audio/subtitle track ids, queue position, chapter, repeat and shuffle, EQ preset, whether lyrics are available.", none.clone(), &[]).ro().out(status_schema()),
        tool("play", "Play", "play", "Resume playback.", none.clone(), &[]),
        tool("pause", "Pause", "pause", "Pause playback.", none.clone(), &[]),
        tool("toggle_pause", "Play / pause", "toggle", "Toggle between playing and paused.", none.clone(), &[]).repeats(),
        tool("stop", "Stop", "stop", "Stop: pause and rewind to the start (the queue stays).", none.clone(), &[]),
        tool("seek", "Seek", "seek", "Seek to an absolute position or by a relative offset, in seconds. Give exactly one of position or offset.",
            json!({ "position": { "type": "number", "minimum": 0, "description": "Absolute position in seconds" },
                    "offset": { "type": "number", "description": "Relative offset in seconds (negative = back)" } }), &[]).repeats(),
        tool("set_volume", "Set volume", "volume", "Set the volume in percent (0-100, up to the maximum from Settings) and/or mute.",
            json!({ "level": { "type": "number", "minimum": 0, "maximum": 1000 }, "muted": { "type": "boolean" } }), &[]),
        tool("set_speed", "Set speed", "speed", "Set the playback speed (0.1-4, 1 = normal).",
            json!({ "speed": { "type": "number", "minimum": 0.1, "maximum": 4 } }), &["speed"]),
        tool("set_repeat", "Repeat mode", "repeat", "Repeat: off, one (the current item) or all (the whole queue).",
            json!({ "mode": { "type": "string", "enum": ["off", "one", "all"] } }), &["mode"]),
        tool("set_shuffle", "Shuffle", "shuffle", "Shuffle the queue (enabled: true) or restore its order (false).",
            json!({ "enabled": { "type": "boolean" } }), &["enabled"]),
        // Opening things
        tool("open", "Open", "open", "Play a local file path or a URL now (http(s), YouTube and other yt-dlp sites, .m3u playlists). Replaces the current queue; use playlist_add to keep it.",
            target("File path or URL"), &["target"]).destructive().repeats(),
        tool("open_network_stream", "Open network stream", "open_stream", "Play a network stream URL now (Internet radio, IPTV .m3u8/HLS, DASH, RTMP/RTSP/UDP, Icecast/SHOUTcast), or append it to the queue with queue: true. Use search_streams to find stations and TV channels.",
            json!({ "target": { "type": "string", "minLength": 1, "description": "Stream URL" },
                    "name": { "type": "string", "description": "Display name while it loads (optional)" },
                    "queue": { "type": "boolean", "description": "Append to the queue instead of playing now" } }), &["target"]).destructive().repeats(),
        tool("search_streams", "Search radio and TV", "search_streams", "Search Internet radio (radio-browser.info, SomaFM), iptv-org TV channels, your Network Streams favorites and your own playlists by name; kind radio or tv limits it (each item has its kind). Returns stream URLs for open_network_stream. Needs the network and the Network Streams plugin; may take a few seconds.",
            json!({ "query": { "type": "string", "minLength": 1 }, "kind": { "type": "string", "enum": ["radio", "tv"] },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200 } }), &["query"]).ro().web(),
        tool("search_library", "Search library", "search", "Search the scanned media library by words in title, artist, album, genre or path. Returns paths usable with open / playlist_add.",
            json!({ "query": { "type": "string" }, "kind": { "type": "string", "enum": ["audio", "video"] } }), &["query"]).ro().paged("library items"),
        tool("list_recent", "Recent", "recent", "The Recent list (Home): everything opened lately from any source (files, folders, playlists, streams, radio, web video, Free Music), newest first. Filter by words or kind; targets work with open.",
            json!({ "query": { "type": "string", "description": "Words in title, subtitle or target" },
                    "kind": { "type": "string", "enum": ["video", "audio", "folder", "playlist", "stream", "web", "music"] } }), &[]).ro().paged("recent entries"),
        // Queue
        tool("playlist_list", "Queue", "playlist_list", "The play queue: index, path, title, current flag.", none.clone(), &[]).ro().paged("queue entries"),
        tool("playlist_add", "Add to queue", "playlist_add", "Append a file path or URL to the play queue (starts playing if idle).", target("File path or URL"), &["target"]).repeats(),
        tool("playlist_play", "Play queue entry", "playlist_play", "Play the queue entry at an index (0-based, see playlist_list).",
            json!({ "index": { "type": "integer", "minimum": 0 } }), &["index"]),
        tool("playlist_move", "Move queue entry", "playlist_move", "Move a queue entry to another position (both 0-based).",
            json!({ "from": { "type": "integer", "minimum": 0 }, "to": { "type": "integer", "minimum": 0 } }), &["from", "to"]).repeats(),
        tool("playlist_remove", "Remove from queue", "playlist_remove", "Remove one queue entry by index (0-based). Removing the playing entry stops it.",
            json!({ "index": { "type": "integer", "minimum": 0 } }), &["index"]).destructive().repeats(),
        tool("playlist_clear", "Clear queue", "playlist_clear", "Remove every queue entry except the playing one (keep_current: false also stops playback and empties the queue). Ask the user first.",
            json!({ "keep_current": { "type": "boolean", "default": true } }), &[]).destructive(),
        tool("next", "Next", "next", "Next item in the queue.", none.clone(), &[]).repeats(),
        tool("previous", "Previous", "prev", "Previous item in the queue.", none.clone(), &[]).repeats(),
        tool("list_saved_playlists", "Saved playlists", "saved_playlists", "Playlists saved in Library → Playlists: name, path, number of entries.", none.clone(), &[]).ro().paged("playlists"),
        tool("open_saved_playlist", "Open saved playlist", "open_saved_playlist", "Play a saved playlist by name (replaces the queue), or append it with queue: true.",
            json!({ "name": { "type": "string", "minLength": 1 }, "queue": { "type": "boolean" } }), &["name"]).destructive().repeats(),
        tool("save_queue_as_playlist", "Save queue as playlist", "save_queue", "Save the current queue as a playlist in Library → Playlists. Fails if the name exists unless overwrite: true (ask the user first).",
            json!({ "name": { "type": "string", "minLength": 1 }, "overwrite": { "type": "boolean" } }), &["name"]).destructive(),
        // Tracks, subtitles, chapters
        tool("list_audio_tracks", "Audio tracks", "tracks", "Audio tracks of the current item: id, title with language, selected.", none.clone(), &[]).ro().fixed(json!({ "type": "audio" })),
        tool("set_audio_track", "Select audio track", "set_track", "Select an audio track by id (see list_audio_tracks), \"auto\" or \"no\".", track_id.clone(), &["id"]).fixed(json!({ "type": "audio" })),
        tool("list_subtitle_tracks", "Subtitle tracks", "tracks", "Subtitle tracks of the current item (embedded and external): id, title with language, selected.", none.clone(), &[]).ro().fixed(json!({ "type": "sub" })),
        tool("set_subtitle_track", "Select subtitles", "set_track", "Select a subtitle track by id (see list_subtitle_tracks), \"auto\", or \"no\" to turn subtitles off.", track_id, &["id"]).fixed(json!({ "type": "sub" })),
        tool("set_subtitles", "Show / hide subtitles", "subtitles", "Show or hide subtitles.", json!({ "enabled": { "type": "boolean" } }), &["enabled"]),
        tool("set_subtitle_offset", "Subtitle timing", "sub_offset", "Shift subtitles in time for the playing file (positive = later); saved per file and restored when it plays again. Give seconds (absolute), delta, or reset: true.",
            json!({ "seconds": { "type": "number", "description": "Absolute offset in seconds" },
                    "delta": { "type": "number", "description": "Change relative to the current offset" },
                    "reset": { "type": "boolean" } }), &[]),
        tool("search_subtitles", "Search subtitles online", "search_subtitles", "Search OpenSubtitles for the playing file: by its file hash first (exact matches, hash_match=true), then by name or IMDb id. Returns file_id, language, downloads, rating, hearing_impaired, fps, trusted, machine_translated. Needs the API key in Settings → Extensions → Subtitle Search.",
            json!({ "query": { "type": "string", "description": "Title, episode or IMDb id (tt0133093); default: from the file name" },
                    "languages": { "type": "string", "description": "Comma-separated codes, e.g. \"en,uk\"; default: the plugin setting" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }), &[]).ro().web(),
        tool("load_subtitle", "Load subtitle", "load_subtitle", "Download a subtitle found with search_subtitles and show it (saved next to the video when that setting is on; remembered for this file). Uses one OpenSubtitles download from the daily quota.",
            json!({ "file_id": { "type": "integer" }, "language": { "type": "string", "description": "Language code from the result (names the saved file)" } }), &["file_id"]).web(),
        tool("sync_subtitles", "Auto-sync subtitles", "sub_sync", "Align the current external subtitle to the audio with the Subtitle Auto-Sync plugin (alass / ffsubsync), in the background.", none.clone(), &[]),
        tool("get_chapters", "Chapters", "chapters", "Chapters of the current item (index, title, start in seconds) and the current chapter.", none.clone(), &[]).ro(),
        tool("seek_chapter", "Go to chapter", "seek_chapter", "Jump to a chapter by index (0-based), or by offset (+1 next, -1 previous).",
            json!({ "index": { "type": "integer", "minimum": 0 }, "offset": { "type": "integer" } }), &[]).repeats(),
        // Bookmarks and A-B loop
        tool("list_bookmarks", "Bookmarks", "bookmarks", "Bookmarks of the playing file, sorted by time: time (s), label, note, thumb (saved frame file name), created (Unix time). Indexes are 0-based and used by jump_to_bookmark / remove_bookmark.", none.clone(), &[]).ro(),
        tool("add_bookmark", "Add bookmark", "bookmark_add", "Bookmark the playing position (like Ctrl+B), optionally with a label and note. Returns added (false when one exists within half a second), its index and the list.",
            json!({ "label": { "type": "string", "description": "Name (default: \"Bookmark at m:ss\")" }, "note": { "type": "string" } }), &[]).repeats(),
        tool("remove_bookmark", "Delete bookmark", "bookmark_remove", "Delete one bookmark of the playing file by index (see list_bookmarks), with its saved frame. Returns the remaining list.",
            json!({ "index": { "type": "integer", "minimum": 0 } }), &["index"]).destructive().repeats(),
        tool("jump_to_bookmark", "Go to bookmark", "bookmark_jump", "Seek to a bookmark of the playing file by index (see list_bookmarks).",
            json!({ "index": { "type": "integer", "minimum": 0 } }), &["index"]),
        tool("export_bookmarks_as_chapters", "Bookmarks to chapters", "bookmarks_export", "Turn the playing file's bookmarks into chapters: to player (default) adds them to the chapter list until the file closes; to file writes an FFmpeg metadata chapters file with the file's chapters and bookmarks (path, default <video>.ffmetadata next to a local video; an existing file is replaced). Usable with mpv --chapters-file or ffmpeg -i video -i file -map_metadata 1 -codec copy.",
            json!({ "to": { "type": "string", "enum": ["player", "file"], "default": "player" },
                    "path": { "type": "string", "description": "Output file for to: file (absolute)" } }), &[]),
        tool("get_ab_loop", "A-B loop", "ab_loop", "The A-B loop: points a and b in seconds (null = unset), active (both set) and the panel's state text.", none.clone(), &[]).ro(),
        tool("set_ab_loop", "Set A-B loop", "ab_loop", "Set the A-B loop: set \"a\" or \"b\" puts that point at the playhead; or give a / b in seconds (null clears that point; a after b swaps them); or clear: true. The seek bar, side panel and top-bar chip follow. Returns the new state.",
            json!({ "set": { "type": "string", "enum": ["a", "b"] },
                    "a": { "type": ["number", "null"], "minimum": 0 }, "b": { "type": ["number", "null"], "minimum": 0 },
                    "clear": { "type": "boolean" } }), &[]),
        // Sleep timer
        tool("get_sleep_timer", "Sleep timer", "sleep", "The sleep timer: mode (off, min:<n>, eof, chapter, items:<n>), active, remaining seconds (minutes and chapter modes), items_left, fade (seconds of volume fade-out before it pauses).", none.clone(), &[]).ro(),
        tool("set_sleep_timer", "Set sleep timer", "sleep", "Pause playback later: minutes (1-1440), or timer = min:<n>, eof (end of the file), chapter (end of the chapter), items:<n> (after n queue items, counting the current one) or off. The volume fades out over the last seconds first. Returns the state.",
            json!({ "minutes": { "type": "integer", "minimum": 1, "maximum": 1440 },
                    "timer": { "type": "string", "minLength": 2, "description": "off, min:<n>, eof, chapter or items:<n>" } }), &[]),
        tool("cancel_sleep_timer", "Cancel sleep timer", "sleep", "Turn the sleep timer off (the volume is put back if it was fading). Returns the state.", none.clone(), &[]).fixed(json!({ "timer": "off" })),
        // Audio
        tool("list_eq_presets", "Equalizer presets", "eq_presets", "Equalizer presets (built-in and custom), the current one and whether the equalizer is on.", none.clone(), &[]).ro(),
        tool("set_eq_preset", "Select equalizer preset", "eq_preset", "Select an equalizer preset by name (see list_eq_presets), or \"Off\" to turn the equalizer off.",
            json!({ "name": { "type": "string", "minLength": 1 } }), &["name"]),
        // Lyrics
        tool("get_lyrics", "Lyrics", "lyrics", "Lyrics shown for the current track (Lyrics plugin): lines, synced flag, current line index, source, timing offset in seconds, whether the karaoke view is open. Empty when none are loaded.", none.clone(), &[]).ro(),
        tool("set_lyrics_offset", "Lyrics timing", "lyrics_offset", "Shift synced lyrics in time (positive = later, ±30 s, 0.1 s steps); saved per track. Give seconds (absolute), delta, or reset: true.",
            json!({ "seconds": { "type": "number", "minimum": -30, "maximum": 30 }, "delta": { "type": "number" }, "reset": { "type": "boolean" } }), &[]),
        tool("set_karaoke", "Karaoke view", "karaoke", "Open (enabled: true) or close the full-screen karaoke lyrics view.",
            json!({ "enabled": { "type": "boolean" } }), &["enabled"]),
        tool("set_visualizer", "Music visualizer", "visualizer", "Music visualizer mode for music and radio without video (Music Visualizer plugin): give mode as a name (Disabled, Spectrometer, Scope, Spectrum, Waveform, Radial, Particles, Pitch), next or prev; without mode it only reports. Returns the mode, all modes and whether it is showing now. The choice is remembered.",
            json!({ "mode": { "type": "string", "minLength": 1 } }), &[]),
        // Screenshot
        tool("screenshot", "Screenshot", "screenshot", "Capture the current video frame. Returns the image (so you can see it) and the saved file path. save: false returns the image without keeping a file. Default folder: Settings → Video → screenshot folder. Errors on audio-only items.",
            json!({ "mode": { "type": "string", "enum": ["subtitles", "video", "window"], "description": "subtitles (default): video + subtitles; video: video only; window: as shown" },
                    "path": { "type": "string", "description": "Absolute output file (.png / .jpg / .webp); optional; must not exist unless overwrite" },
                    "overwrite": { "type": "boolean" },
                    "save": { "type": "boolean", "default": true, "description": "Keep the file (default) or return the image only" },
                    "image": { "type": "boolean", "default": true, "description": "Return the image content (up to 4 MB)" } }), &[]),
        // Plugins
        tool("list_plugins", "Plugins", "plugins", "Installed plugins: id, name, version, kind, enabled.", none.clone(), &[]).ro(),
        tool("plugin_info", "Plugin details", "plugin_info", "Details of one plugin: description, version, author, settings (keys and titles, no values).",
            json!({ "id": { "type": "string", "minLength": 1 } }), &["id"]).ro(),
        tool("list_plugin_commands", "Plugin commands", "plugin_commands", "Commands that enabled plugins add to Vivi's menus, command palette, tray and control bar: plugin, command, label, where it shows up, takes_target (acts on a library item path). Run them with run_plugin_command; each is also a plugin_* tool.", none.clone(), &[]).ro().paged("plugin commands"),
        tool("run_plugin_command", "Run plugin command", "run_plugin_command", "Run a command a plugin contributes (see list_plugin_commands), as if chosen from its menu. Plugin code decides what happens, so confirm with the user when it is not obviously harmless.",
            json!({ "plugin": { "type": "string", "minLength": 1 }, "command": { "type": "string" },
                    "target": { "type": "string", "description": "Library item path, for commands with takes_target" } }), &["plugin", "command"]).destructive().repeats().web(),
    ]
}

/// Tool name for a plugin contribution: `plugin_<id>_<command>` in `[a-z0-9_-]`, ≤ 64 chars.
fn plugin_tool_name(id: &str, cmd: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect()
    };
    let cmd = if cmd.is_empty() {
        "open".to_string()
    } else {
        clean(cmd)
    };
    let mut n = format!("plugin_{}_{}", clean(id), cmd);
    n.truncate(64);
    n
}

/// Plugin contributions as tools (empty when Vivi is not reachable).
fn plugin_tools(statics: &[Tool]) -> Vec<Tool> {
    let Ok(Value::Array(list)) = call_vivi_t("plugin_commands", json!({}), Duration::from_secs(3))
    else {
        return Vec::new();
    };
    let mut out: Vec<Tool> = Vec::new();
    for e in list {
        let (id, cmd) = (
            e["plugin"].as_str().unwrap_or(""),
            e["command"].as_str().unwrap_or(""),
        );
        let mut name = plugin_tool_name(id, cmd);
        let mut i = 2;
        while statics.iter().chain(out.iter()).any(|t| t.name == name) {
            name = format!(
                "{}_{i}",
                &plugin_tool_name(id, cmd)[..plugin_tool_name(id, cmd).len().min(60)]
            );
            i += 1;
        }
        let label = e["label"].as_str().unwrap_or(cmd);
        let props = if e["takes_target"] == true {
            json!({ "target": { "type": "string", "description": "Library item path (needed when it is only an item action)" } })
        } else {
            json!({})
        };
        let desc = format!(
            "{label}. Command \"{cmd}\" of the {} plugin ({}), shown in {}. Plugin code decides what happens.",
            e["plugin_name"].as_str().unwrap_or(id), id, e["where"].as_str().unwrap_or("Vivi")
        );
        out.push(
            tool(&name, label, "run_plugin_command", &desc, props, &[])
                .destructive()
                .repeats()
                .web()
                .fixed(json!({ "plugin": id, "command": cmd })),
        );
    }
    out
}

/// Check `args` against the subset of JSON Schema used above.
fn validate(schema: &Value, args: &Value) -> Result<(), String> {
    let Some(o) = args.as_object() else {
        return Err("arguments must be an object".into());
    };
    let props = schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    for k in o.keys() {
        if !props.contains_key(k) {
            let known: Vec<&String> = props.keys().collect();
            return Err(if known.is_empty() {
                format!("unknown argument {k:?}: this tool takes no arguments")
            } else {
                format!("unknown argument {k:?}; expected one of {known:?}")
            });
        }
    }
    for r in schema["required"].as_array().into_iter().flatten() {
        let r = r.as_str().unwrap_or("");
        if o.get(r).is_none_or(Value::is_null) {
            return Err(format!("missing required argument {r:?}"));
        }
    }
    for (k, v) in o {
        let s = &props[k];
        let types: Vec<&str> = match &s["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let fits = |t: &str| match t {
            "string" => v.is_string(),
            "number" => v.is_number(),
            "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
            "boolean" => v.is_boolean(),
            "array" => v.is_array(),
            "object" => v.is_object(),
            "null" => v.is_null(),
            _ => true,
        };
        if !types.is_empty() && !types.iter().any(|t| fits(t)) {
            return Err(format!("{k:?} must be {}", types.join(" or ")));
        }
        if let Some(e) = s["enum"].as_array() {
            if !e.contains(v) {
                return Err(format!("{k:?} must be one of {}", Value::Array(e.clone())));
            }
        }
        if let Some(n) = v.as_f64() {
            if s["minimum"].as_f64().is_some_and(|m| n < m)
                || s["maximum"].as_f64().is_some_and(|m| n > m)
            {
                return Err(format!(
                    "{k:?} must be between {} and {}",
                    s["minimum"].as_f64().map_or("-∞".into(), |m| m.to_string()),
                    s["maximum"].as_f64().map_or("∞".into(), |m| m.to_string())
                ));
            }
        }
        if let (Some(t), Some(m)) = (v.as_str(), s["minLength"].as_u64()) {
            if (t.trim().chars().count() as u64) < m {
                return Err(format!("{k:?} must not be empty"));
            }
        }
    }
    Ok(())
}

fn text_result(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => serde_json::to_string_pretty(v).unwrap_or_default(),
    }
}

fn tool_error(msg: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": msg }], "isError": true })
}

fn ok_result(v: Value) -> Value {
    let mut r =
        json!({ "content": [{ "type": "text", "text": text_result(&v) }], "isError": false });
    if v.is_object() {
        r["structuredContent"] = v;
    } else if v.is_array() {
        r["structuredContent"] = json!({ "items": v });
    }
    r
}

/// `{total, offset, items, next_offset}` page of a list result.
fn page(v: Value, offset: usize, limit: usize) -> Value {
    let all = match v {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("items") {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let total = all.len();
    let items: Vec<Value> = all.into_iter().skip(offset).take(limit).collect();
    let mut r = json!({ "total": total, "offset": offset, "items": items });
    if offset + limit < total {
        r["next_offset"] = json!(offset + limit);
    }
    r
}

static SHOT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn screenshot(args: Value) -> Value {
    let save = args["save"].as_bool() != Some(false);
    let want_image = args["image"].as_bool() != Some(false) || !save;
    let mut a = args.clone();
    if let Some(o) = a.as_object_mut() {
        o.remove("save");
        o.remove("image");
        if !save {
            let n = SHOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("vivi-mcp-{}-{n}.jpg", std::process::id()));
            o.insert("path".into(), json!(p.to_string_lossy()));
            o.remove("overwrite");
        }
    }
    let v = match call_vivi("screenshot", a) {
        Ok(v) => v,
        Err(e) => return tool_error(&e),
    };
    let path = PathBuf::from(v["path"].as_str().unwrap_or(""));
    let mut content = Vec::new();
    if want_image {
        let mime = match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .as_deref()
        {
            Some("png") => Some("image/png"),
            Some("jpg" | "jpeg") => Some("image/jpeg"),
            Some("webp") => Some("image/webp"),
            _ => None,
        };
        // mpv writes the file right after the command returns: wait for it briefly.
        let mut bytes = None;
        for _ in 0..50 {
            if let Ok(m) = std::fs::metadata(&path) {
                if m.len() > MAX_IMAGE {
                    content.push(json!({ "type": "text", "text": "image larger than 4 MB: not attached (use mode, or a .jpg path)" }));
                    break;
                }
                if m.len() > 0 {
                    std::thread::sleep(Duration::from_millis(50));
                    bytes = std::fs::read(&path).ok();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if let (Some(b), Some(m)) = (bytes, mime) {
            content.push(json!({ "type": "image", "data": base64(&b), "mimeType": m }));
        }
    }
    let mut out = json!({ "saved": save });
    if save {
        out["path"] = json!(path.to_string_lossy());
        content.insert(
            0,
            json!({ "type": "text", "text": format!("Saved {}", path.display()) }),
        );
    } else {
        let _ = std::fs::remove_file(&path);
        content.insert(
            0,
            json!({ "type": "text", "text": "Screenshot (not saved)" }),
        );
    }
    json!({ "content": content, "structuredContent": out, "isError": false })
}

fn call_tool(t: &Tool, args: Value) -> Value {
    if let Err(e) = validate(&t.input, &args) {
        return tool_error(&format!("invalid arguments for {}: {e}", t.name));
    }
    let mut args = args;
    let (mut offset, mut limit) = (0usize, 50usize);
    if let Some(o) = args.as_object_mut() {
        if let Some(f) = t.fixed.as_object() {
            for (k, v) in f {
                o.insert(k.clone(), v.clone());
            }
        }
        if t.shape == Shape::Paged {
            offset = o.remove("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            limit = o
                .remove("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .clamp(1, 500) as usize;
            if t.cmd == "search" {
                o.insert("limit".into(), json!((offset + limit).min(500)));
            }
        }
    }
    if t.cmd == "screenshot" {
        return screenshot(args);
    }
    match call_vivi(t.cmd, args) {
        Ok(v) if t.shape == Shape::Paged => ok_result(page(v, offset, limit)),
        Ok(v) => ok_result(v),
        Err(e) => tool_error(&e),
    }
}

// ---------------------------------------------------------------------------------------
// Resources and prompts

/// (uri, name, description, vivi command)
const RESOURCES: &[(&str, &str, &str, &str)] = &[
    ("vivi://status", "Now playing", "Player state (get_status without the position, which changes every second, triggering no update)", "status"),
    ("vivi://playlist", "Play queue", "Queue entries: index, path, title, current", "playlist_list"),
    ("vivi://recent", "Recent", "The Recent list from Home, newest first", "recent"),
    ("vivi://lyrics", "Lyrics", "Lyrics of the current track (Lyrics plugin)", "lyrics"),
    ("vivi://plugin-commands", "Plugin commands", "Commands plugins add to menus, palette, tray and control bar", "plugin_commands"),
];

/// What a subscription compares: the resource minus fields that tick constantly.
fn fingerprint(uri: &str) -> String {
    let Some(r) = RESOURCES.iter().find(|r| r.0 == uri) else {
        return String::new();
    };
    match call_vivi_t(r.3, json!({}), Duration::from_secs(5)) {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.remove("position");
                o.remove("current_line");
            }
            v.to_string()
        }
        Err(_) => "unreachable".into(),
    }
}

/// (argument name, description, required)
type PromptArg = (&'static str, &'static str, bool);
/// (name, description, arguments, message template with `{arg}` slots)
type Prompt = (&'static str, &'static str, Vec<PromptArg>, &'static str);

fn prompts() -> Vec<Prompt> {
    vec![
        ("now_playing", "Summarize what Vivi is playing and how much is left", vec![],
            "Use get_status to see what Vivi is playing. Tell me the title and artist, where it is (position / duration, time left), and anything notable (subtitles, audio track, chapter, queue position)."),
        ("play_radio", "Find and play an Internet radio station", vec![("query", "Genre, station name, city or country", true)],
            "Find an Internet radio station for \"{query}\" with search_streams (kind: radio), pick the best match (prefer higher bitrate), play it with open_network_stream using its name, then confirm with get_status."),
        ("queue_from_library", "Build a queue from the library", vec![("query", "Artist, album, genre or words", true), ("count", "How many items (default 20)", false)],
            "Search my library for \"{query}\" with search_library, add up to {count} results to the queue with playlist_add (do not use open, it would replace the queue), then show the queue with playlist_list."),
        ("fix_subtitles", "Get working subtitles for the current video", vec![("language", "Language code, e.g. en", false)],
            "Make subtitles work for the current video in language \"{language}\". Check list_subtitle_tracks first and select a matching track with set_subtitle_track. If there is none, use search_subtitles, prefer hash_match results, and load the best one with load_subtitle. Then ask me whether the timing is right; adjust it with set_subtitle_offset or offer sync_subtitles."),
    ]
}

fn get_prompt(params: &Value) -> Result<Value, (i64, String)> {
    let name = params["name"].as_str().unwrap_or("");
    let (_, desc, args, text) = prompts()
        .into_iter()
        .find(|p| p.0 == name)
        .ok_or((-32602, format!("unknown prompt {name:?}")))?;
    let mut t = text.to_string();
    for (a, _, req) in args {
        let v = params["arguments"][a]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let v = match (v, a) {
            (Some(v), _) => v.to_string(),
            (None, _) if req => return Err((-32602, format!("missing prompt argument {a:?}"))),
            (None, "count") => "20".into(),
            (None, "language") => "my usual language".into(),
            (None, _) => String::new(),
        };
        t = t.replace(&format!("{{{a}}}"), &v);
    }
    Ok(
        json!({ "description": desc, "messages": [{ "role": "user", "content": { "type": "text", "text": t } }] }),
    )
}

// ---------------------------------------------------------------------------------------
// Server state, notifications

#[derive(Default)]
struct Subs {
    /// Legacy session: initialized (gets tools/list_changed) and subscribed URIs.
    legacy: bool,
    legacy_uris: BTreeSet<String>,
    /// Modern `subscriptions/listen` streams: (request id, tools, uris).
    listens: Vec<(Value, bool, BTreeSet<String>)>,
}

impl Subs {
    fn watched(&self) -> (bool, BTreeSet<String>) {
        let tools = self.legacy || self.listens.iter().any(|l| l.1);
        let mut u = self.legacy_uris.clone();
        for l in &self.listens {
            u.extend(l.2.iter().cloned());
        }
        (tools, u)
    }
}

type Out = Arc<Mutex<std::io::Stdout>>;

fn send(out: &Out, v: &Value) -> bool {
    let Ok(o) = out.lock() else { return false };
    let mut o = o.lock();
    writeln!(o, "{v}").and_then(|_| o.flush()).is_ok()
}

fn notify(out: &Out, subs: &Mutex<Subs>, method: &str, uri: Option<&str>) {
    let Ok(s) = subs.lock() else { return };
    let base = |sub: Option<&Value>| {
        let mut p = match uri {
            Some(u) => json!({ "uri": u }),
            None => json!({}),
        };
        if let Some(id) = sub {
            p["_meta"] = json!({ META_SUB: id });
        }
        json!({ "jsonrpc": "2.0", "method": method, "params": p })
    };
    let legacy = match uri {
        Some(u) => s.legacy_uris.contains(u),
        None => s.legacy,
    };
    if legacy {
        send(out, &base(None));
    }
    for (id, tools, uris) in &s.listens {
        let want = match uri {
            Some(u) => uris.contains(u),
            None => *tools,
        };
        if want {
            send(out, &base(Some(id)));
        }
    }
}

/// Polls Vivi while anything is subscribed and sends change notifications.
fn poller(out: Out, subs: Arc<Mutex<Subs>>) {
    let ms = std::env::var("VIVI_MCP_POLL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000u64);
    let mut last: std::collections::HashMap<String, String> = Default::default();
    loop {
        std::thread::sleep(Duration::from_millis(ms));
        let (tools, uris) = match subs.lock() {
            Ok(s) => s.watched(),
            Err(_) => return,
        };
        let mut keys: Vec<String> = uris.into_iter().collect();
        if tools {
            keys.push("tools".into());
        }
        last.retain(|k, _| keys.contains(k));
        for k in keys {
            let fp = if k == "tools" {
                fingerprint("vivi://plugin-commands")
            } else {
                fingerprint(&k)
            };
            let changed = last.get(&k).is_some_and(|old| *old != fp);
            last.insert(k.clone(), fp);
            if changed {
                if k == "tools" {
                    notify(&out, &subs, "notifications/tools/list_changed", None);
                } else {
                    notify(&out, &subs, "notifications/resources/updated", Some(&k));
                }
            }
        }
    }
}

fn capabilities() -> Value {
    json!({ "tools": { "listChanged": true }, "resources": { "subscribe": true }, "prompts": {} })
}

fn server_info() -> Value {
    json!({ "name": "vivi-mcp", "title": "Vivi media player", "version": env!("CARGO_PKG_VERSION") })
}

/// Handle one message; None = no reply (notifications, open subscription streams).
fn handle(req: &Value, out: &Out, subs: &Arc<Mutex<Subs>>) -> Option<Value> {
    let method = req["method"].as_str().unwrap_or("");
    let params = &req["params"];
    let Some(id) = req.get("id").cloned() else {
        if method == "notifications/initialized" {
            if let Ok(mut s) = subs.lock() {
                s.legacy = true;
            }
        } else if method == "notifications/cancelled" {
            let rid = &params["requestId"];
            if let Ok(mut s) = subs.lock() {
                s.listens.retain(|l| &l.0 != rid);
            }
        }
        return None;
    };
    let reply = |r: Result<Value, (i64, String, Option<Value>)>| match r {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, msg, data)) => {
            let mut e = json!({ "code": code, "message": msg });
            if let Some(d) = data {
                e["data"] = d;
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": e })
        }
    };
    // Era: a protocol version in `_meta` makes this a stateless (modern) request.
    let asked = params["_meta"][META_VERSION].as_str();
    if let Some(v) = asked {
        if !supported().contains(&v) {
            return Some(reply(Err((
                -32022,
                "Unsupported protocol version".into(),
                Some(json!({ "supported": supported(), "requested": v })),
            ))));
        }
    }
    let modern = asked == Some(MODERN);
    let not_found = if modern { -32602 } else { -32002 };
    let plain = |r: Result<Value, (i64, String)>| r.map_err(|(c, m)| (c, m, None));
    let result: Result<Value, (i64, String, Option<Value>)> = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or(LEGACY[0]);
            let v = if LEGACY.contains(&asked) {
                asked
            } else {
                LEGACY[0]
            };
            Ok(
                json!({ "protocolVersion": v, "capabilities": capabilities(),
                "serverInfo": server_info(), "instructions": INSTRUCTIONS }),
            )
        }
        "server/discover" => Ok(
            json!({ "supportedVersions": supported(), "capabilities": capabilities(),
            "instructions": INSTRUCTIONS, "ttlMs": 3_600_000, "cacheScope": "public" }),
        ),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let mut all = tools();
            let extra = plugin_tools(&all);
            all.extend(extra);
            Ok(json!({ "tools": all.iter().map(Tool::json).collect::<Vec<_>>() }))
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(a) => a.clone(),
            };
            let statics = tools();
            let t = statics
                .iter()
                .find(|t| t.name == name)
                .cloned()
                .or_else(|| {
                    name.starts_with("plugin_")
                        .then(|| plugin_tools(&statics).into_iter().find(|t| t.name == name))
                        .flatten()
                });
            match t {
                None => Err((-32602, format!("unknown tool {name:?}"), None)),
                Some(t) => Ok(call_tool(&t, args)),
            }
        }
        "resources/list" => Ok(json!({ "resources": RESOURCES.iter().map(|(u, n, d, _)|
            json!({ "uri": u, "name": n, "description": d, "mimeType": "application/json" })).collect::<Vec<_>>() })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "resources/read" => {
            let uri = params["uri"].as_str().unwrap_or("");
            match RESOURCES.iter().find(|r| r.0 == uri) {
                None => Err((not_found, format!("resource not found: {uri}"), None)),
                Some(r) => call_vivi(r.3, json!({}))
                    .map_err(|e| (-32603, e, None))
                    .map(|v| {
                        json!({ "contents": [{ "uri": uri, "mimeType": "application/json",
                        "text": serde_json::to_string_pretty(&v).unwrap_or_default() }] })
                    }),
            }
        }
        "resources/subscribe" | "resources/unsubscribe" => {
            let uri = params["uri"].as_str().unwrap_or("");
            if !RESOURCES.iter().any(|r| r.0 == uri) {
                Err((not_found, format!("resource not found: {uri}"), None))
            } else {
                if let Ok(mut s) = subs.lock() {
                    if method == "resources/subscribe" {
                        s.legacy_uris.insert(uri.into());
                    } else {
                        s.legacy_uris.remove(uri);
                    }
                }
                Ok(json!({}))
            }
        }
        "subscriptions/listen" => {
            let f = &params["notifications"];
            let tools = f["toolsListChanged"].as_bool() == Some(true);
            let uris: BTreeSet<String> = f["resourceSubscriptions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|u| RESOURCES.iter().any(|r| r.0 == *u))
                .map(String::from)
                .collect();
            let mut ack = Map::new();
            if tools {
                ack.insert("toolsListChanged".into(), json!(true));
            }
            if !uris.is_empty() {
                ack.insert("resourceSubscriptions".into(), json!(uris));
            }
            send(
                out,
                &json!({ "jsonrpc": "2.0", "method": "notifications/subscriptions/acknowledged",
                "params": { "_meta": { META_SUB: id }, "notifications": ack } }),
            );
            if let Ok(mut s) = subs.lock() {
                s.listens.retain(|l| l.0 != id);
                s.listens.push((id.clone(), tools, uris));
            }
            return None; // the stream stays open until stdin closes or the client cancels
        }
        "prompts/list" => Ok(
            json!({ "prompts": prompts().into_iter().map(|(n, d, a, _)| json!({
            "name": n, "description": d,
            "arguments": a.into_iter().map(|(an, ad, r)| json!({ "name": an, "description": ad, "required": r })).collect::<Vec<_>>() })).collect::<Vec<_>>() }),
        ),
        "prompts/get" => plain(get_prompt(params)),
        "completion/complete" => Ok(json!({ "completion": { "values": [], "hasMore": false } })),
        _ => Err((-32601, format!("method not found: {method}"), None)),
    };
    let result = result.map(|mut r| {
        if modern {
            r["resultType"] = json!("complete");
            r["_meta"] = json!({ "io.modelcontextprotocol/serverInfo": server_info() });
            if method.ends_with("/list") || method == "resources/read" {
                // Lists change with plugins and playback: cache briefly, per user.
                r["ttlMs"] = json!(if method == "resources/read" {
                    0
                } else {
                    60_000
                });
                r["cacheScope"] = json!("private");
            }
        }
        r
    });
    Some(reply(result))
}

fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "vivi-mcp {}: MCP server (stdio) for the Vivi media player.\nProtocol: {} (stateless) and {} (initialize).\nSocket: {}",
            env!("CARGO_PKG_VERSION"),
            MODERN,
            LEGACY.join(", "),
            socket_path().display()
        );
        return;
    }
    let out: Out = Arc::new(Mutex::new(std::io::stdout()));
    let subs: Arc<Mutex<Subs>> = Arc::default();
    {
        let (o, s) = (out.clone(), subs.clone());
        std::thread::spawn(move || poller(o, s));
    }
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": format!("parse error: {e}") } })),
            Ok(Value::Array(batch)) => {
                let r: Vec<Value> = batch
                    .iter()
                    .filter_map(|m| handle(m, &out, &subs))
                    .collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(req) if !req.is_object() => Some(json!({ "jsonrpc": "2.0", "id": null,
                "error": { "code": -32600, "message": "invalid request" } })),
            Ok(req) => handle(&req, &out, &subs),
        };
        if let Some(r) = reply {
            if !send(&out, &r) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn plugin_names_are_valid_tool_names() {
        assert_eq!(
            plugin_tool_name("offline-downloads", "add-current"),
            "plugin_offline-downloads_add-current"
        );
        assert_eq!(plugin_tool_name("YouTube", ""), "plugin_youtube_open");
        let n = plugin_tool_name(&"x".repeat(80), "a b");
        assert!(
            n.len() <= 64
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[test]
    fn validation_catches_bad_arguments() {
        let t = tools();
        let get = |n: &str| t.iter().find(|x| x.name == n).unwrap().input.clone();
        assert!(validate(&get("set_speed"), &json!({ "speed": 1.5 })).is_ok());
        assert!(validate(&get("set_speed"), &json!({}))
            .unwrap_err()
            .contains("missing"));
        assert!(validate(&get("set_speed"), &json!({ "speed": 9 }))
            .unwrap_err()
            .contains("between"));
        assert!(validate(&get("set_speed"), &json!({ "speed": "fast" }))
            .unwrap_err()
            .contains("number"));
        assert!(validate(&get("set_repeat"), &json!({ "mode": "twice" }))
            .unwrap_err()
            .contains("one of"));
        assert!(validate(&get("play"), &json!({ "x": 1 }))
            .unwrap_err()
            .contains("no arguments"));
        assert!(validate(&get("open"), &json!({ "target": "  " }))
            .unwrap_err()
            .contains("empty"));
        assert!(validate(&get("playlist_play"), &json!({ "index": 1.5 })).is_err());
        assert!(validate(&get("set_audio_track"), &json!({ "id": "auto" })).is_ok());
        assert!(validate(&get("set_ab_loop"), &json!({ "a": null, "b": 4.5 })).is_ok());
        assert!(validate(&get("set_ab_loop"), &json!({ "a": "x" })).is_err());
        assert!(validate(&get("set_sleep_timer"), &json!({ "minutes": 0 })).is_err());
        assert!(validate(&get("remove_bookmark"), &json!({})).is_err());
        assert!(validate(&get("cancel_sleep_timer"), &json!({ "timer": "eof" })).is_err());
    }

    #[test]
    fn every_tool_is_well_formed() {
        let t = tools();
        let mut names = BTreeSet::new();
        for x in &t {
            assert!(names.insert(x.name.clone()), "duplicate {}", x.name);
            assert!(x.name.len() <= 64 && !x.desc.is_empty() && !x.title.is_empty());
            let j = x.json();
            assert_eq!(j["inputSchema"]["type"], "object");
            for r in j["inputSchema"]["required"].as_array().unwrap() {
                assert!(
                    j["inputSchema"]["properties"][r.as_str().unwrap()].is_object(),
                    "{}",
                    x.name
                );
            }
            // read-only tools never claim to be destructive
            if j["annotations"]["readOnlyHint"] == true {
                assert!(j["annotations"].get("destructiveHint").is_none());
            }
        }
    }

    #[test]
    fn pages() {
        let v = page(json!([1, 2, 3, 4, 5]), 1, 2);
        assert_eq!(v["items"], json!([2, 3]));
        assert_eq!(v["total"], 5);
        assert_eq!(v["next_offset"], 3);
        let v = page(json!({ "count": 2, "items": [1, 2] }), 0, 50);
        assert!(v.get("next_offset").is_none());
        assert_eq!(v["total"], 2);
    }

    #[test]
    fn prompts_fill_arguments() {
        let r =
            get_prompt(&json!({ "name": "play_radio", "arguments": { "query": "jazz" } })).unwrap();
        assert!(r["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("\"jazz\""));
        assert!(get_prompt(&json!({ "name": "play_radio" })).is_err());
        let r = get_prompt(&json!({ "name": "queue_from_library", "arguments": { "query": "x" } }))
            .unwrap();
        assert!(r["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("up to 20"));
    }
}
