# Changelog

## [Unreleased]

### Added
- **Icons for spots that had none**: Rotate 90° in the Video menu (new two-tone rotate glyph, like the other menu rows); single-tone icons on the text buttons Add bookmark, Add files (empty queue), Open file / Open stream / Add folders (empty library), Check for updates and Copy system info (new copy glyph) in About.
- **Volume control styles**: Customize → Volume offers four styles, each chip with a miniature preview:
  - **Cone**: the wedge, as before and still the default.
  - **Flat bar**: a slider drawn like the seek track of the current bar style.
  - **Pop-up slider**: the speaker icon only; hovering it opens a vertical slider, the wheel changes the volume and a click mutes.
  - **Icon + percent**: the number next to the icon; drag sideways or scroll to change it.

  All styles are one "Volume" slider for screen readers (value, 5 % steps, increment and decrement). The style is stored as `[interface] volume_style` (case-insensitive; unknown values fall back to Cone) and is part of the Customize profile.
- **Keyboard shortcuts overlay**: press `?` (or main menu → Keyboard Shortcuts) for every shortcut in two columns; Esc or `?` closes it.
- One keymap (`src/keymap.rs`) drives the key handler, Settings → Controls → Keyboard Shortcuts, the `?` overlay, onboarding and the menu hints, so the lists can no longer drift. The list now includes S (stop), N / Shift+N, R (record), Ctrl+\ (hide UI), Ctrl+Q, V, P and `?`; a test keeps `docs/features.md` in sync.
- Toasts when recording starts / stops (with the file path) and when Ctrl+\ hides the interface (saying how to bring it back).

