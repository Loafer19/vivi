# Plugin distribution: how plugins are downloaded and updated

Status: implemented in v0.9.0. Code: `src/plugins/packages.rs`,
`scripts/package-plugins.py`, `.github/workflows/plugins.yml`.

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
| Subtitle Search | Lives inside the Add Subtitles dialog (Slint), with a results list. |
| Auto-tagger | Preview dialog with candidates and a per-field diff; fingerprints audio and writes tags/covers in Rust (lofty, rusty-chromaprint). |
| Discord Rich Presence | Needs a Unix socket / Windows named pipe client (Lua cannot open them); `discord-rich-presence` crate on a worker thread. |
| Remote Control | Runs an HTTP server thread and drives the player; scripts cannot listen on sockets. |
| Network Streams | Browser dialog with sources, results and details; plays through the player and keeps saved/recent lists. |
| Free Music | Search dialog with results, licence details, preview and download; writes tags with lofty. |

These use the Rust UI directly. The ListenBrainz Scrobbler needs no UI beyond its settings, so it is a
downloadable package (`plugins/scrobbler`). Scripts cannot draw Slint widgets, so moving them out
would mean either a worse UI (mpv OSD text) or a declarative-UI host API, which is the
WASM step below. They stay built in until that exists.

### Music plugins (#63, #65, #69, #70)

| Plugin | Where | Why | Keys |
| --- | --- | --- | --- |
| Auto-tagger (`auto-tagger`, #69 incl. #65) | built in | preview/diff dialog; tag writing and fingerprinting need lofty and Chromaprint, not available to scripts | AcoustID application key (optional, <https://acoustid.org/new-application>); MusicBrainz and Cover Art Archive need none |
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
icon = "night"                    # Settings icon key (ui/settings.slint PluginIcons)
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

## Registry (catalog)

`plugins/index.json` in this repository, published by the Pages workflow at
<https://loafer19.github.io/vivi/index.json> (override with `VIVI_PLUGIN_INDEX`).
`schema: 1`; built-in plugins are listed with `kind: "builtin"`, packages with
`kind: "package"` and:

```json
{
  "id": "night-mode", "version": "1.0.0", "min_vivi": "0.8.0", "runtime": "mpv-lua",
  "permissions": [], "size": 2731,
  "url": "https://github.com/Loafer19/vivi/releases/download/plugin-night-mode-v1.0.0/night-mode-1.0.0.zip",
  "sha256": "…"
}
```

Package entries are generated by `scripts/package-plugins.py` from `plugins/<id>/`. Zips
are deterministic (sorted, stored, fixed timestamps), so the sha256 committed in the index
equals the asset CI builds and uploads. Signatures (minisign `signature` field) are
reserved: the index is served over HTTPS from the repository and every download is
checked against its sha256; a detached signature can be added without breaking clients.

## Publishing (CI)

`.github/workflows/plugins.yml` on every push touching `plugins/`:

1. Lua syntax check (`luajit -bl`).
2. `package-plugins.py --check` — fails if `index.json` is stale.
3. For each package whose tag `plugin-<id>-v<version>` does not exist: create that release
   with the zip asset (`--latest=false`, so the app release stays "latest").
4. Download every published asset and compare its sha256 with the index.

The Pages workflow deploys the new `index.json` from the same push.

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
