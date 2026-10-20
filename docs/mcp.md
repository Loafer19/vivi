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

## Protocol

`vivi-mcp` is **dual-era**:

- Clients that open with `initialize` (the handshake revisions **2025-11-25**, 2025-06-18,
  2025-03-26, 2024-11-05) get the version they asked for (unknown versions get 2025-11-25).
  Capabilities: `tools` (`listChanged`), `resources` (`subscribe`), `prompts`.
- Requests that carry `io.modelcontextprotocol/protocolVersion` in `_meta` are served
  statelessly as **2026-07-28**: no handshake, `server/discover`, `resultType`,
  `ttlMs`/`cacheScope` on lists, `io.modelcontextprotocol/serverInfo` in result `_meta`,
  `UnsupportedProtocolVersionError` (-32022, with the supported list) for unknown versions,
  and `subscriptions/listen` for change notifications.

Every tool has a `title` and annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`,
`openWorldHint`); clients that honor them ask before destructive calls (replacing or clearing
the queue, removing entries, overwriting a playlist, running plugin commands). Arguments are
checked against the input schema before anything reaches Vivi (types, required, enums,
ranges, unknown keys); a bad call returns a tool error (`isError: true`) that says what to
fix. Results always carry `structuredContent`; list tools take `offset` / `limit` and return
`{total, offset, items, next_offset}` (declared as `outputSchema`, as is `get_status`).

## Tools

**Now playing and transport**

| Tool | Arguments | What it does |
|---|---|---|
| `get_status` | – | Title, artist, album, path, position, duration, volume, mute, speed, playing, subtitles and offset, audio/subtitle track, queue position, chapter, repeat, shuffle, EQ preset, `lyrics_available`, `is_stream`, `has_video` |
| `play`, `pause`, `toggle_pause`, `stop` | – | Transport (`stop` pauses and rewinds) |
| `seek` | `position` *or* `offset` (s) | Absolute or relative seek |
| `set_volume` | `level`, `muted` | Volume in % and/or mute |
| `set_speed` | `speed` (0.1–4) | Playback speed |
| `set_repeat` | `mode` (`off`/`one`/`all`) | Repeat the item or the whole queue |
| `set_shuffle` | `enabled` | Shuffle the queue or restore its order |

**Finding and opening media**

| Tool | Arguments | What it does |
|---|---|---|
| `open` | `target` | Play a file path or URL now, replacing the queue (yt-dlp sites work when yt-dlp is installed) |
| `search_library` | `query`, `kind`, `offset`, `limit` | Words matched in title/artist/album/genre/path of the scanned library |
| `list_recent` | `query`, `kind`, `offset`, `limit` | The Recent list from Home: files, folders, playlists, streams, radio, web video, Free Music, newest first |
| `open_network_stream` | `target`, `name`, `queue` | Play (or queue) a stream URL: Internet radio, IPTV `.m3u8`/HLS, DASH, RTMP/RTSP/UDP, Icecast. `name` is shown while it loads |
| `search_streams` | `query`, `kind` (`radio`/`tv`), `limit` | Search radio-browser.info, iptv-org TV, your Network Streams favorites and playlists; returns `url`s for `open_network_stream` (network; may take a few seconds) |

**Queue and playlists**

| Tool | Arguments | What it does |
|---|---|---|
| `playlist_list` | `offset`, `limit` | Queue entries: `index`, `path`, `title`, `current` |
| `playlist_add` | `target` | Append to the queue |
| `playlist_play` | `index` | Jump to a queue entry |
| `playlist_move` | `from`, `to` | Move an entry (0-based final position) |
| `playlist_remove` | `index` | Remove one entry (destructive) |
| `playlist_clear` | `keep_current` (default true) | Empty the queue, keeping the playing item unless `false` (destructive) |
| `next`, `previous` | – | Queue navigation |
| `list_saved_playlists` | `offset`, `limit` | Library → Playlists: `name`, `path`, `entries` |
| `open_saved_playlist` | `name`, `queue` | Play a saved playlist, or append it |
| `save_queue_as_playlist` | `name`, `overwrite` | Save the queue as a playlist; an existing name needs `overwrite: true` |

**Tracks, subtitles, chapters**

| Tool | Arguments | What it does |
|---|---|---|
| `list_audio_tracks` / `set_audio_track` | `id` (integer, `auto` or `no`) | Audio tracks (`id`, `title` with language, `selected`) and switching |
| `list_subtitle_tracks` / `set_subtitle_track` | `id` (integer, `auto` or `no`) | Subtitle tracks (embedded and external); `no` turns them off |
| `set_subtitles` | `enabled` | Show/hide subtitles |
| `set_subtitle_offset` | `seconds` *or* `delta` *or* `reset` | Subtitle timing (positive = later), saved per file |
| `search_subtitles` | `query` (title, episode or IMDb id `tt…`), `languages` (`en,uk`), `limit` | OpenSubtitles results for the playing file: hash matches first (`hash_match`), then by name/IMDb (`Show S01E05` and `Movie (2020)` are searched by season / episode or year); `file_id`, `language`, `downloads`, `rating`, `hearing_impaired`, `fps`, `trusted`, `machine_translated`. Needs the API key in the Subtitle Search plugin |
| `load_subtitle` | `file_id`, `language` | Download (one from the daily quota; cached copies are free) and show it |
| `sync_subtitles` | – | Align the external subtitle to the audio (Subtitle Auto-Sync plugin, background) |
| `get_chapters` | – | `current` index and `chapters` (`index`, `title`, `start` s) |
| `seek_chapter` | `index` *or* `offset` (+1/-1) | Jump to a chapter |

**Audio, lyrics, screenshots**

| Tool | Arguments | What it does |
|---|---|---|
| `list_eq_presets` / `set_eq_preset` | `name` | Built-in and custom EQ presets; `Off` turns the equalizer off |
| `get_lyrics` | – | Lines shown by the Lyrics plugin, `synced`, `current_line`, `source`, `offset_seconds`, `karaoke` |
| `set_lyrics_offset` | `seconds` *or* `delta` *or* `reset` | Synced-lyrics timing (±30 s), saved per track |
| `set_karaoke` | `enabled` | Open or close the karaoke view |
| `screenshot` | `mode` (`subtitles`/`video`/`window`), `path`, `overwrite`, `save`, `image` | Captures the frame and returns it as **image content** (≤ 4 MB) plus the saved path; `save: false` keeps no file. `path` must be an absolute `.png`/`.jpg`/`.webp` that does not exist (unless `overwrite`). Errors on audio-only items |

**Plugins**

| Tool | Arguments | What it does |
|---|---|---|
| `list_plugins` | – | Plugins: `id`, `name`, `version`, `kind`, `enabled` |
| `plugin_info` | `id` | Description, author, version and setting keys/titles (no values) |
| `list_plugin_commands` | `offset`, `limit` | Commands enabled plugins contribute to the command palette, player/item context menus, Subtitles/Audio/Video menus, tray, control bar and Sources: `plugin`, `command`, `label`, `points`, `where`, `takes_target` |
| `run_plugin_command` | `plugin`, `command`, `target` | Run one of them (only declared contributions can run; `target` is the item path for library-item actions) |
| `plugin_<id>_<command>` | `target` for item actions | **Every plugin contribution is also a tool of its own**, generated from the list above, so installing a plugin gives the agent its commands. The list is re-read on `tools/list`; subscribers get `notifications/tools/list_changed` when plugins change |

IPTV / radio: `open` also plays URLs, but `open_network_stream` validates the URL, can
queue instead of replacing, and shows a station name. A whole `.m3u` playlist URL can be
passed to `open` (mpv expands it into the queue).

## Resources and notifications

| URI | Content |
|---|---|
| `vivi://status` | `get_status` (an update is sent when anything but the position changes) |
| `vivi://playlist` | The queue |
| `vivi://recent` | The Recent list |
| `vivi://lyrics` | Lyrics of the current track (updates ignore the moving line index) |
| `vivi://plugin-commands` | Plugin contributions |