- **Accessibility roles and names**: toggles are switches with their checked state, dropdowns combo boxes with their value, segmented controls tab lists, sliders expose value / range / step with increment and decrement actions, and list rows (menus, library, queue, presets, options) are list items with their title; settings rows and effect sliders pass their label. Slint's `accessibility` feature is now enabled (AccessKit; on Linux AT-SPI over D-Bus via zbus, already a dependency), so screen readers see them; no new system libraries are needed.
- **Keyboard focus in dialogs**: Tab / Shift+Tab move between buttons, toggles, dropdowns and text fields, Space or Enter activates the focused one, and it shows a 2px accent focus ring (`Theme.focus-ring`, `Metrics.focus-ring-w`). Clicking does not move focus, and in the main window (no dialog open) Tab stays a player key; the window chrome is not in the Tab order.
- Tooltips on the top bar's Library, Settings (Ctrl+,) and Fullscreen (F) buttons, like the rest of the icon buttons.
- **Customize shows labelled tiles**: every item in the bottom bar and in Available items has its name under the icon (short captions such as "Back 10s"; the tooltip keeps the full name).
- **Customize rows fade at their scrolling edges**: the bottom-bar and Available items rows fade out on whichever side has more items, so it is clear they scroll.
- **Drag to re-order the bottom bar in Customize**: drag a placed tile left or right; an accent marker shows where it will land. The ‹ › buttons still work (and remain the keyboard path).
- **Progress-bar styles show a preview in Customize**: each style chip draws a miniature of its bar (Compact, Classic, Waveform, Plain); the profile and style chips have screen-reader names.
- **Empty states offer the next step**: an empty library shows Open file, Open stream and Add folders (opens Settings → Library); empty Bookmarks and Queue tabs show Add bookmark / Add files buttons.
- **Failed streams can be retried or copied**: the "Could not open …" banner has Retry (re-opens the same URL) and Copy (message and URL to the clipboard), and stays up 12 s instead of 6 s so the buttons can be used.
- **Every click area has a screen-reader name**: side-panel tabs, queue/playlist/library rows, offset nudges, speed chips, the library rail and header icons, Settings categories and swatches, onboarding, dialogs, the mini player, plugin dialogs (subtitles, music, tag review, lyrics, karaoke) and the filters; the seek bar, volume and equalizer bands are sliders with values and +/- actions. Icon-only library rail and header buttons, the mini-player queue toggle and the panel list +/− buttons gain hover tooltips.
- **Recent remembers what each entry is**: entries now store a type (video, audio, folder, playlist, stream, web video, Free Music) and the title when it is known (station name, track title and artist, page/ICY title); Open Folder and loading a saved playlist add the folder / playlist itself. The Recent view and the home "Recently Played" shelf show that title (the view adds a type label and the folder or host on the second line) with a folder / playlist / stream icon; streams, web videos and folders now appear on the home shelf too. Reopening moves an entry to the top; the list keeps 50 entries (was 12). The old `recent.txt` is migrated to `recent.json` on first start.
- **Open Recent menu and removing single entries**: the main menu has an Open Recent submenu with the ten newest entries, each with its type icon and label on the right, plus Clear Recent. A Recent entry can be removed with "Remove from Recent" in its context menu or with the × that appears on hover (rows and grid tiles). Rows on the home "Recently Played" shelf and in the Recent view show a small type badge, and grid tiles show it in the thumbnail corner. Stations opened from Network Streams keep their logo, and Free Music tracks keep their cover; both show up as the tile or row picture once the image is cached.
- **Recent pictures and icons**: when a web video (YouTube, Vimeo, …) is opened, its thumbnail is fetched in the background and used for its Recent tile and row. YouTube thumbnails come straight from YouTube; for other sites the thumbnail comes from yt-dlp. Thumbnails are cached in `~/.cache/vivi/web-thumbs`. Recent grid tiles without a picture show the entry's own icon (stream, folder, playlist, link for web videos) instead of the generic video icon, and web videos use the link icon in the Open Recent menu too. "Clear" on the home "Recently Played" shelf now updates the home screen and the Recent view right away.
- **Recent in the side panel**: the side panel's Library tab lists the five newest Recent entries (icon, title and type badge) above the search results while the search box is empty; clicking one opens it.
### Changed
- **One image cache for the whole app**: library and Home thumbnails, Recent pictures, station logos, Free Music covers, chapter thumbnails and playlist covers now go through one cache (`src/images.rs`). Each picture is decoded once per size on a background thread (the type icon shows until it is ready), shrunk to the size it is shown at (256 px tiles, 48 px rows), and kept in memory up to 40 MB, least recently used first. Shrunk copies are stored in `~/.cache/vivi/thumbs/pics`, so the next start reads a small PNG instead of the original. Image caches on disk (thumbnails, covers, station logos, web thumbnails) are kept under 200 MB, oldest first. Settings → Library → **Clear image cache** shows how much they use and also empties the memory cache (it was "Clear thumbnail cache").
- **Smaller network cache and fewer mpv threads**: network streams read ahead up to 48 MiB and keep 16 MiB for seeking back (mpv's defaults are 150 MiB and 50 MiB), set with `[playback] cache_mib` and `cache_back_mib` in `config.toml`. mpv's built-in stats, console, select, positioning and commands scripts are no longer loaded (vivi has its own UI and no mpv key bindings); that is five Lua states and threads fewer. The yt-dlp hook still loads.
- **Seek previews start on the first hover**: opening a video no longer starts 50 `ffmpeg` runs (three at a time) right away. Nothing runs until the pointer first goes over the seek bar; then a single worker renders the frames one at a time, coarse to fine, so a few spread-out frames are ready within the first second. Chapter Thumbnails caches are still used directly.
- **Library startup**: Home is built once from the stored index instead of twice (once at startup and again after the library rescan, decoding every thumbnail and picture again). The folder check now runs 3 s after startup in the background without the scanning indicator; it only stats files (tags are read for new or changed files) and rebuilds Home only when something was added, changed or removed. Rescan and adding a folder work as before.
- **Hardware decoding loads only the interop for the GPU vivi draws on**: libmpv's render API loaded every interop (VA-API, CUDA, VDPAU, ...) when the video output started, so on hybrid laptops it loaded libcuda and could wake the NVIDIA GPU while the window rendered on the integrated one. vivi now reads the OpenGL vendor and sets `gpu-hwdec-interop` to VA-API on Mesa (AMD, Intel, nouveau) or CUDA on NVIDIA; an explicit `hwdec` (nvdec, vaapi) picks its own interop. `[playback] hwdec_interop` in `config.toml` overrides it (`auto` = pick).
- **Faster startup**: checking whether yt-dlp and ffmpeg are installed no longer runs them (`yt-dlp --version` alone held the window back 0.2-0.4 s while Python started); vivi looks them up in `PATH` instead. Time to the first window on the test box dropped from about 0.55-0.69 s to 0.27-0.30 s.
- **Karaoke**:
  - **The current line no longer jumps to two lines when it grows.** It uses the large size only when it fits on one line, otherwise the largest size that does, down to the size of the other lines. Only a line too long even at that size wraps, and the space for it is the same for every line, so nothing shifts when the line changes.
  - **Auto-hide:** the karaoke header (offset, Search, Save, close) and the shortcut hint hide with the player controls, after the same timeout, and the mouse cursor hides too. Moving the mouse or pressing a key brings them back.
- **Faster CI**: the CI build uses a new `ci` profile (release optimizations without fat LTO, 16 codegen units, no incremental or debug info) and links with mold. Rust caching is shared between jobs and kept when a job fails. The swap step was dropped from CI because the build no longer needs it. Release and packaging builds keep fat LTO. CI now also runs `cargo test`, using the same `ci` profile (which unwinds on panic, so build and tests share compiled dependencies).
- CI, Release and Packaging jobs that build with the fat-LTO release profile on Linux add a 10 GB swapfile first; with accessibility enabled the AUR vivi-git build was killed for lack of memory.
- **Esc closes the top-most open overlay** — menus, shortcuts, info, onboarding, Customize, tag review, Find Lyrics, music / streams dialog, Add Subtitles, QR, Settings, New Playlist, Network Stream, side panel, library, karaoke — and only then leaves the miniplayer, then fullscreen. It also works while a text field in a dialog has focus.
- **Consistent UI**: every colour, corner radius and font size in the UI now comes from design tokens (`ui/core/theme.slint`: `Theme` status / scrim / shadow / on-media / stage colours and a `Metrics` scale for spacing, radii, fonts, icons and row heights). Odd values are snapped to the scale (e.g. 15 px text → 16 px, 5 px corners → 6 px).
- One set of shared components in `ui/core/`: Primary / Secondary / Flat / Icon buttons with size variants, section headers, badges, value chips, list rows, one slider, one modal (same scrim darkness, rounding, border and shadow for Settings, Customize, Properties, the plugin dialogs and the playlist / stream dialogs) and the text field (now also in the side panel and the library search). One `Icons` lookup replaces five, one `Format` replaces two.
- Menu, dropdown option, visualizer, preset, queue and library rows are built on the shared `ListRow`; onboarding uses the shared `Modal`; the dropdown lives in `ui/core/dropdown.slint`.
- Typed UI state: `BarStyle` (with per-style `BarMetrics` for the control bar geometry), `PanelTab`, `Overlay` (the Esc target is now computed in Slint as `top-overlay`) and `TagStatus` replace string and number comparisons. `bar_style` in config.toml is now matched case-insensitively (`classic` works); unknown values still fall back to Compact.
- UI state moved from one large `AppWindow` into per-feature Slint globals in `ui/state/` that Rust reaches with `app.global::<X>()`: `Overlays` (which overlays are open), `Playback` (title, play state, time, volume, repeat / shuffle, A-B loop, seekbar extras), `Panel` (side panel lists, quick-setting values and actions), `Menus`, `Dialogs` (network stream, New Playlist, QR, Properties), `Library` (home sections, browsed folder, sort / grid, search and scan state, recent files), `Settings` (settings page, shortcuts overlay, accents, onboarding), `Customize` (control bar layout, style and profiles; `bar-action` now lives on `Playback`), `Lyrics` (lyrics view, karaoke words, manual search; the `lyrics-` / `ls-` prefixes are dropped, search fields are `search-*`), `Subtitles` (Add Subtitles dialog, sync / pick state; `subs-` prefix dropped), `MusicDialogState` (shared music plugin dialog; `md-` prefix dropped, the `md-owner == "…"` checks became an `MdCaps { logos, chips, rich }` capability struct that the owning plugin sets), `TagReview` (auto-tagger review; `tr-` prefix dropped), `NowPlaying` (radio now-playing card; replaces the `np-*` properties), `Notices` (toast / resume prompt, stream loading spinner, stream error), `Viz` (music visualizer frame, note and modes; `viz-` prefix dropped), `SkipIntro` (Skip button label and target). The transport callbacks (play / pause, seek, volume, prev / next, stop, record, A-B, fullscreen, miniplayer, open file / URL, queue sort) live on `Playback`; `AppWindow` keeps only window input (`key`, `activity`) and `plugin-command`. `main.rs` is being split into `src/app/`: key translation for mpv (`app/keys.rs`) and the screenshot / test hooks (`app/debug.rs`), the side panel (`app/panel.rs`) and media loading / queue / resume (`app/session.rs`); `main()` is carved into `app/wiring/` (player events, transport, keys, auto-hide, window events) and `app/startup.rs` (environment, backend, feature setup), leaving `main.rs` at ~100 lines. New Playlist names now use `new-playlist-*` instead of `np-*`, which clashed with the now-playing card.
- Settings schema (`src/settings/schema.rs`): each stored setting id maps to one getter and one setter on `Config`, used by both the settings rows and the `setting-changed` handler; General is fully mapped (getter + setter); 50 more ids across the other categories store through schema setters, and ids needing more than an assignment (file pickers, rescans, presets) stay in `settings.rs`.
- Big plugins become folders: Free Music is `plugins/free_music/{mod,catalog,api}.rs` (plugin and UI, track / licence / query model, Internet Archive + Jamendo clients and downloader); the Auto-tagger is `plugins/auto_tagger/{mod,files,providers,review}.rs` (plugin and UI; tag model, renaming, tag reading and fingerprinting; candidate ranking and MusicBrainz / AcoustID / Cover Art lookups; backups, tag + SYLT writing, cover options, review model, apply and folder scan); Network Streams is `plugins/network_streams/{mod,sources,library}.rs` (plugin and UI; radio-browser.info / SomaFM / iptv-org / M3U sources; saved, recent and "My playlists"); Lyrics is `plugins/lyrics/{mod,lrc,sources,radio}.rs` (plugin and UI; LRC parsing; embedded / cached / LRCLIB sources and per-track offsets; network-radio lookups); Subtitle search is `plugins/subtitle_search/{mod,api,files}.rs` (plugin and UI; OpenSubtitles hash / search / download; per-file offsets and picks, folder pairing).
- `settings.rs` becomes `settings/{mod,rows,extensions,apply,customize,onboarding,schema}.rs`: dialog wiring and setup, the row builder with every category and the search filter, the Extensions page (installed cards and catalog), applying the config (theme, mpv options, presets, toolbar, panel sync), and the Customize and onboarding dialogs.
- Shared helpers in `src/util.rs`: `clock` (m:ss / h:mm:ss, used by the library and skip-intro labels), `hms`, `strings` (Slint string models for settings and the library view) and `Task<T>`, a spawned background job polled once from the UI (lyrics search, track and radio lookups; stream lists and station logos; channels remain only where a job streams several messages); HTTP agents with their own timeout and User-Agent go through `web::agent_as` (subtitles, lyrics).
- `plugins/protocol.rs` holds the wire formats: `PluginCommand` (the UI `plugin-command` callback, arguments packed with U+001F via `split_args` / `join`, also used for lyrics and Free Music lookup keys) and `ScriptMessage`, one enum for every `script-message vivi-*` an mpv script can send (preview frames, chapter thumbs, range, fullscreen, setting, pick-folder, sub-offset, AcoustID, QR, toast), parsed once with tests.
- Slint globals no longer repeat their own name: `Library.lib-items` is `Library.items` (and so on for every `lib-*` member; the recent-files list is `Library.recent-files`), `Settings.settings-rows` is `Settings.rows`, `Customize.customize-add` is `Customize.add` (`customize-profile` is `select-profile`), `Panel.panel-tab` is `Panel.tab`, and `Playback.ab-set` is `Playback.set-ab`.
- Plugin modules and structs match their plugin IDs: `network_streams::NetworkStreams` (`network-streams`), `subtitle_search::SubtitleSearch` (`subtitle-search`), `auto_tagger::AutoTagger` (`auto-tagger`); Skip Intro has `skip_intro::ID` like the others. IDs (and so saved settings) are unchanged.
- Source map in `docs/architecture.md`; `docs/plugins-architecture.md` describes the built-in plugin folders, `protocol.rs` and `Task<T>`.
- **Karaoke follows the theme**: the full-screen lyrics stage, its chips and the empty state are light in light mode. It reads a typed lyrics state (found / searching / missing) and the numeric offset instead of comparing display strings.

### Fixed
- **`vivi --version` and `vivi --help`**: `--version` / `-V` prints the version (with the commit and build date when known) and exits, `--help` / `-h` prints the usage and exits; before, both started the player and tried to open the flag as a file. Other unknown `-`/`--` options are reported on stderr and ignored instead of being opened; `--` still lets a file name start with `-`.
- **Station logos load again**: logos were cached without a file extension, so Slint could not tell their format and every one failed ("The image format could not be determined" on each start). Downloads are now checked by their first bytes: PNG and JPEG logos are shrunk to at most 128 px and saved as `.png`, SVG logos as `.svg`, anything else (HTML error pages, icons) is not kept. Existing cache files are converted once in the background and non-images deleted. Logos and Recent pictures are decoded on a worker thread (shrunk to the tile size) once per run instead of at full size on the UI thread at every Home refresh.
- **Seek track and volume slider were not vertically aligned** in the inline bar styles: the Compact and Plain tracks and the volume wedge sat about 3 px above the button row, and in Waveform the wedge was 3.5 px above the waveform's centre. The seek track and wedge are now centred on the button row in Compact, Waveform and Plain, so they share one vertical centre (new `Metrics.bar-track-h` / `bar-wave-h` tokens).
- **Side panel library list showed files twice**: the same file reached through a symlink or overlapping library folders, or copied into two library folders, is now listed once.
- **Hover × on Recent grid tiles opened the item instead of removing it** (most visible on folders, which navigated into the folder). The × now sits above the tile's click area and removes the entry by its stored target.
- Main menu → Keyboard Shortcuts opened Settings → Playback; the library's "Edit subtitles / audio" opened Controls / Privacy instead of Subtitles / Audio.

## [0.11.1] - 2026-10-17

### Added
- **Settings → About** (last item of the System group): app icon, version and build info (commit, build date, target OS/arch, libmpv and Slint versions, install type), **Check for updates** against the latest GitHub release with an "Up to date" / "Version X available" status, release-notes excerpt and a Download button (matching AppImage / tarball / Windows / macOS asset, release page for Flatpak and pacman installs), optional *Check automatically on startup* (off by default), What's new, links (website, plugins, source, prefilled bug report, license) and credits. **Copy system info** puts the build and OS details on the clipboard for bug reports. `VIVI_SETTINGS=<category>` opens Settings on start (screenshot helper).

### Changed
- **Customize Player** rework: every bottom-bar element is a movable item — previous, play, next, ±10 s, elapsed / total time, the seekbar, flexible spaces, volume and all buttons (no fixed centre). Pick the **progress bar style** in the dialog: Compact inline, Classic (full width on top), Waveform (audio files) or Plain thin; the bar under the dialog is a live preview. Profiles: Minimal (Plain), Default (compact reference layout), Power (Waveform, all controls). The separate *Control bar layout*, *Skip buttons* and *Waveform seekbar* settings are gone; existing choices are migrated into the item list and style on load.

### Fixed
- Compact control bar matches the reference density: 36 px row, all buttons on one vertical centre (no per-icon offsets), smaller equal-size buttons, near-black seek track with a dim orange played fill and a slim bright handle, times next to the bar, smaller volume wedge. Title bar buttons are centred in the 40 px strip.
- Compact seek track uses the reference colours (track #090909, played #371F06, accent handle, bar #181818): rounded track with the fill and handle inset 1.5 px so a thin dark border stays all round; times in the secondary grey.
- About → Check for updates picks the real release assets (Windows/macOS `.zip`, AppImage, Linux tarball, `.flatpak`) instead of looking for `.msi` / `.exe` / `.dmg`.

## [0.11.0] - 2026-10-17

Highlights: compact single-row control bar, waveform seekbar and seekbar ranges (chapters, A-B loop, plugin ranges such as Cut & Export In/Out); Subtitle Search rework with hash matching, previews and per-file offsets; Free Music rework (Jamendo, Internet Archive, FMA); full-screen karaoke lyrics; merged, ranked Auto-tagger results; more MCP tools.

### Added
- New compact single-row control bar is the default: prev / play / next (optional −10 s / +10 s buttons), elapsed time, full-width seekbar, total time, volume and toolbar items. The previous two-row bar stays available as Settings → Interface → Control bar layout → Classic.
- Seekbar shows chapter boundaries as segment gaps, and the A-B loop as a highlighted range with draggable edge handles, ‹ / › nudge buttons, `*`-marked bound times and a clear button.
- Waveform seekbar for audio files: ~400 peaks decoded once in the background with symphonia (no ffmpeg), cached in `~/.cache/vivi/peaks`; played part in the accent colour. Settings → Interface → Waveform seekbar (on by default).
- Plugin hook `script-message vivi-range <a> <b> [label]` / `vivi-range clear` draws a plugin's range (e.g. Cut & Export In/Out) on the seekbar in blue; `b = -1` runs up to the playhead.
- `Shift+L` sets A, then B, then clears the A-B loop (plain `L` stays the lyrics view).
- Chapter boundaries get small notches so they read on the thin compact track.
- **Subtitle Search** (built-in) rework: OpenSubtitles searched by the **file hash first** (✓ *Exact match* badge, shown as soon as they arrive), then by name, an edited query or an **IMDb id** (`tt…` or an imdb.com link), merged without duplicates; machine translations sorted last. **Language chips** (multi-select, plus any other codes; remembered in the plugin setting). Results with downloads, rating, FPS, *HI*, *Trusted*, *Machine* and *Your pick* badges, a **details pane** (release, uploader, date, votes) and **Preview text** (first cues; the downloaded copy is reused by *Load*, so a preview costs no extra download; remaining downloads shown). One-click **Load** per row or double-click. **Save next to the video** setting (`Movie.en.srt`, never overwriting a different file). The **pick is remembered per file** and loaded again (side panel → *Forget*). **Per-file subtitle offset**: `Z` / `X` ±0.1 s, `Shift` ±1 s, side panel → Subtitles → Offset (buttons, reset), and the Delay slider — saved in `~/.local/share/vivi/subtitle-offsets.json` and restored when the file opens. *Auto-sync to audio* runs the Subtitle Auto-Sync package (alass / ffsubsync, shared media ffmpeg; Vivi bundles no ffmpeg). API key also from `VIVI_OPENSUBTITLES_KEY`. Plugin hook `script-message vivi-sub-offset <delta|set:seconds|reset>`; the script-opt `vivi-sub-offsets=1` tells Subtitle Auto-Sync 1.2.0 to leave the offset to Vivi.
- **MCP**: `search_subtitles`, `load_subtitle`, `set_subtitle_offset`, `sync_subtitles`; `get_status` reports `subtitle_offset`. The API key stays inside Vivi.
- **Free Music** (#70) rework: Jamendo, Internet Archive and the Free Music Archive (via its Internet Archive mirror) searched in parallel and merged into one list with source badges, covers, artist · album, duration and a licence chip (green when commercial use is allowed). Search as you type (debounced, never blocks the UI; Internet Archive results stream in item by item), better relevance (Jamendo asked three ways — full text, track name, artist — with retries for its empty answers; word/tag scoring; duplicates across sources collapse; runs of one artist broken up). Browse chips Popular / New / Featured and 11 genres; mood, licence (Commercial use OK, Remix allowed, CC BY, CC BY-SA, CC0 / public domain), length and sort filters; infinite scroll. Album and artist pages with Back, **Similar** (plays the track and queues similar ones, radio-like), Queue, per-row play / queue / download buttons, **Copy credit** (attribution to the clipboard), Web page. Downloads: queue with progress, genre + album artist tags, full-size cover, licence/attribution in tags and `.licence.txt`, *Choose…* folder dialog, optional hand-off to the Offline Downloads plugin. Spoken word, game rips and sound effects are filtered out of Internet Archive results; titles cleaned up.
- **Lyrics** (built-in): **full-screen karaoke view** (`L`, or side panel → Playback → Lyrics) — large centred current line in the accent colour, past lines dimmed above, upcoming below, smooth gliding scroll, and a **word-by-word wipe** for enhanced LRC (`<mm:ss.xx>` word tags); header with track, source, offset stepper, Search, Save .lrc and Embed. **Timing offset** per track (`[` / `]` ±0.1 s, `Shift` ±0.5 s, side panel buttons; saved in `~/.local/share/vivi/lyrics-offsets.json`). **Find Lyrics** dialog (`Ctrl+L`): edit artist / title, LRCLIB results with album, duration (accent when within 3 s of the file) and Synced / Plain badges, preview, *Use lyrics* / *Use plain text*; the pick is remembered per track (*Reset pick* undoes it). New sources: lyrics **embedded in tags** (ID3 USLT / SYLT, Vorbis/FLAC `LYRICS`, MP4 `©lyr`, as written by the Auto-tagger) and LRCLIB **plain lyrics** as a fallback. Priority: your pick, then synced before plain, then `.lrc` file → embedded → cache → LRCLIB. **Save as .lrc** / **Embed in tags** (Auto-tagger writer, tag backup, offset baked in) plus the *Keep downloaded lyrics* setting to do it automatically. Radio ICY-title lookup unchanged (search and offset work there too). MCP `get_lyrics` also returns `source`, `offset_seconds` and `karaoke`.
- **Auto-tagger** 0.9.0 rework: results of AcoustID / MusicBrainz, Deezer, iTunes, Last.fm and Discogs are **merged and ranked by match confidence** (score + up to +12 for sources that agree; duplicates of the same recording/release collapse; matches of the current title/artist get +5). Values another source disagrees on are kept as **alternatives**: the review shows a **⇄ N** chip per field to cycle through them, with the source badge following the chosen value. Lyrics rows say **LRCLIB synced** or **LRCLIB plain**; each file card notes the fingerprinter (system `fpcalc` first, else built-in), the sources asked and when lyrics embedding is off. Fingerprint matches now also **suggest a file name** in the built-in flow (new settings *Suggest file names* and *File name pattern*, default `{artist} - {title}`); renaming is **opt-in** (unticked, *Apply all* does not tick it), refuses existing names (also differing only in case), path separators and hidden names, and sanitizes illegal characters, Windows device names and over-long names. Folder tagging shows a percentage with matched/skipped counts. API keys can also come from `VIVI_ACOUSTID_KEY` / `VIVI_LASTFM_KEY` / `VIVI_DISCOGS_TOKEN`. Official builds embed an AcoustID application key at build time (CI variable `VIVI_ACOUSTID_KEY`; order: Settings → environment → built-in), so fingerprinting works out of the box. New **Done** screen: success headline with counts (tagged / renamed / skipped / failed), a card per file with its status, reason and every change, per-file **Undo** and **Open folder**, and **Undo all**.
- **Plugin hooks** for Remote Control 1.1.0: `script-message vivi-qr <title> <text> [caption]` shows a QR code dialog (above Settings), used by *Remote Control → Show QR code* to pair a phone; `script-message vivi-fullscreen [toggle|on|off]` switches Vivi's window fullscreen (mpv's own `fullscreen` property does not reach Vivi's window).
- **Network Streams** 0.9.0: XMLTV program guide — now / next per `tvg-id` from the playlist `url-tvg` / `x-tvg-url` or a guide URL setting (`.xml` / `.xml.gz`); only the next 12 h, three titles per channel, cached compactly for 6 h. **Favorites** (formerly Saved) pinned on top with a ★; `group-title` chips and iptv-org category chips (News, Sports, Movies, Kids, Music, …); **My playlists**: custom M3U URLs or files that refresh themselves (24 h / 6 h / 7 days), show channel count, last update and the last error, and keep the last good copy when a link breaks; **Search all (TV + radio)** across favorites, iptv-org, your playlists and radio-browser; **Add to queue** and **Mini player** actions; broken logo URLs remembered for a week and the logo cache capped at 64 MB.
- **MCP** (`vivi-mcp`): `list_audio_tracks` / `set_audio_track`, `list_subtitle_tracks` / `set_subtitle_track`, `get_chapters` / `seek_chapter`, `set_speed`, `screenshot`, `open_network_stream`, `search_streams`, `list_plugins` / `plugin_info` (read-only) and `get_lyrics`, with matching control-socket commands.

### Fixed
- Plugin toasts (lyrics offset, saved files, …) no longer show the resume toast's *Start over* button.

## [0.10.0] - 2026-10-15

Highlights: new Equalizer, Chapter Thumbnails and AcoustID plugins; SponsorBlock undo, Scrobbler with Last.fm, and Subtitle Auto-Sync with managed helpers; Auto-tagger Deezer/iTunes/Last.fm/Discogs sources with per-field source and optional LRCLIB synced lyrics; plugin marketplace at plugins.viviplayer.app with What’s new and vivi://plugin deep links; viviplayer.app site; install.sh, AUR/Flathub/winget/Scoop packaging.

### Added
- **Equalizer** plugin (`equalizer` 1.0.0, #74): Flat, Bass, Vocal, Rock, Night and Custom presets via mpv `equalizer` / `superequalizer` (`Alt+E` / `Alt+Shift+E`). Uses labelled filter `@vivi-eqplug`, separate from the built-in Audio-panel EQ (`@vivi-eq`), which stays.
- **Chapter Thumbnails** plugin (`chapter-thumbnails` 1.0.0, #75): seek-bar hover previews and chapter list thumbnails cached per file (system ffmpeg or Vivi’s media build from `helper-ffmpeg-v8.0-2`); `Alt+H` shows the chapter list. Core loads the cache via `vivi-preview-frames` / `vivi-chapter-thumbs`.
- **AcoustID** plugin (`acoustid` 1.0.0, #76): `Alt+I` fingerprints the playing file (system `fpcalc` or a managed Chromaprint download), looks up AcoustID + MusicBrainz, opens the Auto-tagger review with a rename proposal from a pattern.
- **Auto-tagger** (#79, #80): extra sources Deezer, iTunes Search, Last.fm (genres) and Discogs (optional token); each proposed field shows its source. Optional *Embed synced lyrics* from LRCLIB into MP3 (SYLT+USLT), FLAC/Ogg/Opus (`LYRICS`) and M4A (`©lyr`).
- **Auto-tagger review dialog**: *Review changes…* / *Review all files…* with before/after per field, checkboxes, cover candidates (Cover Art Archive), Apply selected/all/Skip, and Undo from a per-apply backup.
- **Plugin marketplace** at <https://plugins.viviplayer.app>: searchable catalog with categories, per-plugin pages (screenshots, changelog, older downloads, permissions, *Open in Vivi*), Atom feed (`/feed.xml`). Listings in `plugins/<id>/store.toml`. Browse shows **What’s new** for pending updates from the catalog changelog.
- **vivi://plugin/<id>** deep links open the plugin in Settings → Extensions → Browse (`x-scheme-handler/vivi`). Catalog URL is <https://plugins.viviplayer.app/index.json> with GitHub Pages fallback (<https://loafer19.github.io/vivi/index.json>).
- **viviplayer.app** (<https://viviplayer.app>): SEO site (canonicals, Open Graph, sitemap, SoftwareApplication JSON-LD); `install.sh` at <https://viviplayer.app/install.sh>. GitHub Pages remains a mirror.
- **install.sh**: on Arch builds `vivi-bin` with makepkg; elsewhere installs the release tarball into `~/.local` with SHA256SUMS check; `--uninstall`; container tests in CI.
- **Add Subtitles → From Computer** (Figma, #51): *To This File / To This Folder*, *From Computer / From OpenSubtitles*, folder breadcrumb, video–subtitle pairing and Save.
- Distribution packaging: AUR `vivi-bin` / `vivi-git`, Flathub submission files, winget and Scoop manifests; CI checks (makepkg + namcap, flatpak-builder-lint); release workflow bumps versions and checksums.
- Helper **ffmpeg-media** / **ffprobe-media** (`helper-ffmpeg-v8.0-2`): video decoders, swscale, JPEG/raw frames and PCM out for Chapter Thumbnails and Subtitle Auto-Sync (#75, #72).

### Changed
- **SponsorBlock** 1.1.0 (#73): skip notice with undo (`Alt+Shift+K`); works for local files named with `[video id]` (YouTube plugin downloads).
- **Scrobbler** 1.1.0 (#77): Last.fm (desktop auth, signed scrobbles) alongside ListenBrainz; separate offline queue per service.
- **Subtitle Auto-Sync** 1.1.0 (#72): system alass first, else verified alass 2.0.0 + shared media ffmpeg download; manual offset nudging (`Alt+Z` / `Alt+X`, larger steps with Shift) saved per file.
- **YouTube** plugin 1.2.0: managed ffmpeg is Vivi’s minimal static build (~6 MB per platform instead of ~150 MB), checksums pinned (`helper-ffmpeg-v8.0-1`).
- Marketplace: real plugin screenshots (Xvfb), hidden cards stay hidden under category/search filters.

## [0.9.1] - 2026-10-15

Highlights: Discord Rich Presence and Remote Control become downloadable plugins with small native helpers, a new YouTube & yt-dlp plugin (playlists, sizes, downloads; manages its own yt-dlp and ffmpeg), an MCP server for AI agents, radio lyrics, a visualizer fed by mpv's real output, and the Inter font bundled in the Flatpak and AppImage.

### Added
- **MCP server for AI agents** (#68): new `vivi-mcp` binary (stdio transport, std + serde_json only) that controls a running Vivi over a private local socket (`$XDG_RUNTIME_DIR/vivi.sock`, mode 0600). Tools: play/pause/stop, seek, volume, open file or URL, queue list/add/play/next/previous, now-playing status, library search, subtitles on/off, EQ presets; resources `vivi://status` and `vivi://playlist`. Off by default: *Settings → General → Remote control* or `VIVI_IPC=1`. Docs with client configs in `docs/mcp.md` and on the site; scripted client test in `tests/mcp.rs`.
- **Radio lyrics** (Lyrics plugin): on network streams, every change of the stream title (`icy-title`, else `media-title`) is parsed as "Artist - Title" (decorations like "(Radio Edit)" and "feat." dropped) and looked up on LRCLIB (`/api/get`, then `/api/search`, also matching "Title - Artist"). Ads, jingles, station IDs and titles equal to the station name are skipped; results are cached in memory and in `~/.cache/vivi/lyrics` (misses too). Plain lyrics are shown as a scrollable block because the song's start is unknown; the *Radio lyrics* setting can switch to *Approximate sync* (synced lines timed from the title change, plus *Radio sync offset* in seconds) or *Off*.
- **YouTube & yt-dlp** plugin (`youtube`): list a video or playlist URL into the playlist (`Alt+U` pastes from the clipboard), estimated size per quality, download video or audio only into a chosen folder with progress toasts. Uses a recent system yt-dlp, otherwise manages (verifies, updates) its own copy from the official GitHub releases. Vivi appends that copy's folder to `PATH`, so the Flatpak streams YouTube with it.
- The YouTube plugin uses a system ffmpeg, or downloads a verified static ffmpeg + ffprobe (BtbN builds for Linux/Windows, osxexperts.net for macOS) only when a download needs it (1080p+ merging, mp3/m4a/opus conversion); only that managed copy is updated. Its size is shown in Browse and before it downloads.
- Flatpak and AppImage bundle the **Inter** UI font (`share/vivi/fonts`, used through `SLINT_FONT_PATH`); the Flatpak may reach Discord's IPC socket.

### Changed
- **Visualizer reads mpv's real output** (#71): instead of decoding the file a second time with symphonia (local files only, EQ not reflected) and a synthetic animation for streams, a labelled `af` filter (`@vivi-viz`, kept last in the chain) measures 48 log-spaced bands (30 Hz–14 kHz) with lavfi `bandpass` + `astats` and streams them, timestamped, to a localhost socket; the visualizer picks the frame matching `time-pos`. Works for network streams and reflects the EQ, compressor, reverb and other audio filters. The filter is only installed while a visualizer mode is showing.
- **Discord Rich Presence** and **Remote Control** moved out of the core into downloadable plugins (`discord`, `remote-control`, same ids so settings carry over). Each is a Lua script plus a small native helper (`vivi-discord`, `vivi-remote`, sources in `helpers/`) that talks to mpv over its JSON IPC. The `discord-rich-presence` crate is gone from Vivi.
- Plugin packages can ship per-platform native helpers: manifest `binaries`, catalog `needs`/`binaries` with one sha256 per platform asset, built by CI for Linux x86_64/aarch64, Windows x64 and macOS. Browse shows the size including the helper, or "Not available for <platform>".
- Packages get their folder as script-opt `<id>-pkgdir`, can save their own settings (`vivi-setting`) and open a folder dialog (`vivi-pick-folder`); action settings can set a button `label`.

### Fixed
- Downloadable plugins declared `min_vivi = 0.8.0`, but packages exist only since 0.9.0; they now say 0.9.0 (new patch versions of A-B Loop, Night Mode, Playback Statistics, Scene Chapters, ListenBrainz Scrobbler, SponsorBlock, Subtitle Auto-Sync).

## [0.9.0] - 2026-10-15

Highlights: Network Streams (radio now-playing card and visualizer, IPTV via iptv-org, music genres and SomaFM, M3U import), a loading spinner and error toast for streams, physical-keycode shortcuts, chapter thumbnails, EQ preset import/export, Figma-accurate dialogs, filters and miniplayer, Discord Rich Presence and Remote Control, downloadable plugins (Auto-tagger, Free Music, ListenBrainz, Subtitle Sync, SponsorBlock, Scene Chapters, A-B Loop, Night Mode, Playback Stats), an AppImage cut from 106 MB to 44 MB, and a Flatpak build.

### Added
- **Equalizer** per the Figma components frame (#45): preamp + 10 vertical band sliders (60 Hz–16 kHz) with editable dB boxes, ON/OFF switch, *Open/Close Equalizer* bar and a presets dropdown with VLC's 18 presets (Flat, Classical, Club, Dance, Full bass, Full bass & treble, …) and their icons, plus custom presets (*New Preset*, rename, delete; edits follow the selected custom preset). Applied as a labelled mpv `af` (`@vivi-eq`, lavfi `volume` + `equalizer` chain; preamp uses VLC's convention, 12 dB = unity) and saved in `~/.config/vivi/fx.toml`.
- **Filter panels** (#46) with collapsible *Expand/Collapse Settings* boxes: video adjustments (hue, brightness, contrast, saturation, gamma, sharpen, debanding, film grain), crop (top/bottom/left/right with link), colours & effects (colour extraction, colour threshold/chroma key, negate, posterize, gradient map, cartoon, sepia), audio compressor (RMS/peak, attack, release, threshold, ratio, knee, makeup), reverb and delay/echo. Each group is one labelled mpv `vf`/`af` lavfi filter (`@vivi-video`, `@vivi-crop`, `@vivi-color`, `@vivi-comp`, `@vivi-reverb`, `@vivi-delay`), has Reset and persists in `fx.toml`.
- **Open Network Stream** dialog redrawn from Figma (#47): several links at once (one per line, validated), optional playlist name (saves an `.m3u` and plays it), recent streams, yt-dlp hint kept.
- **New Playlist** dialog (#48): title, thumbnail (saved next to the `.m3u` and shown in the library), files by picker or drag & drop, remove before saving. *Create playlist* in the library opens it.
- **Miniplayer** variants per Figma (#49): queue above the video with “N items · 2h 3m”, sort, remove-selected and add, drag handles to reorder and per-item lengths; compact overlay with ×/⧉ corners, centred transport and an edge progress bar; info strip with title, folder and subtitle track.
- **Downloadable plugins**: plugins no longer have to be built in. A package is a zip with `manifest.toml` (id, name, semver version, `min_vivi`, entry script, permissions, settings) and an mpv Lua/JS script. The catalog `plugins/index.json` (published at <https://loafer19.github.io/vivi/index.json>) lists each package's download URL, sha256 and size. Design, options considered (mpv script packages now, WASM later, native libraries rejected) and the install/update model: `docs/plugins-architecture.md` and the site page *Plugin distribution*.
- **Settings → Extensions tabs**: *Installed* (built-in, downloaded, mpv scripts; Update / Roll back / Uninstall for packages) and *Browse* (catalog cards with version, author, size, permissions and Install / Update / Uninstall). Installing shows the requested permissions first. Downloads and catalog checks run on background threads; packages are verified against their sha256, unpacked safely into a staging folder and switched in atomically under `~/.local/share/vivi/plugins/<id>/<version>/`, keeping the previous version for rollback.
- **Update checks**: the catalog is fetched at startup at most once a day ("Check for plugin updates daily", on by default) or with *Check now*, cached in `~/.cache/vivi/plugin-index.json`; a toast announces available updates. `VIVI_PLUGIN_INDEX` overrides the catalog URL.
- Package settings appear in Extensions like built-in ones and reach the script as mpv `script-opts` (applied live); action settings are sent as script messages.
- Three downloadable plugins: **A-B Loop & Frame Step** (#59), **Night Mode** (#60), **Playback Statistics** (#66).
- Keys Vivi does not use (and all `Alt` combinations) are forwarded to mpv, so key bindings of scripts and packages work.
- `scripts/package-plugins.py` builds deterministic package zips and regenerates the index entries; the **Plugins workflow** checks Lua syntax and the index, publishes each new version as a release `plugin-<id>-v<version>` (never marked latest) and verifies the uploaded sha256.
- **Auto-tagger** built-in plugin (#69, includes AcoustID tagging #65): identifies songs by Chromaprint fingerprint (`fpcalc` when installed, else the bundled `rusty-chromaprint`) on AcoustID, or by tags / file name with a MusicBrainz search; MusicBrainz lookups (≤ 1 request/s, descriptive User-Agent) give title, artist, album, album artist, date, track, genre and ids, and the Cover Art Archive the front cover. A preview dialog lists candidates with a per-field diff; *Write tags* / *Write all* write with lofty. Originals are backed up once to `~/.local/share/vivi/tag-backups/` and *Restore* puts them back. *Tag folder…* batch-identifies a library folder and lists only confident matches. The AcoustID application key is an optional setting.
- **Free Music** built-in plugin (#70): searches CC / public-domain music on the Internet Archive (no key) and Jamendo (client ID setting), shows each track's licence and attribution, previews the stream in the player and downloads to `~/Music/Free Music/<Artist>/` with tags, cover, licence URL and attribution (also in a `.licence.txt`).
- **Network Streams** built-in plugin (#67): a streams browser (separate from the Open Network Stream dialog) with Internet radio from radio-browser.info (search, top voted, by country, by genre; no key), M3U / IPTV playlist import from a file or URL, saved and recent streams, and stream info (ICY title, codec, bitrate, buffer).
  - Audio-only network streams show a *Now playing* card (station name and logo, current ICY title, codec / bitrate / sample rate) and the visualizer animates from a synthetic signal, since streams cannot be decoded a second time.
  - With *M3U / IPTV playlist* selected, text that is not a file or URL searches radio stations by name and genre instead (the source switches to *Radio: search*); *Choose file…* picks a playlist; the placeholder follows the source; percent-encoded names (`01.%20J.%20S.%20BACH…`) are decoded.
- **Discord Rich Presence** built-in plugin (#57, off by default): title, artist and progress in Discord via its IPC socket (`discord-rich-presence` crate, worker thread, reconnects), privacy option to hide names; needs a Discord application ID.
- **Remote Control** built-in plugin (#58, off by default): local web page for phones (play/pause, seek, volume, previous/next, queue, cover art), LAN-only connections, PIN with lockout, port setting. No new dependencies (std TCP server).
- **Subtitle Auto-Sync** package (#56): `Alt+Y` / *Sync current subtitle* aligns the selected external subtitle to the audio with `alass` (or `alass-cli`) or `ffsubsync`, whichever is installed, in the background; the result is saved as `<name>.synced.<ext>` next to the original and loaded. A clear message with install hints when neither tool is found.
- **SponsorBlock** package (#64): skips sponsor / self-promotion / interaction (configurable) segments in YouTube streams, automatically or on `Alt+K`, and marks them as chapters on the seek bar. Uses the privacy-preserving hash-prefix API; no key.
- **Scene Chapters & Thumbnails** package (#62): ffmpeg scene detection (`Alt+J` or automatic) adds `Scene N` chapters to videos without chapters and saves a JPEG thumbnail per chapter, cached under `~/.local/share/vivi/plugin-data/scene-chapters/`.
- **ListenBrainz Scrobbler** package (#63): now playing, listens after 50 % or 4 min, offline queue sent in batches, status and *Send queued listens now* actions. Last.fm is not included (needs a signed browser auth flow).
- **Network Streams: TV (iptv-org)**: *TV: by country* and *TV: by category* list the iptv-org countries/categories (iptv-org.github.io/api); *Play* on one loads its playlist (cached for a day in `~/.cache/vivi/iptv/`), typing filters channels by name, rows show channel logos, and channels play as video. A word typed into the *M3U* source now searches iptv-org channels instead of radio stations; pasted URLs and files still import as before.
- **Network Streams: music**: *Music: genres* browses radio-browser.info tags by station count plus *Popular* (top voted) and *Trending* (top clicked) and opens the top stations of a genre; *Music: SomaFM* lists SomaFM channels with logos.
- **Stream loading overlay** (core player): while a network stream opens or buffers (before the first frame/audio, or paused for cache) a spinner with the stream name is shown instead of a black screen; it hides on playback start, and a failed open shows an error toast.
- **Equalizer preset import/export** (#61): custom presets can be exported to and imported from JSON files.
- **Chapter thumbnails** (#62): the side panel's Chapters list shows the thumbnails saved by Scene Chapters.
- **Packaging**: Flatpak manifest, AppStream metainfo and a CI job that builds a `.flatpak` bundle (#29). The AppImage now bundles a slim libmpv (decoders-only FFmpeg, no mpv CLI, zstd -19), cutting it from 106 MB to 44 MB; the Windows zip is smaller too.

### Fixed
- Shortcuts with modifiers are forwarded to mpv by physical key (winit KeyCode mapped to US-QWERTY mpv names), so they work with any keyboard layout; the Cyrillic table stays as a fallback.
- Plugin package key combos (A-B Loop Alt+A/B/C, Night Mode Alt+N, Statistics Alt+S) gave no feedback on Wayland: mpv's OSD is only drawn into the video frame, so with no video (idle, audio files, UI on top) their messages were invisible. Packages now also send `script-message vivi-toast <text>`, which Vivi shows as a toast (A-B Loop 1.0.1, Night Mode 1.0.2, Playback Statistics 1.0.1).
- Alt/Ctrl combos typed with a Cyrillic layout active (e.g. Ukrainian "т" for Alt+N) are mapped to the QWERTY key, so mpv bindings still match. `VIVI_KEY_DEBUG=1` logs every key event and the mpv key name it is forwarded as.
- Extensions shows whether each downloaded plugin / mpv script is loaded in mpv (Loaded, Not loaded: disabled / startup loading off, Failed / Error with mpv's message); load results and script errors are also logged to stderr.
- Subtitle Search: OpenSubtitles answers `301` unless query parameters are sorted by name, so searches sent `moviehash`/`query`/`languages` in the wrong order and relied on a redirect. Parameters (and the language list) are now sent sorted and lowercase.
- **Free Music relevance**: Internet Archive search is limited to audio in music collections (netlabels, opensource_audio, audio_music, Jamendo/FMA mirrors, 78rpm, etree), excludes podcasts, lectures, audiobooks and radio shows, skips files over 20 minutes, expands each item into its individual tracks with tidied titles, and ranks results by artist/title match. Jamendo is the default source once a client ID is set; the setting links to https://devportal.jamendo.com.
- The docked Library panel now hides with the other controls after inactivity during playback and comes back on mouse move (it used to stay, leaving half the window black during audio streams).

### Docs
- Screenshots refreshed with Sintel, including `docs/screenshots/responsive` (#50); new `panel-audio`, `audio-effects`, `equalizer-presets`, `new-playlist` and `miniplayer-queue` images and Figma compare images for the equalizer, filters, dialogs and miniplayer.
- Docs site: catalog cards show an *Install in Vivi* badge, size, minimum version and permissions; new *Plugin distribution* page; plugin guide gained *Package & publish*.

## [0.8.0] - 2026-10-14
- Docs site uses inline Lucide icons (no JavaScript), screenshot gallery, credits and mobile layout fixes; build fails on broken local links
- Screenshots regenerated from the release build with Sintel (CC BY) and Kevin MacLeod music (CC BY); `VIVI_SCREENSHOT=1` enables the screenshot helpers in release builds

Milestone M8 (v0.8): Figma fidelity. Each part has a side-by-side Figma vs Vivi image in `docs/design/compare/`.

### Added
- **Plugin system** (`src/plugins/`): compiled-in plugins implement the `Plugin` trait (hooks: `enable`, `disable`, `file_loaded`, `tick`, `setting_changed`, `command`) and describe themselves with a manifest (id, name, description, version, author, icon, settings). Enabled state and settings are saved in `config.toml` under `[plugins.<id>]`. Settings → Extensions now lists every plugin as a card with an ON/OFF toggle, followed by its settings while it is on, plus "Load add-ons on startup", the scripts folder, Rescan and a link to the catalog.
- **mpv scripts as external plugins**: Lua/JavaScript scripts (or directories with `main.lua`/`main.js`) in `~/.config/vivi/scripts` get their own card and toggle. `@name`, `@description`, `@version` and `@author` header comments fill in the card. Enabled scripts are loaded with mpv `load-script`. mpv cannot unload a script, so turning one off takes effect after a restart. An example script is in `plugins/examples/now-playing.lua`.
- **Skip Intro & Credits plugin**: a "Skip intro" / "Skip credits" button appears while playback is inside a chapter named Intro, Opening, OP, Credits, Ending, ED (and similar), or inside a per-show range (`show = 0:30-2:00, 21:10-end`). Ranges can be typed in or marked while watching with the Mark button. There is an optional auto-skip, and skipping credits at the end goes to the next file.
- **Lyrics plugin**: shows synced lyrics as a karaoke overlay that scrolls and highlights the current line, over video or the visualizer. Lyrics come from a `.lrc` next to the file, then a disk cache (`$XDG_CACHE_HOME/vivi/lyrics`), then LRCLIB (`/api/get`, falling back to `/api/search`). The lookup uses artist, title, album and duration from the tags (lofty) or mpv metadata. Online lookup can be turned off, text size is configurable, and the cache can be cleared.
- **Subtitle Search plugin**: "Add subtitle file…" opens an Add Subtitles dialog based on the Figma frame. It searches OpenSubtitles by the file's OpenSubtitles hash plus a query cleaned from the file name, in the configured languages. The chosen file is downloaded to `$XDG_CACHE_HOME/vivi/subtitles` and loaded with `sub-add`. The API key is set in the plugin settings. Without a key, the dialog explains where to get one. Login with username and password is optional. "From Computer" still opens the file picker.
- **The visualizer is a plugin**: it can be turned off in Settings → Extensions.
- **Plugin catalog and docs site**: `plugins/index.json` is a machine-readable list, and a unit test keeps it in sync with the built-in manifests. `docs/site/` holds a static dark-theme site with a landing page, a plugin catalog generated from `index.json` by `scripts/build-site.py`, and a plugin developer guide covering the manifest format, hooks, UI and mpv script plugins. The `.github/workflows/pages.yml` workflow deploys it to GitHub Pages.
- **Music visualizer** (`src/viz/`): audio-only files (no video track; cover art does not count) show a visualizer instead of a blank area. There are seven modes: Spectrometer (scrolling spectrogram), Scope (oscilloscope), Spectrum (bars with peak caps), Waveform (scrolling loudness envelope), Radial (circular spectrum that pulses on beats), Particles (bursts on onsets) and Pitch (12 pitch-class wheel coloured by the dominant note, with the note name shown). Colours follow the theme accent and background. Pick a mode, or Disabled, from the Figma-style dropdown at the top left, or press `V` to cycle. The choice is remembered in `$XDG_STATE_HOME/vivi/visualizer`, and the visualizer fills the window in fullscreen. Audio comes from a second lightweight decode of the file (symphonia) kept in sync with mpv's `time-pos`, so it works with any audio output and needs no PipeWire monitor. The FFT uses rustfft. Analysis and drawing run at about 30 fps on a canvas of at most 800 px, and they stop when paused or when the file has video. New modes implement the `Visual` trait in `src/viz/modes.rs`.

### Fixed
- `Player::cmd` quoted arguments with mpv's `%len%` syntax, which `mpv_command_string` does not understand. Adding a subtitle or audio file from the menu failed. Arguments are now double-quoted with escapes.

### Changed
- **Menus** (#34): the main menu, its submenus and the library context menus (file, folder, playlist, group) now match the Figma frames, including sizes, icons, shortcut hints and checked rows. New entries: Stop (`S`), Record, Hide UI (`Ctrl+\`), Open Folder, Mark as Watched, Show in File Explorer, Go to Source, Add Subtitle/Audio Tracks, Add to Playlist, Properties & Metadata, and Clear Progress.
- **Sort, view and dropdowns** (#35): the sort and view menus follow Figma, with the active sort shown as checked and a new "Largest First" sort. The library can sort by unwatched or watched. Settings uses the new Figma dropdown in place of ComboBox.
- **Library sidebar** (#36): it was rebuilt with a 50 px icon rail (Home, Videos, Songs, Streaming, Playlists, Recent, Add), back and forward history, a breadcrumb that collapses to "…" when long, an item count, and sort, view and search buttons. The Home screen shows these sections: Continue Watching, Listen Again, Frequently Visited Folders and Playlists, Frequently Listened Artists, Favorites, Recently Added and Recently Played. During playback the sidebar stays docked on the left. The Music root, Network (with recent stream URLs) and Recent views were added.
- **Cone volume control** (#37): the outline and gradient fill now match the Figma states.
- **Library rows** (#38): rows are 32 px high, with a thumbnail, a progress line and dimmed text for watched items. Hovering a row shows a "⋯" menu.
- **Settings window**: rebuilt to match the Figma settings frames. It has a search field and a sidebar grouped into App (General, Interface, Privacy, Controls), Media (Playback, Video, Audio, Subtitles, Library) and System (Network, Performance, Recording, Extensions). Each section ends with a collapsible "Advanced Options" bar. New reusable controls are in `ui/controls.slint`: an ON/OFF toggle, an accent slider with ticks and scale labels, a segmented control, a text field, a path field with Browse, and a flat button. The dropdown with a check mark is reused. New options are applied to mpv live and saved to `config.toml`:
  - Theme: Dark, Light or System.
  - Logging: log file and log level.
  - Playback: repeat, precise seeking and yt-dlp quality.
  - Video: enable video, stop the screensaver, default picture adjustments, deinterlacing, aspect ratio, and snapshot folder and format.
  - Audio: enable audio, output device, startup volume, loudness normalization and default delay.
  - Subtitles: enable subtitles, encoding, font, bold, color, opacity, outline, shadow and position.
  - Network: buffering, cache, proxy, TLS verification and user agent.
  - Performance: decoding threads, loop filter, frame dropping and video sync.
  - Controls: seek and volume step sizes.

  The Recording, Casting, Streaming and Extensions rows, and a few others, are placeholders for now. Their values are saved, but they do not change playback yet.
- **Icons**: every icon is now sliced from the Figma icon sheet, keeping the two-tone and orange active variants. See `docs/design/icon-map.md` and `docs/design/compare/icons.png`.

## [0.7.0] - 2026-10-14

Milestone M7: packaging and desktop integration.

### Added
- **Packages for every platform** (#28, #29): an AppImage with libmpv bundled, a Linux tarball (binary, `.desktop` file, icons), Arch `PKGBUILD`s in `packaging/arch` (`vivi-bin` from the release tarball and `vivi` from source; the release also ships a `PKGBUILD` with checksums filled in), a Windows x86_64 zip (MSVC build with `libmpv-2.dll` from shinchiro's mpv builds), and a best-effort macOS arm64 `Vivi.app` (Homebrew mpv, unsigned). Every release has `SHA256SUMS`.
- **Desktop file and app icon.** The icon is also used as the window icon.
- **File associations** (#30): `vivi.desktop` declares 22 video and audio MIME types. The README shows how to make Vivi the default player with `xdg-mime`.
- **MPRIS2** (#31, Linux): media keys, `playerctl`, and Waybar or desktop widgets work. Supported: play/pause/stop, next/previous, seek and position, volume, plus metadata (title, artist, album, length, cover art).
- **Seek-bar hover preview** (#10): hovering the seek bar shows a video frame above the time. Frames are keyframes extracted in the background with `ffmpeg` (optional). Turning off thumbnails in Settings → Library also turns this off.
- **Miniplayer** (#32), following the Figma "Miniplayer" frame: a compact 391×339 always-on-top window with the title, seek bar and transport controls. An optional queue view grows it to 391×690. Toggle it with `P` or the new top-bar button; `Esc` goes back to the full player at its previous size.
- **Responsive layout** (#33): the window can shrink to 480×320. Bottom-bar buttons that do not fit move into a “⋯” overflow menu, and the volume slider collapses below 900 px. The side panel overlays the video, and below 800 px it takes the full width. The library grid picks its column count from the width, falls back to a list when very narrow, and the library sidebar becomes an icon rail below 700 px. Below 760 px, Settings replaces its category sidebar with a dropdown. Onboarding and other dialogs scale down.
- **Resizable miniplayer** (#33): it can shrink to 280×160, and its size and position are remembered (`interface.mini_window`). mpv letterboxes the video, so the aspect ratio is kept. When the window is small, the queue and secondary controls are hidden; at tiny sizes only the video shows, with play/pause and a progress line on hover.
- **Drag & drop on Wayland** (#5): winit does not support Wayland drag & drop yet. `VIVI_X11=1` runs Vivi through XWayland, where dropping files works. The README documents this.

### Fixed
- **Top-bar buttons did not respond to clicks** (seen on Hyprland since v0.5.0). The invisible hover strip that reveals the top bar was declared after the bar, so it sat on top of it in z-order and caught every click. It now sits underneath the bar.
- Video turned black after library thumbnails or other images had been drawn: GL state is now reset before mpv renders each frame.
- Home directory and path separators now work on Windows. SQLite is bundled on Windows and macOS.

## [0.6.0] - 2026-10-14

Milestone M6: Library.

### Added
- **Library home screen** (#26): Continue watching (from mpv resume positions, with progress bars), Pinned items, Recently added and Recent. It replaces the old empty state and opens over playback with the new Home button in the top bar.
- **Sections** (#24): Video (folder browsing with a breadcrumb), Music (Songs, Albums, Artists, Genres from file tags), Playlists (saved `.m3u` files).
- **Search, sorting and views:** search the whole library or the current section; sort by name (natural order, so “Ep 2” comes before “Ep 10”), date, size or watched progress; grid or list view. The sort and view choices are saved.
- **Context menu** (right-click): Play/Open, Add to queue, Pin to Home / Unpin, Remove from Continue watching.
- **Open Network Stream** (#25): plays http(s), HLS/DASH, `.m3u` playlists, rtsp/rtmp/udp URLs. YouTube and other sites work when `yt-dlp` is installed (the dialog says whether it is).
- **Library folders** in Settings → Library: add folders (folder picker), remove them, rescan; in Advanced, turn thumbnails on/off and clear the thumbnail cache. Defaults to `~/Videos` and `~/Music` if they exist.
- **Background scanning:** `walkdir` scan into a SQLite cache (`~/.cache/vivi/library.db`). Only new or changed files are re-read; tags and durations come from `lofty`.
- **Lazy thumbnails:** a worker thread renders video frames and album covers with the `ffmpeg` CLI (optional) into `~/.cache/vivi/thumbs`. No image decoders are linked into the binary.
- **Compact library** tab in the side panel: search, click to play, `+` to add to the queue.
- Playing a song from a list queues the songs after it.

### Changed
- Build now links the system SQLite (`sqlite` on Arch, `libsqlite3-dev` on Debian/Ubuntu). The release binary grew by about 1.5 MB in total.

## [0.5.0] - 2026-10-14

Milestone M5 complete: customization and onboarding.

### Added
- **Customize Player** (#22), following the Figma "Player - customize" frame: choose which buttons appear in the bottom bar, re-order them (‹ ›), remove them (×) and add any of 16 items: queue, subtitles, audio, volume, video, speed, chapters, bookmark, ±10 s, stop, snapshot, repeat, shuffle, settings, fullscreen. Changes preview live; Cancel reverts.
- **Layout profiles** (#22): built-in Minimal, Default and Power layouts, plus Custom for your own. Pick one in Customize Player or in Settings → Interface → Toolbar profile. Saved in `config.toml` (`interface.profile`, `interface.toolbar`).
- **First-run onboarding** (#23), following the Figma Onboarding frames: welcome, then theme and accent color, then a short hotkeys intro. Skippable; the `interface.onboarded` flag keeps it from showing again, and Settings → Interface → Welcome tour (Advanced) replays it.
- New bottom-bar actions: repeat (loop file), shuffle the queue, snapshot (mpv screenshot), stop, and ±10 s seek.

### Fixed
- Side-panel sliders (delays, subtitle size/position, video adjustments, speed, volume) now show mpv's actual values, including those set by a per-show preset, and keep them when the panel is closed and reopened.
- Settings language fields keep focus while you type: rows are now updated in place instead of being rebuilt.
- The settings search heading now matches the search box, also when the search is set programmatically.
- The volume slider's maximum follows the volume boost cap setting.

### Changed
- The `VIVI_SETTINGS`, `VIVI_ADVANCED` and `VIVI_SEARCH` test variables (and the new `VIVI_CUSTOMIZE` and `VIVI_ONBOARDING`) now work only in debug builds.

## [0.4.0] - 2026-10-13

Milestone M5: Settings.

### Added
- **Settings dialog** (gear button in the top bar or `Ctrl+,`) following the Figma Settings frames: categories, a search box across all settings, a Basic/Advanced toggle and info tooltips on every option.
- **Interface:** dark/light theme (Figma light tokens), six accent colors, auto-hide delay.
- **Playback:** resume on/off, default speed, hardware decoding mode.
- **Audio:** preferred audio languages, volume boost cap (100–300%).
- **Subtitles:** preferred subtitle languages, font size, auto-loading of external subtitle files.
- **Keyboard Shortcuts** reference page (read-only).
- **Privacy:** clear recent files and saved resume positions.
- **Per-show presets** (#16): save subtitle/audio delays, subtitle size and position, and track languages from the side panel; they are applied automatically to every episode of the same show and can be deleted in Settings → Presets.
- Settings persist to `~/.config/vivi/config.toml` (atomic write; an invalid file is kept as `.bak`).

## [0.3.0] — 2026-10-13
Milestone M4: queue, playlists and resume.

### Highlights
- **Queue** tab in the side panel: everything you open, drop or pass on the command line plays in order.
- **Resume where you left off**, with a "Start over" prompt (Figma "continue" toast).
- **Recent files** on the start screen.

### Added
- Queue: add files (multi-select), drag & drop to append, click to play, reorder, remove, clear; next/prev buttons step through it.
- Playlists: save the queue as `.m3u` in `$XDG_DATA_HOME/vivi/playlists`, load and delete saved playlists.
- Resume: mpv watch-later files in `$XDG_STATE_HOME/vivi/watch_later` (position, tracks, delays, volume, speed), saved every 5 s and on quit.
- Recent: last 8 files on the empty state (stored in `$XDG_STATE_HOME/vivi/recent.txt`), with Clear.

### Known issues
- Queue reordering uses up/down buttons; drag-to-reorder is not implemented.
- The playlist name field still uses Slint's default (light) text input style.
- Drag & drop does not work on Wayland yet (#5).
- Settings and per-show presets are still not saved (M5).

## [0.2.0] — 2026-10-13
Milestone M3: side panel & tracks.

### Highlights
- New right-hand **side panel** (from the Figma "sidebar" frame) with Subtitles, Audio, Video and Playback tabs.
- Subtitles stay visible above the controls.

### Added
- Subtitles: track list from mpv, add external subtitle file, delay, size, position.
- Audio: track list, output device, delay, volume boost up to 150%, EQ presets (Flat, Bass, Treble, Voice, Loudness).
- Video: aspect ratio, rotation, zoom, pan & scan, brightness/contrast/saturation/gamma, hardware decoding toggle.
- Playback: speed presets + slider, A-B loop, sleep timer, chapters, in-memory bookmarks (Ctrl+B).

### Fixed
- Subtitles were hidden behind the bottom bar.
- Empty gaps above lists in the side panel.

### Known issues
- Settings, bookmarks and per-show presets are not saved yet (planned for M5).
- Drag & drop does not work on Wayland yet (#5).
- Not yet tested on real Hyprland/GPU hardware; CI and screenshots use Xvfb + software GL.

## [0.1.0] — 2026-10-13
First playable build.

### Added
- Playback via libmpv (OpenGL render API) under a Slint UI (winit + femtovg; Wayland and X11).
- Figma-based bottom bar: thick seek bar with hover time, prev/play/next, volume, fullscreen.
- 201 SVG icons extracted from the Figma "VLC Redesign" file.
- Auto-hiding bars and cursor (2 s), title top bar.
- Empty state with "Open file", drag & drop hint and a Recent placeholder; drag & drop on X11.
- Shortcuts: Space/K, ←/→ (Shift ×60 s), ↑/↓, F, M, Ctrl+O, Esc.
- `hwdec=auto-safe`; falls back to a null audio output when no device is available.
