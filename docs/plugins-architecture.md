# Plugin distribution: how plugins are downloaded and updated

Status: implemented in v0.9.0. Developer-facing guide: <https://plugins.viviplayer.app/dev/> (source `docs/dev/*.md` in vivi-plugins); this file keeps the core design notes. Code: `src/plugins/packages.rs`,
`scripts/package-plugins.py` and `.github/workflows/plugins.yml` in
[Loafer19/vivi-plugins](https://github.com/Loafer19/vivi-plugins) (packages, helpers, catalog, marketplace and all plugin/helper releases live there).

## Goals

* Keep the app small (well under 100 MB) and the core fast; not every feature is built in.
* Users install, update, roll back and remove plugins from Settings → Extensions without
  leaving the app, and see what a plugin may do before installing it.
* Publishing a plugin is "add a folder, bump a version, push"; CI does the rest.
* No UI blocking: all network and disk work runs on background threads.

## What stays built in

| Plugin | Why built in |
| --- | --- |
| Music Visualizer | Core feature; renders through the Rust FFT/`viz` pipeline and Slint, 60 fps. |
| Skip Intro & Credits | Draws a Slint button over the video; needs the Rust UI. |
| Lyrics | Karaoke overlay is a Slint component synced to the player clock. |
| Subtitle Search | Lives inside the Add Subtitles dialog (Slint): results with badges, details and preview; per-file offset (`z` / `x`, side panel) in `subtitle-offsets.json`, picks in `subtitle-picks.json`; `script-message vivi-sub-offset <delta\|set:v\|reset>` lets the Subtitle Auto-Sync package hand over its offset keys. |
| Auto-tagger | Preview dialog with candidates and a per-field diff; fingerprints audio and writes tags/covers in Rust (lofty, rusty-chromaprint). |
| Network Streams | Browser dialog with sources, results and details; plays through the player and keeps saved/recent lists. |
| Free Music | Shared music dialog in rich mode (`FmRow` rows with covers, badges and licence chips, `MdFilter` dropdowns, infinite scroll); Jamendo + Internet Archive searched in parallel and merged; download queue writes tags with lofty. |

These use the Rust UI directly. Discord Rich Presence (#57) and Remote Control (#58) were
built in until 0.9.0; they need a socket client / server that Lua cannot do, so they are now
packages with a native helper (see [Native helpers](#native-helpers)), and the core no
longer links `discord-rich-presence` or carries the HTTP server. The ListenBrainz Scrobbler needs no UI beyond its settings, so it is a
downloadable package (`plugins/scrobbler`). Scripts cannot draw Slint widgets, so moving them out
would mean either a worse UI (mpv OSD text) or a declarative-UI host API, which is the
WASM step below. They stay built in until that exists.

Built-in plugins hand the UI typed state rather than display strings: the tag review
sends a `TagStatus` per file (tagged / renamed / skipped / failed / undone), lyrics a
`LyricsState` (found / searching / missing) and numeric offsets. Shared enums live in
`ui/core/types.slint` (also `BarStyle`, `PanelTab`, `Overlay`); Rust maps its own values
onto them at the boundary.

Each built-in plugin with a UI owns one Slint global in `ui/state/` instead of
prefixed properties on `AppWindow`: `Lyrics` (lyrics view, karaoke words, manual
search as `search-*`) and `Subtitles` (Add Subtitles dialog, sync / pick state).
`MusicDialogState` is shared by Auto-tagger, Free Music and Network Streams: the
plugin that opens it sets `owner` (its id, for routing `plugin-command`) and `caps`,
an `MdCaps { logos, chips, rich }` saying which parts of the dialog it uses, so the
UI never compares plugin ids. The tag review and now-playing card are moving to the
same pattern. Plugin
code writes them with `cx.app.global::<crate::Lyrics>().set_lines(...)`, and
`ui/app.slint` binds the plugin's dialog properties to that global, so a new field
touches the global, the dialog, one binding and the plugin. User actions still reach the plugin through
`plugin-command(id, action, args)`.

### Music plugins (#63, #65, #69, #70)

| Plugin | Where | Why | Keys |
| --- | --- | --- | --- |
| Auto-tagger (`auto-tagger`, #69 incl. #65) | built in | preview/diff dialog; tag writing and fingerprinting need lofty and Chromaprint, not available to scripts | AcoustID application key (optional, <https://acoustid.org/new-application>); MusicBrainz, Cover Art Archive, Deezer, iTunes Search and LRCLIB need none; Last.fm API key (genres) and Discogs token optional (#79). Candidates from all sources are merged and ranked by confidence (`rank_candidates`, `Candidate::confidence`); disagreeing values become per-field alternatives (⇄ in the review); fingerprint matches propose a sanitized, opt-in file name (`sanitize_file_name`, `rename_to`). The review shows the source of every field; synced LRCLIB lyrics are embedded when *Embed synced lyrics* is on (MP3 SYLT+USLT, FLAC/Ogg/Opus `LYRICS`, M4A `©lyr`, #80) |
| Equalizer (`equalizer`, #74) | script | presets via mpv `equalizer`/`superequalizer` af | none |
| Subtitle sync (`subtitle-sync`, #72) | script | alass (system or managed download) + managed media ffmpeg | none |
| Chapter thumbnails (`chapter-thumbnails`, #75) | script + core | managed media ffmpeg; core reads the cache | none |
| AcoustID (`acoustid`, #76) | script + tagger | fpcalc (system or Chromaprint download); results open in the tagger review with a rename proposal | AcoustID application key |
| Scrobbler (`scrobbler`, #77) | script | Last.fm + ListenBrainz | Last.fm API key + secret, ListenBrainz token |
| Free Music (`free-music`, #70) | built in | search/results/licence dialog; downloads are tagged with lofty | Jamendo `client_id` (optional, <https://devportal.jamendo.com>); Internet Archive needs none |
| Subtitle Auto-Sync (`subtitle-sync`, #56) | package (mpv Lua) | runs an external tool and `sub-add`s the result; no UI | none; needs `alass`/`alass-cli` or `ffsubsync` in `PATH` |
| SponsorBlock (`sponsorblock`, #64) | package (mpv Lua) | HTTP via `curl`, seeks and chapters; "ask" mode uses a toast + `Alt+K` instead of a Slint button | none |
| Scene Chapters (`scene-chapters`, #62) | package (mpv Lua) | ffmpeg subprocess, `chapter-list`; thumbnails cached for a future Chapters-panel view | none; needs `ffmpeg` |
| ListenBrainz Scrobbler (`scrobbler`, #63) | package (mpv Lua) | no UI: settings, OSD status and HTTP via `curl` are enough | ListenBrainz user token (<https://listenbrainz.org/settings/>) |

MusicBrainz calls share one process-wide limiter (at most one request per second) and send
`User-Agent: Vivi/<version> ( https://github.com/Loafer19/vivi )`. Last.fm scrobbling needs an
MD5-signed desktop auth flow through the browser; it is left for a later built-in plugin.

### Network Streams (#67)

Built in (`network-streams`): it needs a list dialog, so it reuses the shared plugin dialog
(source dropdown, query, results, detail pane) rather than the *Open Network Stream* dialog,
which keeps its Figma design. Sources: radio-browser.info (`/json/stations/search` by
name / country / countrycode / tag, `/json/stations/topvote`, `hidebroken=true`, ordered by
votes, mirrors `de1`, `fi1`, `at1` tried in turn; a play is reported to `/json/url` as the API
asks), M3U / extended M3U / IPTV playlists from a path or URL (`#EXTINF` name, `tvg-logo`,
`group-title`, `tvg-country`; relative entries skipped), and JSON lists `saved.json` /
`recent.json` (30 entries, de-duplicated by URL) under `$XDG_DATA_HOME/vivi/streams/`.
Stream info reads mpv's `icy-title`, `icy-name`, codec, bitrate and `demuxer-cache-duration`.
TV comes from iptv-org: `api/countries.json` / `api/categories.json` as pickers, then
`iptv/countries/<code>.m3u` / `iptv/categories/<id>.m3u` (and `index.m3u` for a word typed
into the M3U source), cached for 24 h under `$XDG_CACHE_HOME/vivi/iptv/` with the stale
copy used offline. Music adds radio-browser `/json/tags` (by station count) with
`topvote` / `topclick`, and SomaFM `channels.json` (MP3 playlist preferred). Picker sources
filter locally as the user types; row logos load in the background into
`stream-logos/`. The loading spinner / error toast for network streams is core
(`src/loading.rs`, mpv `start-file`, `playback-restart`, `paused-for-cache`, `end-file`
error). No key. Later: EPG for IPTV, podcasts (RSS).

## Options considered

### (a) mpv Lua/JavaScript script packages — **chosen now**

* mpv already embeds Lua (and JavaScript via mujs); Vivi already loads user scripts.
  Zero extra binary size, zero new runtime.
* Rich API: properties, commands, key bindings, OSD/ASS overlays, filters (`af`/`vf`),
  `mp.options` for settings, `mp.utils` for files and HTTP via `curl` subprocess.
* Huge existing ecosystem (most mpv scripts work unchanged).
* Downsides: no sandbox (a script runs with the user's rights), mpv cannot unload a
  script (disable/update/uninstall take effect on restart), UI limited to the mpv OSD.

### (b) WASM plugins (wasmtime / Extism) — **planned, not now**

* Real sandbox and capability-based permissions; one binary for every OS; any language.
* A host API would expose player control, `http` (allow-listed), settings and declarative
  UI messages (toast, overlay text, panel lists) rendered by Slint.
* Cost: wasmtime adds roughly 10–20 MB to the binary and a lot of API surface to design
  and keep stable; Extism is lighter to integrate but still bundles wasmtime. Within the
  100 MB budget, but not worth it until a plugin needs Slint UI or untrusted code is
  common. The package format below already has a `kind` field, so `kind = "wasm"` can be
  added later without changing the catalog, installer or Settings page.

### (c) Native dynamic libraries — **rejected**

* Rust has no stable ABI: every plugin would have to be rebuilt for each Vivi version,
  compiler version and OS/arch.
* Native code is not sandboxable at all and can crash the player.

## Package format

A zip file (`<id>-<version>.zip`) with `manifest.toml` at its root:

```toml
id = "night-mode"                 # lowercase-with-dashes, unique, = folder name
name = "Night Mode"
version = "1.0.0"                 # semver MAJOR.MINOR.PATCH
description = "Compresses loud scenes…"
author = "Vivi"
kind = "mpv-lua"                  # mpv-lua | mpv-js (wasm reserved)
entry = "night-mode.lua"          # file name = mpv script name = id
min_vivi = "0.8.0"                # oldest compatible Vivi
icon = "night"                    # Settings icon key (ui/core/icons.slint Icons.plugin)
lucide = "moon"                   # docs site icon
permissions = []                  # network | filesystem | subprocess
default_enabled = true            # optional

[[settings]]                      # optional, shown in Settings → Extensions
key = "strength"
title = "Strength"
type = "choice"                   # toggle | choice | text | secret | action
options = ["Medium", "Light", "Strong"]
default = "Medium"
```

Settings become mpv `script-opts` (`<id>-<key>=<value>`; toggles as `yes`/`no`) that the
script reads with `mp.options.read_options(o, "<id>", on_update)`. Changes in Settings are
applied live with `change-list script-opts append`. An `action` setting sends
`script-message-to <script> <key>` (mpv names the script after the entry file with `-`
replaced by `_`, e.g. `ab_loop`).

Vivi turns off mpv's default key bindings and forwards keys it does not use itself (and
every `Alt` combination) to mpv, so packages bind keys with `mp.add_forced_key_binding`
(plain `add_key_binding` lands in mpv's disabled default section).

Permissions are declarations, shown before install and in Settings. Because mpv scripts are
not sandboxed they are a promise reviewed when a plugin is accepted into the catalog, not
an enforced boundary; enforcing them is one of the reasons for the WASM step.

## Native helpers

A package may ship small programs next to its script, for what mpv scripts cannot do
(sockets, servers, native APIs):

```toml
binaries = ["vivi-discord"]       # helper names; built per platform
```

* Sources live in `helpers/` (one Rust crate, its own `Cargo.lock`, only `serde_json`;
  nothing is linked into Vivi, so the Flatpak's `cargo-sources.json` is unaffected).
  `helpers/src/lib.rs` is a request/response client for mpv's JSON IPC.
* CI (`plugins.yml`, job `helpers`) builds them for `linux-x86_64` and `linux-aarch64`
  (static musl), `windows-x86_64`, `macos-aarch64` and `macos-x86_64`, and uploads
  `<name>-<platform>[.exe]` to the package's release next to the zip.
* The catalog entry lists `needs` (the names) and `binaries`
  (`{name, platform, url, sha256, size}` per asset). Helper builds are not reproducible,
  so CI writes these checksums into `plugins/index.json` after publishing a new version
  (bot commit) and redeploys the site; released versions always reuse their published
  assets. Until that commit lands, the new version shows "Not available for <platform>".
* The installer downloads the zip and the helpers for `std::env::consts::{OS, ARCH}`,
  checks each sha256 and writes them to `<id>/<version>/bin/<name>[.exe]` (mode 0755)
  before switching `current`. Browse shows the zip + helper size; a package without a
  helper for this platform shows "Not available for <platform> yet".
* Vivi passes every package its folder as the script-opt `<id>-pkgdir` (the key `pkgdir`
  is reserved). The script turns on `input-ipc-server` if it is not set
  (`$XDG_RUNTIME_DIR/vivi-mpv-<pid>.sock`, `\\.\pipe\vivi-mpv-<pid>` on Windows), starts
  the helper with mpv's `subprocess` command (`playback_only = false`) and the socket path
  plus its settings as arguments, restarts it when a setting changes and aborts it on
  shutdown. Helpers exit by themselves when the IPC connection closes, report status in
  `user-data/<plugin>/…` and show toasts with `script-message vivi-toast`.

Script messages Vivi handles for packages:

| Message | Effect |
| --- | --- |
| `vivi-toast <text>` | toast |
| `vivi-preview-frames <dir>` | load cached seek-bar frames `seek-NN.jpg` from `<dir>` (0.9.2) |
| `vivi-chapter-thumbs <dir>` | reload chapter thumbnails `chapter-N.jpg` from `<dir>` (0.9.2) |
| `vivi-tagger-acoustid <path> <json>` | open the auto-tagger review for `<path>` with AcoustID ids and a rename pattern (0.9.2) |
| `vivi-setting <id> <key> <value>` | saves one of the package's declared settings (e.g. a generated PIN) and applies it as script-opt |
| `vivi-pick-folder <id> <key>` | folder dialog; the result is saved as that setting and sent back as `script-message-to <script> folder-picked <path>` |
| `vivi-qr <title> <text> [caption]` | dialog with `text` as a QR code (e.g. a pairing link), shown above Settings (0.10.1+) |
| `vivi-fullscreen [toggle\|on\|off]` | switches Vivi's window fullscreen (0.10.1+) |

`action` settings may set `label` (button text, default "Run"); `size_note` adds a note to
the size in Browse (e.g. the YouTube plugin's own yt-dlp download).

## Registry (catalog)

`plugins/index.json` in vivi-plugins, published at
<https://plugins.viviplayer.app/index.json>, falling back to <https://loafer19.github.io/vivi-plugins/index.json> (override with `VIVI_PLUGIN_INDEX`).
`schema: 1`; built-in plugins are listed with `kind: "builtin"`, packages with
`kind: "package"` and:

```json
{
  "id": "night-mode", "version": "1.0.0", "min_vivi": "0.8.0", "runtime": "mpv-lua",
  "permissions": [], "size": 2731,
  "url": "https://github.com/Loafer19/vivi-plugins/releases/download/plugin-night-mode-v1.0.0/night-mode-1.0.0.zip",
  "sha256": "…"
}
```

Package entries are generated by `scripts/package-plugins.py` from `plugins/<id>/`. Zips
are deterministic (sorted, stored, fixed timestamps), so the sha256 committed in the index
equals the asset CI builds and uploads. Signatures (minisign `signature` field) are
reserved: the index is served over HTTPS from the repository and every download is
checked against its sha256; a detached signature can be added without breaking clients.

## Publishing (CI)

`.github/workflows/plugins.yml` (vivi-plugins) on every push touching `plugins/`:

1. Lua syntax check (`luajit -bl`).
2. `package-plugins.py --check` — fails if `index.json` is stale.
3. For each package whose tag `plugin-<id>-v<version>` does not exist: create that release
   with the zip asset (`--latest=false`, so the app release stays "latest").
4. Download every published asset and compare its sha256 with the index.

Dokploy redeploys plugins.viviplayer.app from the same push; the vivi-plugins Pages mirror follows.

## Client

* **Install dir**: `$XDG_DATA_HOME/vivi/plugins/<id>/<version>/`, plus `<id>/current`
  naming the active version.
* **Update check**: at startup if the cached catalog (`$XDG_CACHE_HOME/vivi/plugin-index.json`)
  is older than 24 h and "Check for plugin updates daily" is on; and on demand with
  *Check now*. When updates exist, a toast says so and Extensions lists them.
* **Install / update** (background thread): download (20 MB cap) → sha256 must match →
  unpack into `<id>/.staging-*` (no absolute paths, `..`, or symlinks; 50 MB cap) →
  manifest id/version must match the catalog, entry must exist, `min_vivi` must be met →
  rename to `<id>/<version>` → atomically replace `current` (write temp + rename).
  A failure at any step leaves the previous version untouched.
* **Permissions prompt**: installing a package (or an update that adds permissions) first
  shows its permissions on the card with *Allow & install* / *Cancel*.
* **Rollback**: the previous version is kept; *Roll back* switches `current` to it. Older
  versions are pruned.
* **Uninstall**: removes `<id>/`. Settings stay in `config.toml` for a reinstall.
* **Enable/disable**: the same ON/OFF toggle as other plugins. A fresh install is loaded
  right away; mpv cannot unload scripts, so disable, update, rollback and uninstall take
  effect after a restart (the UI says so).
* **Compatibility**: entries whose `min_vivi` is newer than the app show "Needs Vivi x".

## Future work

* `kind = "wasm"` via Extism with a declarative UI host API; then Lyrics and Subtitle
  Search can move out of the binary.
* minisign signatures on index entries.
* Third-party catalogs (several index URLs).
