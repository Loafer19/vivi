# Changelog

## [Unreleased]

### Added
- **Downloadable plugins**: plugins no longer have to be built in. A package is a zip with `manifest.toml` (id, name, semver version, `min_vivi`, entry script, permissions, settings) and an mpv Lua/JS script. The catalog `plugins/index.json` (published at <https://loafer19.github.io/vivi/index.json>) lists each package's download URL, sha256 and size. Design, options considered (mpv script packages now, WASM later, native libraries rejected) and the install/update model: `docs/plugins-architecture.md` and the site page *Plugin distribution*.
- **Settings → Extensions tabs**: *Installed* (built-in, downloaded, mpv scripts; Update / Roll back / Uninstall for packages) and *Browse* (catalog cards with version, author, size, permissions and Install / Update / Uninstall). Installing shows the requested permissions first. Downloads and catalog checks run on background threads; packages are verified against their sha256, unpacked safely into a staging folder and switched in atomically under `~/.local/share/vivi/plugins/<id>/<version>/`, keeping the previous version for rollback.
- **Update checks**: the catalog is fetched at startup at most once a day ("Check for plugin updates daily", on by default) or with *Check now*, cached in `~/.cache/vivi/plugin-index.json`; a toast announces available updates. `VIVI_PLUGIN_INDEX` overrides the catalog URL.
- Package settings appear in Extensions like built-in ones and reach the script as mpv `script-opts` (applied live); action settings are sent as script messages.
- Three downloadable plugins: **A-B Loop & Frame Step** (#59), **Night Mode** (#60), **Playback Statistics** (#66).
- Keys Vivi does not use (and all `Alt` combinations) are forwarded to mpv, so key bindings of scripts and packages work.
- `scripts/package-plugins.py` builds deterministic package zips and regenerates the index entries; the **Plugins workflow** checks Lua syntax and the index, publishes each new version as a release `plugin-<id>-v<version>` (never marked latest) and verifies the uploaded sha256.
### Fixed
- Subtitle Search: OpenSubtitles answers `301` unless query parameters are sorted by name, so searches sent `moviehash`/`query`/`languages` in the wrong order and relied on a redirect. Parameters (and the language list) are now sent sorted and lowercase.

### Docs
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
