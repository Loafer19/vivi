# Controlling Vivi from AI agents (MCP)

`vivi-mcp` is a small separate binary that speaks the
[Model Context Protocol](https://modelcontextprotocol.io) over **stdio** and forwards tool
calls to a running Vivi through a local control socket. The player itself only gains a
tiny socket listener (`src/ipc.rs`); the MCP server links nothing but `std` and
`serde_json`.

```
AI client (Claude Desktop, Cursor, …) ──stdio/JSON-RPC──▶ vivi-mcp ──Unix socket/JSON lines──▶ vivi
```

## 1. Enable remote control in Vivi

The socket is **off by default**. Turn on *Settings → General → Remote control → Allow AI
agents and scripts* and restart Vivi, or start it once with `VIVI_IPC=1 vivi`.

The socket is `$XDG_RUNTIME_DIR/vivi.sock` (fallback `~/.cache/vivi/vivi.sock`, override
with `VIVI_SOCKET`), created with mode `0600`: only your user can connect. There is no
network listener.

## 2. Get `vivi-mcp`

It is built together with Vivi: `cargo build --release` produces `target/release/vivi-mcp`
next to `target/release/vivi`. The Arch package installs it to `/usr/bin/vivi-mcp`, the
Flatpak to `/app/bin/vivi-mcp`. `vivi-mcp --help` prints the socket path it will use.

## 3. Configure your client

**Claude Desktop** (`~/.config/Claude/claude_desktop_config.json`,
macOS `~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "vivi": { "command": "/usr/bin/vivi-mcp" }
  }
}
```

**Cursor** (`~/.cursor/mcp.json`, or `.cursor/mcp.json` in a project):

```json
{
  "mcpServers": {
    "vivi": { "command": "vivi-mcp", "args": [] }
  }
}
```

**Claude Code**: `claude mcp add vivi -- vivi-mcp`

**VS Code / GitHub Copilot** (`.vscode/mcp.json`):

```json
{ "servers": { "vivi": { "type": "stdio", "command": "vivi-mcp" } } }
```

**Windsurf, Zed, Continue, Cline, …**: any client that launches stdio servers works with
`command: vivi-mcp`. If the client does not inherit `XDG_RUNTIME_DIR`, pass the socket
explicitly: `"env": { "VIVI_SOCKET": "/run/user/1000/vivi.sock" }`.

**Flatpak**: use `"command": "flatpak", "args": ["run", "--command=vivi-mcp", "io.github.loafer19.vivi"]`.

## Tools

| Tool | Arguments | What it does |
|---|---|---|
| `get_status` | – | Now playing: title, artist, album, path, position, duration, volume, mute, playing, subtitles, queue position, EQ preset |
| `play`, `pause`, `toggle_pause`, `stop` | – | Transport (`stop` pauses and rewinds) |
| `seek` | `position` *or* `offset` (s) | Absolute or relative seek |
| `set_volume` | `level`, `muted` | Volume in % and/or mute |
| `open` | `target` | Play a file path or URL now (yt-dlp sites work when yt-dlp is installed) |
| `playlist_list` | – | Queue entries with index and current flag |
| `playlist_add` | `target` | Append to the queue |
| `playlist_play` | `index` | Jump to a queue entry |
| `next`, `previous` | – | Queue navigation |
| `search_library` | `query`, `kind`, `limit` | Words matched in title/artist/album/genre/path of the scanned library |
| `set_subtitles` | `enabled` | Show/hide subtitles |
| `list_eq_presets` | – | Built-in and custom EQ presets, current one |
| `set_eq_preset` | `name` | Select a preset, or `Off` to disable the equalizer |
| `list_audio_tracks` | – | Audio tracks: `id`, `title` (with language), `selected` |
| `set_audio_track` | `id` (integer, `auto` or `no`) | Switch the audio track |
| `list_subtitle_tracks` | – | Subtitle tracks (embedded and external) |
| `set_subtitle_track` | `id` (integer, `auto` or `no`) | Switch subtitles; `no` turns them off |
| `get_chapters` | – | `current` index and `chapters` (`index`, `title`, `start` s) |
| `seek_chapter` | `index` *or* `offset` (+1/-1) | Jump to a chapter |
| `set_speed` | `speed` (0.1–4) | Playback speed |
| `screenshot` | `path`, `mode` (`subtitles`/`video`/`window`) | Saves the current frame and returns its path; default folder is Settings → Video → screenshot folder. Errors on audio-only items |
| `open_network_stream` | `target`, `name`, `queue` | Play (or queue) a stream URL: Internet radio, IPTV `.m3u8`/HLS, DASH, RTMP/RTSP/UDP, Icecast. `name` is shown while it loads |
| `search_streams` | `query`, `kind` (`radio`/`tv`), `limit` | Search radio-browser.info, iptv-org TV, your Network Streams favorites and playlists; returns `url`s for `open_network_stream` (needs network; may take a few seconds) |
| `list_plugins` | – | Plugins: `id`, `name`, `version`, `kind`, `enabled` (read-only) |
| `plugin_info` | `id` | Description, author, version and setting keys/titles (no values) |
| `get_lyrics` | – | Lines shown by the Lyrics plugin, `synced`, `current_line` |

IPTV / radio: `open` also plays URLs, but `open_network_stream` validates the URL, can
queue instead of replacing, and shows a station name. A whole `.m3u` playlist URL can be
passed to `open` (mpv expands it into the queue); to browse channels, guide and logos use
the Network Streams dialog in Vivi.

Resources: `vivi://status` and `vivi://playlist` (JSON).

## Testing

- Automated: `cargo test --test mcp` runs the real `vivi-mcp` binary against a fake Vivi
  socket and checks initialize → tools/list (all tools) → search → open → status → track listing.
- MCP Inspector against a running Vivi:
  `VIVI_IPC=1 vivi &` then `npx @modelcontextprotocol/inspector target/release/vivi-mcp`.
- By hand:
  ```sh
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_status","arguments":{}}}' | vivi-mcp
  ```
- The socket can also be scripted directly:
  `echo '{"cmd":"status"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/vivi.sock`.

## Limitations

- Linux and macOS only (Unix socket); on Windows `vivi-mcp` reports that it is unsupported.
- stdio transport only; no HTTP/SSE server yet.
- The setting takes effect after a restart; one Vivi instance owns the socket.
- `search_library` covers the scanned library only (Settings → Library folders).
- Not exposed: changing plugin settings or enabling/disabling plugins (plugin tools are
  read-only), and the Network Streams program guide.
