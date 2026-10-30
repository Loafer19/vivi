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
| Music Visualizer | Needs a per-frame audio tap (an `af` lavfi filter kept last in mpv's chain, read on a socket), Rust analysis (band levels, onsets, chroma) and a software canvas handed to Slint at ~30 fps; a script could not draw at that rate. |
| Skip Intro & Credits | Draws a Slint button over the video; needs the Rust UI. |
| Lyrics | Karaoke overlay is a Slint component synced to the player clock. |
| Subtitle Search | Lives inside the Add Subtitles dialog (Slint): results with badges, details and preview; picks in `subtitle-picks.json`. The per-file subtitle offset is core (src/suboffset.rs, below), not part of this plugin. |
| Auto-tagger | Preview dialog with candidates and a per-field diff; fingerprints audio and writes tags/covers in Rust (lofty, rusty-chromaprint). |
| Network Streams | Browser dialog with sources, results and details; plays through the player and keeps saved/recent lists. |
| Free Music | Shared music dialog in rich mode (`FmRow` rows with covers, badges, licence chips and downloaded check marks, `MdFilter` dropdowns, infinite scroll, a detail cover; the row model is updated in place); Jamendo + Internet Archive searched in parallel and merged; download queue writes tags with lofty; tracks described with `host::media_meta` (licence on the now-playing card). |

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
| `auto_tagger` (`auto-tagger`) | `files.rs` tag model, renaming, reading, fingerprints · `providers.rs` ranking, MusicBrainz / AcoustID / Cover Art · `review.rs` backups, tag + SYLT writing, review model, apply · `cache.rs` per-file AcoustID answers (path + size + mtime, 300 files, 0600) · `mod.rs` also identifies untagged songs while playing (`auto-identify`) |
| `free_music` (`free-music`) | `catalog.rs` track / licence / query model · `api.rs` Internet Archive + Jamendo clients, downloader |
| `lyrics` (`lyrics`) | `lrc.rs` LRC parsing · `sources.rs` embedded lyrics, cache locations, offsets · `radio.rs` stream-title lookups (titles split by `nowplaying::split_icy_loose`) · `mod.rs` songs from the shared now-playing track (music services, Auto-tagger) |
| `network_streams` (`network-streams`) | `sources.rs` radio-browser, SomaFM, iptv-org, M3U · `library.rs` saved, recent, My playlists |
| `subtitle_search` (`subtitle-search`) | `api.rs` OpenSubtitles hash, search, download · `files.rs` picks, folder pairing |
| `skip_intro`, `epg`, `tagsources` | single files |
| `lrclib` (no plugin) | LRCLIB client for Lyrics and the Auto-tagger (`tagsources`): `/api/get` + `/api/search` with one User-Agent and 12 s timeout, 503 / 429 retried (1 s, 2 s), ranking and `pick` (synced → plain → instrumental), answer cache files (`.lrc` / `.txt` / `.instrumental` 30 days / `.missing` 7 days) |

Wire formats live in `plugins/protocol.rs`: `PluginCommand` for the UI's
`plugin-command(id, name, args)` callback (arguments packed with U+001F, see `split_args`
/ `join`) and `ScriptMessage`, the one enum of every `script-message vivi-*` a package can
send (`vivi-toast`, `vivi-setting`, `vivi-pick-folder`, `vivi-range`, `vivi-fullscreen`,
`vivi-qr`, `vivi-sub-offset`, `vivi-sub-offset-import`, `vivi-tagger-acoustid`, `vivi-preview-frames`,
`vivi-chapter-thumbs`, `vivi-eq-preset`, `vivi-eq-import`). A new message is a variant plus a `parse` arm and a test.
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
`MusicDialogState.grid`). `MusicDialogState.selected` is the highlighted row (reset to -1
when the dialog opens); an owner that re-sorts or replaces its rows sets it so the
highlight, Enter and the footer buttons stay on the same item (Free Music). `TagReview` holds the review dialog's files and
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
| `drawer.panel` | Player side drawer (`src/drawer.rs`, `ui/drawer.slint`): right slide-out with an icon tab per plugin, hides with the controls, Esc closes; opened from player menu → Plugins → Side drawer or the palette. Lyrics shows its synced lines; other plugins show their description (or their live content, `vivi-content`) and `action` settings as buttons, the gear opens their settings. `command` is sent when the panel opens; `vivi-drawer` opens it | done | none |
| `home.widget` | Card on Home under *Widgets* (icon, `label`, the plugin's description; click runs `command`; with `vivi-content <id> home` a taller card with a value line, bars and rows). Each plugin's gear view has *Show on Home* and *Position on Home* (stored as `ui.home-hidden` in the plugin's settings and `order` in the `vivi-home` entry) | done | none |
| `controlbar.button` | Bottom-bar button: listed under *Available items* in Customize Player after the built-in ones (never in a profile, so off until the user adds it). Stored in `interface.toolbar` as `plug:<plugin id>\|<command>`; it folds into ⋯ like other items and disappears while the plugin is off. Lyrics (karaoke view), Skip Intro (mark), Subtitle Search (search online) | done | none |
| `tray.item` | Tray icon menu (Linux StatusNotifierItem, `src/tray.rs`), between Show / Hide window and Quit; clicking shows the window and runs `command` | done | none |
| `notify` | Desktop notifications: built-ins call `Ctx::notify(title, body)`, packages send `script-message vivi-notify <plugin id> <title> [body]`. Shown through `org.freedesktop.Notifications` on Linux (`src/notify.rs`) only when the plugin declares `notify` and *Settings → Extensions → Desktop notifications from extensions* is on; otherwise (or on Windows / macOS for now) the text shows as a player toast. | done | none |
| `downloads` | The plugin reports jobs to the shared Downloads panel (see *Downloads* below); declaring it lists the panel under *Where it shows up* | done | none |
| `downloads.action` | Button in the Downloads panel header (e.g. *Record stream*, *Find free music*) | done | none |
| `streams.music` | A service tab on the Streams hub's *Music* tab (`src/streams/`): the service switcher lists every enabled plugin with this point (Free Music built in, YouTube Music and other music packages); choosing it runs `command`, which opens the plugin's dialog as the tab's page | done | none |
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

### Plugin dialogs as Library pages (Streams hub, #86)

The shared plugin dialog (`MusicDialogState`, `ui/plugins.slint` `MusicBody`) is drawn either
as a modal or as a full-width Library page. Core (`src/streams/mod.rs`, `open_dialog`)
decides when a plugin opens it (`set_md_open` in built-ins, `vivi-dialog … "open": true` for
packages):

* **Page** when the owner is a *source* (`sidebar.source`), a `streams.music` service or the
  radio / TV directory (Network Streams): `MusicDialogState.page` is set and the Library shows
  the dialog body full width with the now-playing card. A page has no title row of its own
  (the dialog title, back arrow and close X are modal only; the list / grid toggle moves to the
  end of the search row): the Library header's breadcrumb (*Streams ▸ <service>*) and back
  arrow cover it, both on the service's own page (reached from its sidebar icon) and inside
  the hub on the Radio / TV / Music tabs. If the user is elsewhere (the player, another section) the Library opens at that
  plugin's place: `service:<id>`, or `streams:music:<id>` / `streams:radio` / `streams:tv`
  when the hub was what asked for it.
* **Modal** otherwise (Auto-tagger review, a package's settings or info dialog), unchanged.

Nothing changes for plugins: the same fields, rows, filters and commands; `close` is sent when
the user leaves the page (Back, another section, the Library closed) like when the modal was
dismissed. Esc in a page goes back to the hub instead of closing an overlay. Places use one
syntax (`src/streams/route.rs`): `streams[:recent|favorites|radio|tv|music[:<id>]|url]` and
`service:<id>`; `VIVI_PAGE=<place>`, the control socket's `page` command and the palette
(*Streams: …*) open them; the socket's `url_history` shows the URL bar with its history list. The directory gets `hub <tab> [query]` and `filter <name> <value>`
(the hub's Radio filters: country, genre, language, bitrate, sort; TV: country, group, guide).
Favorites, Recent and the URL history are core (`src/streams/store.rs`, below), so every card
has a working ★ even with Network Streams turned off; then the Radio and TV tabs are hidden.

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
are features. `contrib::MOVED` lists entries that moved to another plugin (Scene Chapters'
*Detect scene chapters* → Chapters & Previews' `scenes-detect`): while both are enabled
only the new one is shown.

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
| `subtitle` | Shown before the state on the second line (e.g. "Artist · Album") |
| `state` | `queued`, `active` (alias `downloading`, `recording`), `paused`, `done`, `failed` (alias `error`), `cancelled` (alias `canceled`: stopped by the user, shown as "Cancelled" with Retry, no notification; Vivi 0.12 and older read it as failed) |
| `done`, `total` | Bytes so far / in total (0 = unknown) |
| `progress` | 0..1 when only the share is known |
| `percent` | 0..100, used when `progress` is not given |
| `speed` | Bytes per second (else measured by Vivi) |
| `eta` | Seconds left (else estimated from the speed) |
| `path` | The file (enables Play / Open / Show in folder when done) |
| `artwork` | The thumbnail: a local image file, or an http(s) URL (fetched into the plugin cover cache, so a row has its cover before the file exists) |
| `error` | Shown when failed |
| `detail` | Free text instead of size / ETA (e.g. "Recording · 12:30") |
| `can_pause` | Show Pause / Resume |
| `parent` | Job id of a group row of the same plugin (e.g. the playlist this track belongs to) |
| `items_done`, `items_total` | A group row's counts ("12 / 40"); without them Vivi counts the group's rows |
| `done_title` | Heading of the notification when the job finishes, instead of "Download finished" (e.g. `"Export finished"` → "Export finished: clip.mp4"); failures still say "Download failed". Vivi 0.13.1 and older ignore it |

Numbers may be fractional (whole-number fields are rounded, negative ones read 0) or
numeric strings; a field that is `null` or not a number is ignored, the rest of the report
still applies. Vivi 0.12 and older drop a whole report that has a fractional number, so
send whole numbers for those.

**Groups** (playlist / album downloads): a job whose `parent` names another job of the
same plugin is drawn indented under it. The group row shows "12 / 40" (its
`items_done` / `items_total`, or its rows done / all), "· 2 failed", progress by items,
and is active while any of its items runs; a chevron shows or hides all its items
(collapsed, only the item downloading now is shown, so a 1000-track playlist stays one
row). Items finish without a notification; the group's own `done` / `failed` notifies.
Remove on the group removes its items; Cancel / Retry on it call the plugin with the
group's job id. Group rows are not counted in the badge / tray line. Older Vivi shows
items and group as separate rows.

* **Built-in plugins** call `crate::downloads::report(ID, job, Report { … })` and
  `crate::downloads::remove(ID, job)`; safe from any plugin callback (the panel refreshes
  from a timer). Free Music reports its queue this way.
* **Packages** send
  `mp.commandv("script-message", "vivi-download", "<plugin id>", "<job id>", utils.format_json({...}))`
  and `vivi-download-remove <plugin id> <job id>`; `vivi-downloads-show` opens the panel.
* **Callbacks**: the panel's buttons call the plugin back with the job id:
  `download-pause`, `download-resume`, `download-cancel`, `download-retry` (a built-in's
  `command(name, [job])`, a package's `script-message-to <script> download-cancel <job>`).
  The plugin then reports the new state (`cancelled` after a Cancel). Cancel on a queued
  job is marked cancelled by the host as well. Play / Open / Show in folder / Remove / Clear finished
  are handled by Vivi.

Older Vivi versions ignore the `vivi-download` messages, so a package can send them
unconditionally and keep its own UI as a fallback. Vivi sets the mpv property
`user-data/vivi/downloads` to the integer `1` when the panel exists; Offline Downloads checks it and
only drops its own queue page then. Read it with
`tostring(mp.get_property_native("user-data/vivi/downloads", "")) == "1"`: earlier builds
set it as a string node, which `mp.get_property` returns quoted (`"1"`).

### Host API for packages (dialogs, covers, now playing, private files)

Code: `src/plugins/host.rs`, messages in `src/plugins/protocol.rs`. Scripts cannot draw
Slint, so Vivi draws for them: a package sends `script-message` broadcasts and gets the
user's answers back. All payloads are JSON (mpv Lua: `utils.format_json`); unknown
fields are ignored and every field is optional, so packages written for a newer Vivi
still load. Packages that never send these messages are unaffected.

| Message | Effect |
| --- | --- |
| `vivi-dialog <id> <json>` | Open (`"open": true`) or update the shared music dialog owned by `id`. Fields: `info_only` (feature `info-dialog`: a read-only dialog with no search row and no per-row play / queue / download; Enter and double-click only select, footer buttons with an empty label are hidden; kept on updates, reset when another package opens the dialog), `title`, `placeholder`, `search_label`, `query`, `sources` + `source` (dropdown), `chips` + `chip`, `actions` (buttons), `filters` (`[{name, options, value}]` dropdowns), `rows`, `status`, `busy`, `detail` (side pane text), `detail_cover` (large cover above it, URL or path; a `detail` without it hides the cover), `primary` / `secondary` (button labels), `has_more` (infinite scroll), `grid_toggle` (offer list / grid), `grid`, `copy`. A row is `{title, sub, badge, chip, chip_ok, dur, cover, dl, saved, selected, bar}` (`saved`: a check mark for an item downloaded already; `bar`, feature `bars`: a native 0–1 bar under the title, e.g. a share of listening time); `cover` is an http(s) URL or a local path, downloaded into the image cache (`<cache>/plugin-covers`) and decoded at row (48 px) or tile (256 px) size. The grid shows square cover tiles. Updates for a dialog that another plugin took over are dropped until the next `open`. |
| `vivi-dialog-close <id>` | Close it if `id` still owns it. |
| `vivi-content <id> [home\|drawer\|all] <json>` | Live content (feature `content`) of the package's Home widget (`home`) and/or drawer tab (`drawer`; no surface = `all`). Fields: `title` (replaces the contribution label), `value` (a large line, e.g. "1 h 05 min today"), `sub` (a line under it; on Home it replaces the description), `progress` (0–1 bar under them; negative removes it), `rows` (`[{label, value, sub, bar, heading}]`: `bar` 0–1 draws a native bar between label and value, `heading: true` makes a section title). Fields left out keep their last value; `"replace": true` starts from empty. Home shows the first 8 rows, the drawer up to 60. Kept for the session (until `vivi-content-clear` or quit); the widget / tab redraws at once when shown, so send updates only when something changed. Numbers out of 0–1 are clamped, NaN dropped. Code: `src/plugins/content.rs`. |
| `vivi-content-clear <id> [home\|drawer\|all]` | Drop that content: the widget and tab show the static description again. |
| `vivi-drawer <id> [open\|close\|toggle]` | (Feature `drawer`.) Open (default), close or toggle the player side drawer on the package's own `drawer.panel` tab. Ignored for a package without one. The drawer shows while media plays (it stays flagged open otherwise). |
| `vivi-indicator <id> <key> <json>` | (Feature `indicators`.) Show or update a top-bar status chip `{icon, tip, label}`: `icon` is a Vivi icon name (drawn in the accent colour; unknown names fall back to `extension`), `tip` the tooltip, `label` up to 12 characters after the icon (e.g. "TV"). Up to 3 per package, shown while the package is loaded. A click sends `{"event":"indicator","args":[key]}` on `vivi-ui`. Code: `src/status.rs`. |
| `vivi-indicator-clear <id> [key]` | Remove that chip, or every chip of the package. |
| `vivi-sleep <spec>` | (Feature `sleep`.) Start or cancel the shared sleep timer: `off`, `min:<n>`, `eof`, `chapter`, `items:<n>`. It fades the volume out over `playback.sleep_fade` seconds, pauses and restores the volume. The state is in `user-data/vivi/sleep` (JSON `{mode, active, remaining, items_left, fade}`). Code: `src/sleep.rs`. |
| `vivi-confirm <id> <key> <json>` | Warning / confirm dialog `{title, heading, text, ok}` (Enter confirms, Esc cancels). |
| `vivi-media-meta <id> <json>` | `{url, title, artist, album, cover, info}` or `{"items": [...]}` for URLs the package queued: Vivi shows them on the now-playing card (cover, title, artist · album, `info` · codec · bitrate) while one of them plays as audio, uses the cover in Recent and merges them into `user-data/vivi/now-playing` (below). Optional for that: `album_artist`, `mbid` (MusicBrainz recording id), `duration` (s), `kind` (`music` / `video` / `podcast` / `stream`; e.g. `music` for a music video of a music service) and `service` (else `info`). At most 5000 URLs are kept. |
| `vivi-protect <id> <path>` | `chmod 0600` (0700 for a folder) on a file inside the package's data folder `<data>/plugin-data/<id>` (credentials). Paths outside it are ignored. No-op on Windows. |
| `vivi-wipe <id> <path>` | Overwrite a file in that folder with zeros, then delete it (sign out). |

**Host capabilities.** Vivi sets `user-data/vivi/host-api` to the integer `1` and
`user-data/vivi/host-features` to a space-separated word list of the host API it
understands: `dialog confirm media-meta contributions content drawer bars info-dialog indicators sleep`
(`src/plugins/host.rs`, `FEATURES`). Older Vivi leaves both unset, so a package checks a
word and keeps its old UI otherwise:

```lua
local function host_has(word)
  local f = mp.get_property_native("user-data/vivi/host-features") or ""
  return (" " .. tostring(f) .. " "):find(" " .. word .. " ", 1, true) ~= nil
end
if host_has("content") then
  mp.commandv("script-message", "vivi-content", "playback-stats", "home", utils.format_json({
    value = "1h 05m today", sub = "3-day streak",
    rows = { { label = "Mon", bar = 0.4, value = "26m" }, { label = "Tue", bar = 1, value = "1h 05m" } },
  }))
end
```

**Screenshots and tests of the drawer**: `VIVI_DRAWER=<plugin id>` (debug builds or
`VIVI_SCREENSHOT=1`) opens the drawer on that tab 3 s after start, and the control socket
(`VIVI_IPC=1`) has `{"cmd":"drawer","args":{"plugin":"playback-stats","open":true}}`
(`open`: `true` / `false` / `"toggle"`, no `plugin` = the current or first tab), which
answers `{open, tab, tabs, shown}` (`shown`: open and media playing). Neither needs input
synthesis.

**Now playing** (`src/nowplaying.rs`, Vivi 0.13+): Vivi publishes the playing track,
normalized, as one mpv node map `user-data/vivi/now-playing`, so packages do not each
re-read tags, ICY titles and other packages' metadata. Per field the first non-empty value
wins: the package metadata of the URL (`vivi-media-meta`), the file's tags, the stream
title of Shoutcast / Icecast radio (`icy-title` "Artist - Title", also with an en / em
dash; station `icy-name`),
the library database, then mpv's `media-title`. Keys: `title`, `artist`, `album`,
`album_artist`, `genre` (strings, "" = unknown), `track` (int, 0 = unknown), `duration`
(seconds, 0 = unknown / live), `cover` (http(s) URL from the package, else a `file://`
PNG Vivi renders with ffmpeg; "" until ready), `kind` (`music`, `video` = a real video
track, `podcast` = genre podcast / audiobook or `.m4b`, `stream` = a URL without length
or with ICY titles; "" while idle), `source` (`plugin:<id>`, `tags`, `stream`, `library`
or `file`), `path` (the mpv `path` the values describe), `mbid` (MusicBrainz recording id,
lower case), `service` (music service / station) and `id`, a counter that grows on every
change. The map is replaced whole once a file is loaded and whenever a value changes (a
new ICY title, a cover rendered, metadata sent later); while a new file opens it still
describes the old one, so compare `path` with mpv's `path`. Vivi sets
`user-data/vivi/now-playing-api` to `1`; packages check it and keep their own logic for
older Vivi. Lua: `mp.observe_property("user-data/vivi/now-playing", "native", fn)`;
helpers: `get_property` / `observe_property` over mpv's JSON IPC; the control socket
answers `now_playing`. MPRIS and the now-playing card read the same track.

Answers arrive as `script-message-to <script> vivi-ui <json>` with
`{"event", "args", "query", "source", "chip", "grid"}`; `event` is one of `search`,
`edited` (query typed), `select` / `primary` / `secondary` (`args[0]` = row index),
`act:<action>` (`args[0]` = row index), `chip`, `filter` (`[name, value]`), `more`,
`view` (`grid` / `list`), `close`, and `confirm` (`[key, "yes" | "no"]`). They are
forwarded only to a package that opened a dialog or confirm in this run.

Downloads go to the shared Downloads panel with `vivi-download` (see above).

**Playlists from plugins** (`vivi-playlist <id> <json>`): a package that keeps a Vivi
playlist in sync with an online list (YouTube Music's *Download playlist*) sends
`{name, source, url, entries: [{path, title, duration}], files: [paths], cover}`.
`source` is the plugin's opaque id of the list, `cover` a local image or http(s) URL.
Vivi writes `<data>/playlists/<name>.m3u` (`store::save_owned_playlist`) with the header
`#PLAYLIST:<name>`, `#VIVI-SOURCE:<plugin id> <source>` and `#VIVI-URL:<url>`, then the
entries (`#EXTINF` when a title is given), sets the cover next to it and answers
`script-message-to <script> playlist-saved <source> <path>` (or `playlist-failed <source>
<error>`). The file is chosen so a user's playlist is never overwritten: the playlist that
already carries this source (kept when the user renamed it), else `<name>.m3u`, else
`<name> (<plugin name>).m3u`, `<name> (<plugin name> 2).m3u`, ... Edits to such a playlist
are replaced on the next sync; deleting it ends the sync. The Playlists tab shows the
plugin's name as a badge and in the subtitle, and its menu adds **Sync** (calls
`playlist-sync <source>`) and **Open Source** (`playlist-open <source>`; the URL in the
browser when the plugin is off). Vivi sets `user-data/vivi/playlists` to `1`; on older
builds a package can write the same file format itself (`#VIVI-SOURCE` in the header).

Playback stays in the package: it queues URLs with mpv's
`loadfile <url> append-play -1 <options>` (mpv 0.38+ takes the index; older mpv does not)
and per-file options such as `force-media-title=…` and `ytdl-raw-options=cookies=%N%<file>`
(`%N%` quoting keeps commas and `=` intact). yt-dlp is found in `PATH`, which includes the
YouTube & yt-dlp package's managed copy (`plugin-data/youtube/bin`); Vivi itself never
needs it at startup. Contribution points (`sidebar.source`, `palette.command`,
`context.player`, …) come from `[[contributes]]` in the manifest, see above.

### Skip ranges (one Skip button)

Skip Intro & Credits (`src/plugins/skip_intro.rs`) owns the player's only Skip button. Besides
chapter names and the user's per-show ranges it takes ranges that packages find for the current
file, so a package such as SponsorBlock supplies segments and leaves the prompt, auto-skip, undo
and mute to the core:

```lua
local function core_skip() -- 1 while Skip Intro & Credits is on (0 / unset: do it yourself)
  return tostring(mp.get_property_native("user-data/vivi/skip-api", "")) == "1"
end
mp.commandv("script-message", "vivi-skip-ranges", "sponsorblock", utils.format_json({
  { start = 12.5, ["end"] = 41.0, category = "sponsor", label = "sponsor", action = "skip" },
  { start = 300, ["end"] = 304, category = "selfpromo", label = "self-promotion", action = "mute" },
}))
mp.commandv("script-message", "vivi-skip-ranges-clear", "sponsorblock")
```

* `vivi-skip-ranges <source> <json>` replaces every range of `<source>` (the package id) for the
  file playing when it arrives; ranges of another file are dropped when the next one loads, and
  `[]` or `vivi-skip-ranges-clear <source>` removes them. `chapters` and `show` are reserved.
* Entry fields: `start`, `end` (seconds, `end > start`), `kind` or `category` (`sponsor`,
  `intro`, …; default `segment`), `label` (what the button names: "Skip <label>", "Skipped
  <label> · Undo"; default the kind; a leading "Skip " is dropped), `action`: `skip` (skip
  automatically), `ask` (show the button), `mute` (mute while inside, sound back after;
  "Unmute <label>" on the button), `off` (ignored). Missing `action` follows *Skip
  automatically*. At most 500 entries per source.
* `vivi-skip-undo`: the same as `Alt+Shift+K` (undo the last skip, or unmute).
* Modes: the user's *Per category or source* setting (`intro=skip sponsorblock=ask
  sponsorblock:filler=off`) wins over `action`; most specific first (`source:kind`, a show
  remembered as "always skip", `kind`, `source`, then `action`).
* One button per segment: ranges of the same action that overlap by at least half of the
  shorter one are one segment; per-show ranges win over package ranges, package ranges over
  chapter names. Chapters that start or end exactly where a submitted range does (the
  "Intro" / "End of intro" chapters SponsorBlock writes for the seek bar) are left out of the
  chapter-name detection, and titles such as "End of intro" or "Intro end" never start a
  range.
* `user-data/vivi/skip-api` is the integer `1` while Skip Intro & Credits is on and `0` while
  it is off (unset in Vivi before 0.13.0): packages should then keep their own skipping. With it
  off, `Alt+K` / `Alt+Shift+K` go to mpv like any unbound Alt key.

### Per-file subtitle offset

The subtitle offset is the player's own (`src/suboffset.rs`), with or without Subtitle Search:
`z` / `x` and `Alt+Z` / `Alt+X` move subtitles 0.1 s (`Shift`: 1 s), the side panel's
Subtitles → Offset group has the same buttons, a reset and the delay slider, and the control
socket / MCP have `sub_offset` (`set_subtitle_offset`). The value is mpv's `sub-delay`, saved
per file (canonical path, or the URL) in `$XDG_DATA_HOME/vivi/subtitle-offsets.json` and
applied when the file opens, after a per-show preset.

```lua
local function core_offset() -- 1: Vivi keeps the offset (keys, storage, side panel)
  return tostring(mp.get_property_native("user-data/vivi/sub-offset", "")) == "1"
end
mp.commandv("script-message", "vivi-sub-offset", "-0.1")     -- or "set:2.5", "reset"
mp.commandv("script-message", "vivi-sub-offset-import")      -- hand over offsets.json once
```

* `user-data/vivi/sub-offset` is the integer `1` (unset before Vivi 0.13.0). Vivi also sets the
  script-opt `vivi-sub-offsets=1` for Subtitle Auto-Sync 1.2, which checked that instead.
* `vivi-sub-offset <delta|set:seconds|reset>` changes the playing file's offset and saves it
  (a toast says so); Subtitle Auto-Sync sends `reset` after loading a synced copy.
* Handover: offsets Subtitle Auto-Sync kept itself on older Vivi
  (`plugin-data/subtitle-sync/offsets.json`, `"<path>|<size>": seconds`) are imported at
  startup and on `vivi-sub-offset-import`; entries Vivi already has win, files whose size
  changed are skipped, and `plugin-data/subtitle-sync/offsets.imported` marks it done (the
  package may then delete its file).
* `Alt+Z` / `Alt+X` without media go to mpv, like other unbound Alt keys.

### Extensions window

`ui/extensions.slint` + global `Extensions` (`ui/state/extensions.slint`), filled by
`src/extwin.rs`. Tabs: *All / Marketplace* (the former Settings → Extensions rows:
Installed / Browse, reusing `settings::extensions`) then one tab per plugin, sources first.
A plugin tab's overview shows the description, load status, *Open* for sources, *Where it
shows up* and its `action` settings as buttons; the gear in the header switches to its
settings (the Settings `Row` component; changes go through `Settings.setting-changed`, so
`plugin.*`, `pkg.*`, `ext.*`, `scripts.*` and `src.*` rows share one handler). Open it
from Rust with `extwin::open(app, id, gear)`; a plugin opens its own settings with
`plugins::run(id, "", &[])`. Browse lists *Recommended* plugins first, with a badge, in the
order of the catalog's `featured_rank` (from `store.toml` in vivi-plugins; the marketplace's
Featured block uses the same field): `chapter-thumbnails`, `youtube`, `sponsorblock`,
`night-mode`, `subtitle-sync`, `remote-control`. A catalog without any `featured_rank` falls
back to the same list in `packages::RECOMMENDED`; deprecated plugins are never recommended
(`packages::recommended_rank`, `packages::browse_order`). Scrobbler is offered from
Settings → Privacy instead (it needs an account). Core code that
suggests a package installs it with `packages::install_and_enable(app, id)`: the window
opens on Browse with the normal Install / permission prompt, and the package is enabled
once installed (the seek-bar hint in `src/preview.rs` uses it).

### Music plugins (#63, #65, #69, #70)

| Plugin | Where | Why | Keys |
| --- | --- | --- | --- |
| Auto-tagger (`auto-tagger`, #69 incl. #65) | built in | preview/diff dialog; tag writing and fingerprinting need lofty and Chromaprint, not available to scripts | AcoustID application key (optional, <https://acoustid.org/new-application>); MusicBrainz, Cover Art Archive, Deezer, iTunes Search and LRCLIB need none; Last.fm API key (genres) and Discogs token optional (#79). Candidates from all sources are merged and ranked by confidence (`rank_candidates`, `Candidate::confidence`); disagreeing values become per-field alternatives (⇄ in the review); fingerprint matches propose a sanitized, opt-in file name (`sanitize_file_name`, `rename_to`). AcoustID answers are cached per file (`cache.rs`, `tagger-cache.json`); the key goes in a POST body and is masked in errors. *Identify untagged songs while playing* (0.13, off by default) watches the shared now-playing track: kind `music` + source `file` (no tags, no library row, no package metadata) starts a low-priority background lookup after 3 s; a match scoring ≥ 70 is published like `vivi-media-meta` from `auto-tagger` (plus mpv `force-media-title` for the file) with a *Review tags* toast; tags are only written from the review. The review shows the source of every field; synced LRCLIB lyrics are embedded when *Embed synced lyrics* is on (MP3 SYLT+USLT, FLAC/Ogg/Opus `LYRICS`, M4A `©lyr`, #80) |
| Equalizer (`equalizer`, #74) | script, **deprecated** since 0.13.0 | presets via mpv `equalizer`/`superequalizer` af; replaced by the built-in Equalizer (`Alt+E` / `Alt+Shift+E`, auto pre-amp) | none |
| Subtitle sync (`subtitle-sync`, #72) | script | alass (system or managed download) + managed media ffmpeg | none |
| Chapters & Previews (`chapter-thumbnails`, #75, #62) | script + core | managed media ffmpeg; core reads the cache (the only source of seek-bar previews and chapter pictures; offered by a hint on the first seek-bar hover and marked *Recommended* in Browse); since 1.2.0 also scene chapters (`scenes-detect`, Video menu / palette via `[[contributes]]`) | none |
| AcoustID (`acoustid`, #76) | script + tagger, **deprecated** (1.1.1, `builtin:auto-tagger`) | fpcalc (system or Chromaprint download); results open in the tagger review with a rename proposal; its auto-identify and answer cache are built into the Auto-tagger since 0.13 | AcoustID application key |
| Scrobbler (`scrobbler`, #77) | script | Last.fm + ListenBrainz | Last.fm API key + secret, ListenBrainz token |
| Free Music (`free-music`, #70) | built in | search/results/licence dialog; downloads are tagged with lofty | Jamendo `client_id` (optional, <https://devportal.jamendo.com>); Internet Archive needs none |
| Subtitle Auto-Sync (`subtitle-sync`, #56) | package (mpv Lua) | runs an external tool and `sub-add`s the result; no UI | none; needs `alass`/`alass-cli` or `ffsubsync` in `PATH` |
| SponsorBlock (`sponsorblock`, #64) | package (mpv Lua) | HTTP via `curl`, seeks and chapters; "ask" mode uses a toast + `Alt+K` instead of a Slint button | none |
| Scene Chapters (`scene-chapters`, #62) | package (mpv Lua), **deprecated** (→ `plugin:chapter-thumbnails`) | ffmpeg subprocess, `chapter-list`; 1.1.1 steps aside when Chapters & Previews 1.2 is loaded (`scene-chapters-superseded`), and its Video menu / palette entries are hidden then (`contrib::MOVED`) | none; needs `ffmpeg` |
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
`group-title`, `tvg-country`; relative entries skipped), and the Streams hub's lists (below).
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
error). No key. Later: podcasts (RSS).

Favorites, recent streams and the URL history are core since #86 (`src/streams/store.rs`):
`favorites.json`, `history.json` (recent, 50) and `urls.json` (typed / pasted URLs, 20) under
`$XDG_DATA_HOME/vivi/streams/`, each entry with a `kind` (radio, tv, music, web) and the
`service` that played it. On first run they are filled once from Network Streams' `saved.json` /
`recent.json` and the URLs of the global Recent list; the old files are left in place (older
Vivi still reads them) and not written again.

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
secrets_file = true               # optional: secret settings via a private file (0.13+)

[[settings]]                      # optional, shown in Settings → Extensions
key = "strength"
title = "Strength"
type = "choice"                   # toggle | choice | text | secret | action
options = ["Medium", "Light", "Strong"]
default = "Medium"
```

Settings become mpv `script-opts` (`<id>-<key>=<value>`; toggles as `yes`/`no`) that the
script reads with `mp.options.read_options(o, "<id>", on_update)`. Changes in Settings are
applied live with `change-list script-opts append`. Script-opts keys are global and
mp.options takes every `<identifier>-*` key, so a package whose id has dashes also gets
them as `<id with _>-<key>` (`youtube_music-folder`), and the dashed form is left out
when another script's id plus `-` is a prefix of the package id (`youtube` would
otherwise read `youtube-music-folder` as its own `music-folder`). New packages should
read `mp.options.read_options(o, ("<id>"):gsub("-", "_"), on_update)` (Vivi 0.13+). An `action` setting sends
`script-message-to <script> <key>` (mpv names the script after the entry file with `-`
replaced by `_`, e.g. `ab_loop`).

**Secret settings** (`type = "secret"`): mpv writes every `script-opts` change to its
log with the value (`change-list` at debug level, property writes at verbose level, and a
`--log-file` always records both), so tokens there end up in Vivi's log file. A package
that sets `secrets_file = true` in its manifest (Vivi 0.13+) gets none of its `secret`
settings as script-opts. Vivi writes them as a JSON object `{"<key>": "<value>", …}` to
`<data>/plugin-data/<id>/vivi-secrets.json` (0600, folder 0700; written through a
temp file created 0600) and sets the one script-opt `<id>-secrets=<revision>:<path>`
instead. The package declares a `secrets = ""` option, reads the file at start and
again whenever that option changes (`on_update`), and on an older Vivi (option empty)
uses the script-opts values as before. `pkgdir` and `secrets` are reserved keys.
`config.toml`, which holds the values, is written 0600 as well.

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
| `vivi-toast <text> [seconds]` | toast on every screen (player, library, Settings, Extensions, dialogs); `seconds` is honored within 1.5–15 s (default 4; 0.13+, older Vivi shows 4 s) |
| `vivi-preview-frames <dir>` | load cached seek-bar frames `seek-NN.jpg` from `<dir>` (0.9.2) |
| `vivi-chapter-thumbs <dir>` | reload chapter thumbnails `chapter-N.jpg` from `<dir>` (0.9.2) |
| `vivi-tagger-acoustid <path> <json>` | open the auto-tagger review for `<path>` with AcoustID ids and a rename pattern (0.9.2); since 0.13 the answer is also stored in the Auto-tagger's per-file cache |
| `vivi-setting <id> <key> <value>` | saves one of the package's declared settings (e.g. a generated PIN) and applies it as script-opt |
| `vivi-pick-folder <id> <key>` | folder dialog; the result is saved as that setting and sent back as `script-message-to <script> folder-picked <path>` |
| `vivi-qr <title> <text> [caption]` | dialog with `text` as a QR code (e.g. a pairing link), shown above Settings (0.11.0+) |
| `vivi-fullscreen [toggle\|on\|off]` | switches Vivi's window fullscreen (0.11.0+) |

**Repeat and shuffle** (after 0.12.0, `src/loopsync.rs`): Vivi's repeat button follows mpv's
`loop-file` whoever sets it (mpv keys, scripts, IPC), so a package can just set `loop-file` /
`loop-playlist`. mpv has no shuffle state, so Vivi publishes it as the string property
`user-data/vivi/shuffle` (`yes` / `no`); setting it asks Vivi to shuffle or unshuffle the queue
and its button follows. A package checks that the property exists before offering repeat /
shuffle (older Vivi: the buttons would not follow).

Vivi sends `script-message vivi-preview-wanted` on the first seek-bar hover of each local
video while Chapter Thumbnails is enabled (its *On hover* mode starts rendering then).

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
* **Uninstall**: removes `<id>/`. From Extensions → Installed, settings and data stay for a
  reinstall; the plugin's own page (Extensions window, *Uninstall…*) also removes its
  `config.toml` settings and `plugin-data/<id>/` unless *Keep its data* is checked.
* **Enable/disable**: the same ON/OFF toggle as other plugins. A fresh install is loaded
  right away; mpv cannot unload scripts, so disable, update, rollback and uninstall take
  effect after a restart (the UI says so).
* **Compatibility**: entries whose `min_vivi` is newer than the app show "Needs Vivi x".
* **Deprecation**: a catalog entry (from the manifest's `[deprecated]` table) may carry
  `"deprecated": {"since": "0.13.0", "message": "…", "replacement": "builtin:equalizer",
  "action": "Open Equalizer"}`. Vivi then hides it from *Available plugins* unless it is
  installed, shows the message in its Extensions tab with a button that opens the
  replacement (`builtin:equalizer` = Audio panel → Equalizer; Vivi sets
  `user-data/vivi/equalizer` to 1 so the old package stops applying its own filter, and the
  package hands its preset over with `vivi-eq-preset <name>` or `vivi-eq-import <json>`
  (the *Export* presets format; added, the first selected, the equalizer turned on),
  `builtin:ab-loop` = the
  keyboard shortcut list (A-B loop keys, `src/abloop.rs`; Vivi sets `user-data/vivi/ab-loop`
  to 1 so the old package stays quiet), `builtin:auto-tagger` = the Auto-tagger's settings in
  the Extensions window (AcoustID package), `plugin:<id>` = that plugin's tab), and once (ids remembered in `~/.config/vivi/deprecation-notices.txt`) a
  toast with the same button when a file plays.

## Future work

* `kind = "wasm"` via Extism with a declarative UI host API; then Lyrics and Subtitle
  Search can move out of the binary.
* minisign signatures on index entries.
* Third-party catalogs (several index URLs).
