# Plugin distribution: how plugins are downloaded and updated

Status: implemented in v0.9.0. Developer-facing guide: <https://plugins.viviplayer.app/dev/> (source `docs/dev/*.md` in vivi-plugins); this file keeps the core design notes. Code: `src/plugins/packages.rs`,
`scripts/package-plugins.py` and `.github/workflows/plugins.yml` in
[Loafer19/vivi-plugins](https://github.com/Loafer19/vivi-plugins) (packages, helpers, catalog, marketplace and all plugin/helper releases live there).

## Goals

* Keep the app small (well under 100 MB) and the core fast; not every feature is built in.
* Users install, update, roll back and remove plugins from the Extensions window (All / Marketplace tab) without
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

### Source layout of the built-in plugins

Each built-in plugin is `src/plugins/<module>/` (or one file when small) whose module,
struct and manifest ID agree. `mod.rs` holds the plugin itself (state, UI updates,
`Plugin` impl, tests); the rest is plain logic with no Slint:

| Module (ID) | Files |
| --- | --- |
| `auto_tagger` (`auto-tagger`) | `files.rs` tag model, renaming, reading, fingerprints · `providers.rs` ranking, MusicBrainz / AcoustID / Cover Art · `review.rs` backups, tag + SYLT writing, review model, apply |
| `free_music` (`free-music`) | `catalog.rs` track / licence / query model · `api.rs` Internet Archive + Jamendo clients, downloader |
| `lyrics` (`lyrics`) | `lrc.rs` LRC parsing · `sources.rs` embedded / cache / LRCLIB, offsets · `radio.rs` stream-title lookups |
| `network_streams` (`network-streams`) | `sources.rs` radio-browser, SomaFM, iptv-org, M3U · `library.rs` saved, recent, My playlists |
| `subtitle_search` (`subtitle-search`) | `api.rs` OpenSubtitles hash, search, download · `files.rs` offsets, picks, folder pairing |
| `skip_intro`, `epg`, `tagsources` | single files |

Wire formats live in `plugins/protocol.rs`: `PluginCommand` for the UI's
`plugin-command(id, name, args)` callback (arguments packed with U+001F, see `split_args`
/ `join`) and `ScriptMessage`, the one enum of every `script-message vivi-*` a package can
send (`vivi-toast`, `vivi-setting`, `vivi-pick-folder`, `vivi-range`, `vivi-fullscreen`,
`vivi-qr`, `vivi-sub-offset`, `vivi-tagger-acoustid`, `vivi-preview-frames`,
`vivi-chapter-thumbs`). A new message is a variant plus a `parse` arm and a test.
Background lookups return through `util::Task<T>`.

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
an `MdCaps { logos, chips, rich, grid }` saying which parts of the dialog it uses, so the
UI never compares plugin ids (`grid` adds the header's list / grid toggle, its state in
`MusicDialogState.grid`). `TagReview` holds the review dialog's files and
results; `NowPlaying` holds the radio now-playing card (station, track, info, logo). `SkipIntro` holds the Skip button's label and target. Plugin
code writes them with `cx.app.global::<crate::Lyrics>().set_lines(...)`, and
`ui/app.slint` binds the plugin's dialog properties to that global, so a new field
touches the global, the dialog, one binding and the plugin. User actions still reach the plugin through
`plugin-command(id, action, args)`. After this split `AppWindow` itself only carries window
input (`key`, `activity`), `plugin-command` and a few layout values computed from the
window size (for example `karaoke-big` / `karaoke-cols`); everything else is a global.

## Contribution points (where a plugin shows up)

Code: `src/plugins/contrib.rs` (points, `Role`, defaults), `plugins::contributions` /
`plugins::run` (registry), `src/extwin.rs` (Extensions window, sidebar Sources),
`src/menus.rs` (menus). One API for every surface: a plugin declares a **role** and a list
of **contributions**; each surface asks the registry for the entries of enabled plugins at
its point and runs the chosen one.

* **Role**: `source` (a content browser: Network Streams, Free Music, YouTube, YouTube
  Music, …; listed under *Sources* in the Extensions window) or `feature` (the default:
  embedded in existing UI).
* **Contribution**: `point`, `label` (defaults to the plugin name), `command`, optional
  `icon` (an `Icons.plugin` key; defaults to the plugin's icon). Choosing it runs
  `command`: `Plugin::command(cx, command, args)` for built-in plugins,
  `script-message-to <script> <command> [args…]` for packages (use the key of an `action`
  setting so the same action also works from Settings). An empty command opens the
  plugin's tab in the Extensions window.

| Point | Surface | Status | Args |
| --- | --- | --- | --- |
| `sidebar.source` | Library icon rail + Home *Sources* row; user can hide / reorder (gear → Sidebar) | done | none |
| `menu.subtitles` | Subtitle menu (right-click → Subtitle), before *Subtitle Settings…* | done | none |
| `menu.audio` | Audio menu | done | none |
| `menu.video` | Video menu | done | none |
| `context.item` | Right-click on a library file / folder (audio, video, folder kinds) | done | item key (path) |
| `context.player` | Right-click on the player → *Plugins* submenu | done | none |
| `palette.command` | Command palette (Ctrl+K, `src/palette.rs`): fuzzy search over every keymap action, a few app commands, sources (*Open …*), drawer panels and these entries (plugin name as the group) | done | none |
| `drawer.panel` | Player side drawer (`src/drawer.rs`, `ui/drawer.slint`): right slide-out with an icon tab per plugin, hides with the controls, Esc closes; opened from player menu → Plugins → Side drawer or the palette. Lyrics shows its synced lines; other plugins show their description and `action` settings as buttons, the gear opens their settings. `command` is sent when the panel opens | done | none |
| `home.widget` | Card on Home under *Widgets* (icon, `label`, the plugin's description; click runs `command`). Each plugin's gear view has *Show on Home* and *Position on Home* (stored as `ui.home-hidden` in the plugin's settings and `order` in the `vivi-home` entry) | done | none |
| `controlbar.button` | Bottom-bar button: listed under *Available items* in Customize Player after the built-in ones (never in a profile, so off until the user adds it). Stored in `interface.toolbar` as `plug:<plugin id>\|<command>`; it folds into ⋯ like other items and disappears while the plugin is off. Lyrics (karaoke view), Skip Intro (mark), Subtitle Search (search online) | done | none |
| `tray.item` | Tray icon menu (Linux StatusNotifierItem, `src/tray.rs`), between Show / Hide window and Quit; clicking shows the window and runs `command` | done | none |
| `notify` | Desktop notifications: built-ins call `Ctx::notify(title, body)`, packages send `script-message vivi-notify <plugin id> <title> [body]`. Shown through `org.freedesktop.Notifications` on Linux (`src/notify.rs`) only when the plugin declares `notify` and *Settings → Extensions → Desktop notifications from extensions* is on; otherwise (or on Windows / macOS for now) the text shows as a player toast. | done | none |
| `downloads` | The plugin reports jobs to the shared Downloads panel (see *Downloads* below); declaring it lists the panel under *Where it shows up* | done | none |
| `downloads.action` | Button in the Downloads panel header (e.g. *Record stream*, *Find free music*) | done | none |
| `downloads.url` | The Downloads panel's *paste a link* field (first plugin that declares it); `label` is the placeholder | done | the URL |

Unknown points are ignored with a log line, so a manifest written for a newer Vivi still
loads. The Extensions window lists each plugin's points under *Where it shows up*.

Built-in plugins use the manifest builder:

```rust
Manifest::builtin(ID, "YouTube Music", "…", "streams")
    .source("open")                                   // sidebar.source → command "open"
    .contribute(contrib::PALETTE_COMMAND, "YouTube Music: search", "open")
    .contribute(contrib::CONTEXT_PLAYER, "Start radio from this song", "radio")
```

`source(cmd)` sets the role and adds the `sidebar.source` entry; the command usually sets
`MusicDialogState.owner` / `caps` and opens the shared plugin dialog (see Network Streams).
Opened from the Extensions window's *Open* button, the window closes first.

Packages declare the same in `manifest.toml`:

```toml
role = "source"            # optional: source | feature (default feature)

[[contributes]]            # optional, any number
point = "sidebar.source"
label = "YouTube"
command = ""               # "" opens the plugin's Extensions tab

[[contributes]]
point = "menu.subtitles"
label = "Auto-sync subtitles"
command = "sync"           # an action setting key → script-message-to <script> sync
```

**Backward compatibility**: when a package manifest has neither field, Vivi uses
`contrib::known_defaults(id)` for catalog packages released before 0.13 (YouTube is a
source; Subtitle Auto-Sync → `menu.subtitles`; Equalizer → `menu.audio`; Scene Chapters →
`menu.video`; A-B Loop, Cut & Export, AcoustID, SponsorBlock, Offline Downloads →
`context.player`; …), and any other plugin is a feature with no entries (Extensions window
only). A manifest that sets `contributes = []` opts out of the defaults. mpv user scripts
are features.

**Sidebar order / hiding** is host state, not plugin settings: `[plugins.<id>]
settings."ui.hidden" = "true"` hides a source, `[plugins.vivi-sources] settings.order =
"a,b"` orders them (`plugins::arrange_source`).

### Downloads (shared panel)

Library → **Downloads** (icon rail, with a badge counting queued / running / paused jobs)
lists every plugin's downloads: artwork (or the plugin glyph), title, plugin, progress
bar, size / speed / ETA (or the error), and the actions that fit the state: Pause /
Resume (only when the job says `can_pause`), Cancel, Retry, Play now, Open file, Show in
folder, Remove from list, plus *Clear finished*. `src/downloads.rs` owns the list,
saves it to `downloads.json` in the data folder (jobs still running when Vivi quits come
back as failed, "Interrupted when Vivi closed", so Retry works), measures the speed from
the byte counts, sends a notification when a job finishes or fails (the `notify` path:
desktop notification if allowed, else a toast) and puts "2 downloading · 45%" in the
tray tooltip / menu.

A job is identified by (plugin id, job id); the job id is the plugin's own (a URL hash,
a track id …). Report fields (all optional; omitted ones keep their value):

| Field | Meaning |
| --- | --- |
| `title` | Shown in the row |
| `state` | `queued`, `active` (alias `downloading`, `recording`), `paused`, `done`, `failed` (alias `cancelled`) |
| `done`, `total` | Bytes so far / in total (0 = unknown) |
| `progress` | 0..1 when only the share is known |
| `speed` | Bytes per second (else measured by Vivi) |
| `path` | The file (enables Play / Open / Show in folder when done) |
| `artwork` | A local image file for the thumbnail |
| `error` | Shown when failed |
| `detail` | Free text instead of size / ETA (e.g. "Recording · 12:30") |
| `can_pause` | Show Pause / Resume |

* **Built-in plugins** call `crate::downloads::report(ID, job, Report { … })` and
  `crate::downloads::remove(ID, job)`; safe from any plugin callback (the panel refreshes
  from a timer). Free Music reports its queue this way.
* **Packages** send
  `mp.commandv("script-message", "vivi-download", "<plugin id>", "<job id>", utils.format_json({...}))`
  and `vivi-download-remove <plugin id> <job id>`; `vivi-downloads-show` opens the panel.
* **Callbacks**: the panel's buttons call the plugin back with the job id:
  `download-pause`, `download-resume`, `download-cancel`, `download-retry` (a built-in's
  `command(name, [job])`, a package's `script-message-to <script> download-cancel <job>`).
  The plugin then reports the new state. Cancel on a queued job is marked failed
  ("Cancelled") by the host as well. Play / Open / Show in folder / Remove / Clear finished
  are handled by Vivi.

Older Vivi versions ignore the `vivi-download` messages, so a package can send them
unconditionally and keep its own UI as a fallback. Vivi sets the mpv property
`user-data/vivi/downloads` to `1` when the panel exists; Offline Downloads checks it and
only drops its own queue page then.

### Extensions window

`ui/extensions.slint` + global `Extensions` (`ui/state/extensions.slint`), filled by
`src/extwin.rs`. Tabs: *All / Marketplace* (the former Settings → Extensions rows:
Installed / Browse, reusing `settings::extensions`) then one tab per plugin, sources first.
A plugin tab's overview shows the description, load status, *Open* for sources, *Where it
shows up* and its `action` settings as buttons; the gear in the header switches to its
settings (the Settings `Row` component; changes go through `Settings.setting-changed`, so
`plugin.*`, `pkg.*`, `ext.*`, `scripts.*` and `src.*` rows share one handler). Open it
from Rust with `extwin::open(app, id, gear)`; a plugin opens its own settings with
`plugins::run(id, "", &[])`.

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
`stream-logos/` and are decoded through `src/images.rs` at row size (48 px) or, in the grid
view, at tile size (256 px). The view (list / grid) is the plugin's `view` setting. The loading spinner / error toast for network streams is core
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