Subscribe with `resources/subscribe` (handshake clients) or `subscriptions/listen` with
`resourceSubscriptions` / `toolsListChanged` (2026-07-28; acknowledged with
`notifications/subscriptions/acknowledged`, notifications carry the subscription id).
`vivi-mcp` polls Vivi once a second while something is subscribed (`VIVI_MCP_POLL_MS`
overrides the interval) and sends `notifications/resources/updated` /
`notifications/tools/list_changed`.

## Prompts

`now_playing`, `play_radio` (`query`), `queue_from_library` (`query`, `count`) and
`fix_subtitles` (`language`): ready-made requests a client can offer in its prompt menu.

## Testing

- Automated: `cargo test --test mcp` runs the real `vivi-mcp` binary against a fake Vivi
  socket: handshake and stateless requests, every tool listed with annotations, argument
  validation, pagination, queue tools, plugin tools, screenshots as images, prompts, and
  change notifications in both eras. `cargo test --bin vivi-mcp` covers the helpers.
- MCP Inspector against a running Vivi:
  `VIVI_IPC=1 vivi &` then `npx @modelcontextprotocol/inspector target/release/vivi-mcp`.
- By hand:
  ```sh
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_status","arguments":{}}}' | vivi-mcp
  ```
- The socket can also be scripted directly:
  `echo '{"cmd":"status"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/vivi.sock`.

## Security

- Local only: a `0600` Unix socket, no network listener; nothing leaves the machine except
  the network searches you ask for (streams, subtitles).
- The OpenSubtitles API key never leaves Vivi: `search_subtitles` / `load_subtitle` run
  inside Vivi's socket thread and return results only.
- No file reading beyond media: tools return paths, and the only file `vivi-mcp` reads is
  the screenshot Vivi just wrote. Screenshots never overwrite a file unless asked;
  `save_queue_as_playlist` writes only into Vivi's playlist folder.
- Plugin commands are limited to what plugins declare as contributions; plugin settings
  cannot be read or changed.

## Limitations

- Linux and macOS only (Unix socket); on Windows `vivi-mcp` reports that it is unsupported.
- stdio transport only; no Streamable HTTP (it would need an authenticated local listener).
- The setting takes effect after a restart; one Vivi instance owns the socket.
- `search_library` covers the scanned library only (Settings → Library folders).
- Not exposed: plugin settings, enabling/disabling plugins, Free Music search (use its
  plugin commands), the Network Streams program guide, audio effects other than the EQ.
