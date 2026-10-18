# Vivi — full feature reference

> Moved from the old README. Short overview: [README](../README.md).


A fast, lightweight media player written in Rust (Slint + libmpv), inspired by
[Juxtopposed's VLC redesign](https://www.youtube.com/watch?v=ERFeOgqk324). Goals: under 100 MB,
instant startup, a simple interface by default with deep customization when you want it.

![Vivi](screenshots/player.png)

## Features
- Plays nearly any format via libmpv (OpenGL render API), `hwdec=auto-safe`; only the hardware-decoding interop for the GPU vivi renders on is loaded (VA-API on Mesa, CUDA on NVIDIA; override with `[playback] hwdec_interop`)
- Wayland (Hyprland) and X11 — Slint winit + femtovg backend
- UI based on the Figma "VLC Redesign": thick seek bar with hover time, Figma icons, dark theme
- Controls and cursor auto-hide after 2 s of inactivity; subtitles move above the bar when it's shown
- Side panel with **Subtitles / Audio / Video / Playback** tabs:
  - subtitle & audio track selection, external subtitle files, delays, subtitle size/position
  - audio output device, volume boost up to 150%, EQ presets
  - aspect ratio, rotation, zoom, pan & scan, brightness/contrast/saturation/gamma, hwdec toggle
  - speed, A-B loop, sleep timer, chapters, bookmarks (Ctrl+B)
- Miniplayer (`P`): compact always-on-top window (391×339 by default, resizable down to 280×160, size and position remembered) with an optional queue view; tiny sizes show the video only, with hover controls
- Figma-matched menus (main right-click menu with submenus and shortcut hints, file/folder/playlist context menus, sort/view menus), dropdowns, cone volume control and icons, with comparisons in [docs/design/compare](design/compare)
- Responsive layout down to 480×320: the bottom bar folds extra buttons into a “⋯” menu, the side panel overlays the video (full width below 800 px), the library switches to an icon rail and list view, Settings uses a category dropdown — see [docs/screenshots/responsive](screenshots/responsive)
- Seek-bar hover preview: frame thumbnails while hovering the seek bar (needs `ffmpeg`)
- MPRIS2 (Linux): media keys, `playerctl`, Waybar/desktop widgets with title, artist and cover art
- Queue (add, drop, reorder, remove), saved `.m3u` playlists, resume where you left off, recent files
- Open via file dialog (XDG portal), command line (multiple files are queued), or drag & drop (X11, Windows, macOS; on Wayland see below)
- Settings (`Ctrl+,`) with search, Basic/Advanced mode and tooltips: dark/light theme, accent color, resume, default speed, hwdec, preferred audio/subtitle languages, volume cap, subtitle size, auto-load, privacy. Saved to `~/.config/vivi/config.toml`
- Library sidebar with an icon rail and history: home screen (Continue Watching, Listen Again, frequent folders/playlists/artists, Favorites, Recently Added/Played), Video folders with breadcrumb, Network streams, Music by songs/albums/artists/genres, playlists, search, sort, grid/list, right-click menu, thumbnails via ffmpeg
- Open Network Stream (URLs, m3u; YouTube etc. via yt-dlp)
- Customizable bottom bar (16 items, re-order/remove/add) with Minimal / Default / Power profiles
- First-run onboarding: theme, accent color, hotkeys intro (skippable)
- Music visualizer for audio-only files and streams (no video track; cover art does not count), fed from what mpv actually plays, so EQ and audio filters show up: Spectrometer, Scope, Spectrum, Waveform, Radial, Particles and Pitch modes, drawn in the theme accent and reacting to frequencies, beats/onsets, loudness and the dominant note. Pick a mode from the dropdown at the top left or press `V` to cycle; it fills the window in fullscreen (`F`). See [docs/screenshots/visualizer.png](screenshots/visualizer.png)
- Settings window rebuilt per Figma: grouped sidebar (App / Media / System), 13 pages with toggles, accent sliders, segmented controls and path fields, all backed by mpv options
- Plugins (Settings → Extensions): Skip Intro & Credits, synced karaoke Lyrics (LRCLIB / `.lrc`), Subtitle Search (OpenSubtitles), Auto-tagger (AcoustID + MusicBrainz + Cover Art Archive), Free Music (Internet Archive / Jamendo), Network Streams (Internet radio, iptv-org TV, music genres, SomaFM, M3U/IPTV, saved streams), the visualizer, plus downloadable plugins installed and updated from Settings → Extensions → Browse (Discord Rich Presence, Remote Control (phone web page), YouTube & yt-dlp downloads, A-B Loop, Night Mode, Playback Statistics, ListenBrainz Scrobbler, Subtitle Auto-Sync, SponsorBlock, Scene Chapters & Thumbnails) and mpv Lua/JS scripts from `~/.config/vivi/scripts`. See [Plugins](#plugins)
- 10-band equalizer with VLC presets and custom presets, plus video filters (adjust, crop, colour effects) and audio effects (compressor, reverb, delay), saved in `~/.config/vivi/fx.toml`
- Per-show presets: delays, subtitle size/position and track languages applied automatically to every episode

| Library | Side panel |
|---|---|
| <img src="screenshots/library-home.png" width="420"> | <img src="screenshots/panel-subtitles.png" width="420"> |
| **Settings** | **Main menu** |
| <img src="screenshots/settings.png" width="420"> | <img src="screenshots/menu-main.png" width="420"> |
| **Visualizer** | **Lyrics** |
| <img src="screenshots/visualizer.png" width="420"> | <img src="screenshots/lyrics.png" width="420"> |
| **Karaoke lyrics** | **Find Lyrics** |
| <img src="screenshots/lyrics-karaoke.png" width="420"> | <img src="screenshots/lyrics-search.png" width="420"> |
| **Queue** | **Seek preview** |
| <img src="screenshots/queue.png" width="420"> | <img src="screenshots/seek-preview.png" width="420"> |
| **Customize player** | **Light theme** |
| <img src="screenshots/customize.png" width="420"> | <img src="screenshots/light.png" width="420"> |
| **Miniplayer** | **Onboarding** |
| <img src="screenshots/miniplayer.png" width="260"> | <img src="screenshots/onboarding.png" width="420"> |
| **Equalizer** | **Audio effects** |
| <img src="screenshots/panel-audio.png" width="420"> | <img src="screenshots/audio-effects.png" width="420"> |
| **Open Network Stream** | **New Playlist** |
| <img src="screenshots/network.png" width="420"> | <img src="screenshots/new-playlist.png" width="420"> |
| **Video filters** | **Miniplayer queue** |
| <img src="screenshots/panel-video.png" width="420"> | <img src="screenshots/miniplayer-queue.png" width="200"> |

All screenshots are in [docs/screenshots](screenshots) (every settings page is there as `settings-<page>.png`).
They are captured on Xvfb from the release build with `VIVI_SCREENSHOT=1`, which enables the same
env-driven helpers as debug builds (`VIVI_SETTINGS`, `VIVI_PANEL`, `VIVI_LIBRARY`, `VIVI_MENU`, `VIVI_SUBS`, `VIVI_MINI`, …).

> **Design status:** not every frame of the Figma file has been ported yet. Still missing: the equalizer
> UI and filter panels from the components frames, the Open Network Stream / New Playlist dialogs as drawn,
> the miniplayer queue variant and a few other frames in [docs/design/frames](design/frames).
> See the [v0.9 Figma fidelity II milestone](https://github.com/Loafer19/vivi/milestones).

## Install

Linux (x86_64), one line:

```sh
curl -fsSL https://viviplayer.app/install.sh | sh
```

On Arch and derivatives it builds the `vivi-bin` package with `makepkg -si`; elsewhere it puts `vivi`, `vivi-mcp`, the desktop file and icons into `~/.local` and tells you if `libmpv` is missing (e.g. `sudo apt install libmpv2`). Downloads are checked against the release `SHA256SUMS`. Update by running it again; remove with `curl -fsSL https://viviplayer.app/install.sh | sh -s -- --uninstall` (settings and library stay). [Read the script](site/install.sh) first if you like.

| Platform | Package |
|---|---|
| Arch / Omarchy | `packaging/arch/vivi-bin` (prebuilt) or `packaging/arch/vivi` (from source): `makepkg -si`. The release also ships a ready `PKGBUILD` with checksums. |
| Any Linux | `Vivi-<version>-x86_64.AppImage` (slim libmpv bundled, ~44 MB): `chmod +x` and run |
| Linux Flatpak | `vivi-<version>-x86_64.flatpak`: `flatpak install --user vivi-<version>-x86_64.flatpak` (runtime from Flathub). Build it yourself with `packaging/flatpak/` (see `packaging/README.md`) |
| Linux tarball | `vivi-<version>-linux-x86_64.tar.gz` (needs system `mpv`) with `.desktop` file and icons |
| Windows 10/11 | `vivi-<version>-windows-x86_64.zip` — unzip, run `vivi.exe` (`libmpv-2.dll` included) |
| macOS (Apple Silicon, best effort) | `vivi-<version>-macos-arm64.zip` — unsigned `Vivi.app`; right-click → Open the first time |

Downloads: [Releases](https://github.com/Loafer19/vivi/releases) (each with `SHA256SUMS`).

### Set Vivi as the default player (Linux)
`vivi.desktop` declares the common video/audio MIME types and is installed by the packages
(for the tarball: copy it to `~/.local/share/applications/`). Then:
```sh
# all types listed in the desktop file
grep '^MimeType=' /usr/share/applications/vivi.desktop | cut -d= -f2 | tr ';' '\n' \
  | xargs -r xdg-mime default vivi.desktop
# or a single type
xdg-mime default vivi.desktop video/mp4
xdg-mime query default video/x-matroska   # → vivi.desktop
```

### Drag & drop on Wayland
winit (the windowing library under Slint) does not support drag & drop on Wayland yet.
Until it does, run Vivi through XWayland when you want to drop files onto the window:
`VIVI_X11=1 vivi` (or add `VIVI_X11=1` to the `Exec=` line of a copy of `vivi.desktop`).
Opening files from the file manager, the command line or the library works natively.

## Build from source (Arch / Omarchy)
```sh
sudo pacman -S --needed mpv sqlite rust base-devel libxkbcommon wayland mesa xdg-desktop-portal
# optional: ffmpeg (library thumbnails), yt-dlp (YouTube & other sites)
git clone https://github.com/Loafer19/vivi && cd vivi
cargo build --release
./target/release/vivi ~/Videos/film.mkv
```
Debian/Ubuntu: `sudo apt install libmpv-dev libsqlite3-dev libxkbcommon-x11-0 libfontconfig1-dev`.

Debugging: `VIVI_LOG=1` (position/track log), `VIVI_MPV_DEBUG=1` (verbose mpv log),
`VIVI_X11=1` (force XWayland).

## Keyboard shortcuts
| Key | Action |
|---|---|
| Ctrl+K | Command palette |
| Space / K | Play / pause |
| → | Seek forward (Shift: long skip) |
| ← | Seek back (Shift: long skip) |
| ↑ | Volume up |
| ↓ | Volume down |
| F | Fullscreen |
| P | Miniplayer on / off |
| M | Mute |
| S | Stop |
| N / Shift+N | Next / previous in queue |
| Shift+L | A-B loop: set A / set B / clear |
| L | Karaoke lyrics view |
| Ctrl+L | Find lyrics |
| [ / ] | Lyrics timing offset ±0.1 s (Shift: 0.5 s) |
| Z / X | Subtitle offset ±0.1 s (Shift: 1 s), per file |
| V | Next visualizer mode (audio files) |
| R | Start / stop recording |
| Ctrl+\\ | Hide / show the interface |
| Ctrl+Q | Quit |
| Ctrl+O | Open file |
| Ctrl+, | Settings |
| Ctrl+E | Extensions |
| Ctrl+B | Add bookmark |
| ? | Keyboard shortcuts |
| Esc | Close the top dialog or panel, then leave miniplayer / fullscreen |
| Double-click | Fullscreen |
| Click | Play / pause |
| Right-click | Context menu |
| Drop files | Open files or add them to the queue |

## Plugins
Turn plugins on or off and change their settings in the **Extensions** window (`Ctrl+E`, the plugin icon at the bottom of the sidebar, or Settings → Extensions → Open Extensions): one tab per plugin with what it does, where it shows up and its actions, an ON/OFF switch and a gear for its settings, plus **All / Marketplace** to install, update and remove plugins. Source plugins (Network Streams, Free Music, YouTube) are also listed under **Sources** in the sidebar and on Home; hide or reorder them in their settings (gear). Feature plugins add entries where they belong, e.g. Subtitle Search adds *Search online…* to the Subtitles menu. The state is saved in `~/.config/vivi/config.toml` under `[plugins.<id>]`.

| Plugin | What it does |
|---|---|
| **Music Visualizer** | Visuals for audio-only files. Pick a mode with the top-left dropdown, or press `V` to cycle. |
| **Skip Intro & Credits** | Shows a *Skip intro* / *Skip credits* button inside chapters named Intro, Opening, OP, Credits, Ending or ED, or inside your own per-show ranges (`the wand = 0:30-2:00, 21:10-end`). To mark a range while watching, press **Mark** once at the start and again at the end. *Skip automatically* jumps over these parts without asking. |
| **Lyrics** | Synced lyrics as a karaoke overlay, or full screen with `L` (side panel → Playback → Lyrics): the current line large in the accent colour with a word-by-word highlight for enhanced LRC. Sources: your manual pick (`Ctrl+L` *Find Lyrics*: edit artist/title, pick a result by duration and synced/plain), `song.lrc` next to `song.flac`, lyrics embedded in the tags (USLT/SYLT/LYRICS/©lyr), the cache, then [LRCLIB](https://lrclib.net) using the file's tags (music only; plain lyrics when nothing synced exists). `[` / `]` shift the timing by 0.1 s (`Shift`: 0.5 s), saved per track. *Save as .lrc* / *Embed in tags* keep the lyrics with the song (optionally automatic). On radio streams it looks up the current stream title ("Artist - Title", ads and station IDs skipped, results cached) and shows plain lyrics in a scrollable block; *Approximate sync* times synced lyrics from the title change, with a manual offset. Settings: online lookup, keep downloaded lyrics, radio lyrics mode and offset, text size and clear cache. |
| **Subtitle Search** | Side panel → Subtitles → *Find online…* (or the Subtitle menu) opens the Add Subtitles dialog. It searches [OpenSubtitles](https://www.opensubtitles.com) by the file's **hash first** (✓ *Exact match*: made for this very release, no syncing needed), then by name, an edited query or an **IMDb id** (`tt0133093` or an imdb.com link). Pick languages with the chips (several at once, remembered). Results show downloads, rating, FPS and *HI* (hearing impaired), *Trusted* and *Machine* (translated) badges; the side pane has the details and *Preview text*, and *Load* (or double-click) loads it. The pick is remembered per file and loaded again next time; *Save next to the video* keeps it as `Movie.en.srt`. **Per-file offset**: `Z` / `X` move subtitles 0.1 s earlier / later (`Shift`: 1 s), also from the side panel, saved per file and restored when it opens again (Lyrics keep `[` / `]`). *Auto-sync to audio* runs the Subtitle Auto-Sync plugin (alass / ffsubsync with the shared media ffmpeg). Needs a free API key from [opensubtitles.com/consumers](https://www.opensubtitles.com/consumers) in the plugin settings (or `VIVI_OPENSUBTITLES_KEY`); username and password are optional and raise the daily download limit. MCP: `search_subtitles`, `load_subtitle`, `set_subtitle_offset`, `sync_subtitles`. |
| **Auto-tagger** | *Tag file…* (playing file) or *Tag folder…* (a library folder or queue folder) identifies songs by an audio fingerprint on [AcoustID](https://acoustid.org) (Chromaprint: the system `fpcalc` when installed — no extra size — otherwise the built-in port) or, without a key, by their tags / file name on [MusicBrainz](https://musicbrainz.org). Matches are enriched from **Deezer** and **iTunes** (no key), **Last.fm** (genre, API key) and **Discogs** (year, style, cover, token), merged and **ranked by confidence** (score + agreeing sources). The **preview** shows current → proposed per field with the source badge of every value; tick fields on/off or click **⇄** to switch to another source's value; pick a cover candidate (size and resolution shown, click 🔍 to enlarge); see whether **synced or plain LRCLIB lyrics** will be embedded (only when *Embed synced lyrics* is on). Fingerprint matches also propose a **file name** from your pattern (default `{artist} - {title}`) — renaming is an opt-in checkbox, never overwrites a file and replaces characters not allowed in file names. Folders: progress, confident matches only, *Apply all* / *Skip*; every write is backed up (`~/.local/share/vivi/tag-backups/`) and **Undo** restores tags, covers and names. Keys come from the plugin settings or the environment (`VIVI_ACOUSTID_KEY`, `VIVI_LASTFM_KEY`, `VIVI_DISCOGS_TOKEN`). ![Auto-tagger review](screenshots/tagger-review.png) |
| **Free Music** | *Browse…* searches and browses Creative Commons / public-domain music from [Jamendo](https://www.jamendo.com) (free client ID from [devportal.jamendo.com](https://devportal.jamendo.com)), the [Internet Archive](https://archive.org) and the Free Music Archive (via its Internet Archive mirror; both need no key), merged in one list with source badges, covers and licence chips. Genre chips, Popular / New / Featured, mood, licence (commercial use, CC BY, BY-SA, CC0), length and sort filters, infinite scroll, album / artist pages, *Similar* (radio-like), queue, *Copy credit*. *Download* saves to `~/Music/Free Music/<Artist>/` (configurable, or hands off to Offline Downloads) with tags, cover, the licence URL and an attribution line in the tags and in a `.licence.txt` file next to it. |
| **Network Streams** | *Browse…* opens a streams browser (separate from *Open Network Stream*): Internet radio from [radio-browser.info](https://www.radio-browser.info) (search by name, top voted, by country or country code, by genre; no key), **M3U / IPTV** import from a file path or URL (`tvg-logo`, `group-title`; a plain word searches iptv-org channels), **TV** by country or category from [iptv-org](https://github.com/iptv-org/iptv) (cached playlists, filter as you type, channel logos), **Music** genres (popular, trending, top stations per tag) and [SomaFM](https://somafm.com) channels, **Saved** and **Recent** lists, shown as a list or as a grid of logos (header toggle, remembered; arrow keys, Home / End and Enter work in both). *Play* plays the selection, *Save* / *Remove* manage the saved list. *Stream info* shows the ICY now-playing title, codec, bitrate and buffer. Lists are kept in `~/.local/share/vivi/streams/`. |

### Downloadable plugins
Settings → Extensions has two tabs. **Installed** lists built-in plugins, downloaded plugins (with Update, Roll back and Uninstall) and mpv scripts. **Browse** shows the catalog from <https://plugins.viviplayer.app/index.json> (falls back to <https://loafer19.github.io/vivi-plugins/index.json>) with Install / Update / Uninstall buttons, version, author, size and the permissions each plugin asks for (you confirm them before installing). Downloads run in the background (the size shown includes a plugin's native helper for your platform), are checked against the sha256 in the catalog and are installed atomically into `~/.local/share/vivi/plugins/<id>/<version>/`; the previous version is kept for rollback. Vivi checks for updates at most once a day on startup (can be turned off) or when you press *Check now*, and shows a notification when updates are available.

| Package | What it does |
| --- | --- |
| **Discord Rich Presence** (`discord`) | Off by default. Shows title, artist and progress (“Listening to …” / “Watching …”) in Discord. Needs the **Application ID** of a Discord application you create at [discord.com/developers/applications](https://discord.com/developers/applications); *Hide titles and file names* shows only “Watching a video” / “Listening to music”. Ships the native helper `vivi-discord` (Linux x86_64/aarch64, Windows x64, macOS), which talks to Discord's IPC socket (`$XDG_RUNTIME_DIR/discord-ipc-0`, Flatpak/Snap paths, Windows named pipe) and reconnects when Discord starts later. |
| **Remote Control** (`remote-control`) | Off by default. A phone-friendly page at `http://<this computer's LAN IP>:8765` (port setting) with play/pause, ±10 s, seek, volume, previous/next, the queue (tap to play) and cover art, served by the native helper `vivi-remote`. Only loopback, private and link-local addresses may connect; every API call needs the PIN (random 6 digits, saved, unless you set one; *Show* gives URL and PIN); 5 wrong PINs block an address for a minute. |
| **YouTube & yt-dlp** (`youtube`) | Paste a video or playlist URL into its settings (or copy it and press `Alt+U`): *Add to playlist* lists the entries in the Vivi playlist, *Show sizes* gives the estimated size per quality (yt-dlp `filesize`/`filesize_approx`), *Download* saves video (quality choice) or audio only (original, mp3, m4a or opus) into a folder you pick (*Choose…*; default `~/Downloads/Vivi`) with progress toasts; *Cancel* stops it. Uses a system yt-dlp released in the last 90 days if there is one (PATH and common folders), otherwise downloads its own copy from the [official releases](https://github.com/yt-dlp/yt-dlp/releases) (checked against `SHA2-256SUMS`, 18–40 MB) into `~/.local/share/vivi/plugin-data/youtube/bin` and updates it daily (`yt-dlp -U`; *yt-dlp to use*: auto / system / managed). The chosen yt-dlp is also used for streaming URLs, and Vivi finds the managed copy on its own (this is how the Flatpak gets yt-dlp). ffmpeg (merging 1080p+ video with audio, mp3/m4a/opus conversion) is also taken from the system, or Vivi's own verified minimal static build (about 6 MB, release `helper-ffmpeg-v8.0-1`) is downloaded only when such a download needs it and kept up to date; below 1080p without ffmpeg single-file formats are used. |
| **A-B Loop & Frame Step** (`ab-loop`) | `Alt+A` / `Alt+B` set loop points, `Alt+C` clears, `Alt+←/→` nudge the last point by a frame, `,` / `.` step a frame while paused; an on-screen bar shows the loop. |
| **Night Mode** (`night-mode`) | Dynamic range compression (mpv `dynaudnorm`, or EBU R128 `loudnorm`) so dialogue stays audible at low volume. Strength setting, `Alt+N` toggles. |
| **ListenBrainz Scrobbler** (`scrobbler`) | Sends *now playing* and listens to [ListenBrainz](https://listenbrainz.org) after half the track or 4 minutes (tracks under 30 s skipped). Offline listens are queued and sent later. Needs your user token from [listenbrainz.org/settings](https://listenbrainz.org/settings/) and `curl`. |
| **Subtitle Auto-Sync** (`subtitle-sync`) | `Alt+Y` aligns the selected external subtitle to the audio with [alass](https://github.com/kaegi/alass) or [ffsubsync](https://github.com/smacke/ffsubsync) (install one: `cargo install alass-cli` or `pip install ffsubsync`) and loads `<name>.synced.<ext>`. |
| **SponsorBlock** (`sponsorblock`) | Skips [SponsorBlock](https://sponsor.ajay.app) segments in YouTube streams (auto or `Alt+K`), categories configurable, segments shown as chapters. No key; needs `curl` and yt-dlp for YouTube playback. |
| **Scene Chapters & Thumbnails** (`scene-chapters`) | `Alt+J` (or automatic) adds scene-change chapters with ffmpeg to videos without chapters and saves chapter thumbnails, cached on disk. Needs `ffmpeg`. |
| **Playback Statistics** (`playback-stats`) | Time watched/listened per day, top artists and shows, kept locally; `Alt+S` shows a summary, export to `~/vivi-stats.csv`, clear. |

Packages are mpv Lua scripts with a `manifest.toml`, kept in the separate **[Loafer19/vivi-plugins](https://github.com/Loafer19/vivi-plugins)** repository (together with their native helpers, tests and the marketplace) and published there as release assets tagged `plugin-<id>-v<version>`. Plugin releases made before the split were removed from this repository; installed plugins keep working and update from vivi-plugins. Design and rationale: [docs/plugins-architecture.md](plugins-architecture.md). How to publish your own: [Package & publish](https://plugins.viviplayer.app/dev/#package).

### mpv scripts
Any mpv Lua or JavaScript script placed in `~/.config/vivi/scripts/` shows up as an **external plugin** with its own toggle. It is loaded with mpv `load-script`, and turning one off takes effect after a restart. Optional `-- @name`, `@description`, `@version` and `@author` header lines fill in the card. See [plugins/examples/now-playing.lua](https://github.com/Loafer19/vivi-plugins/blob/master/plugins/examples/now-playing.lua).

The machine-readable catalog is [plugins/index.json](https://github.com/Loafer19/vivi-plugins/blob/master/plugins/index.json) in vivi-plugins. The docs site in [docs/site](site) has a landing page, the plugin catalog and a guide to writing plugins. Build it with `python3 scripts/build-site.py` (output in `_site/`). The [Pages workflow](../.github/workflows/pages.yml) deploys a mirror to <https://loafer19.github.io/vivi/> on every push to `master`; the main copy is **<https://viviplayer.app>** and the plugin marketplace **<https://plugins.viviplayer.app>** (built from [vivi-plugins](https://github.com/Loafer19/vivi-plugins)). Both run on Dokploy, see [deploy/](../deploy). Site icons are [Lucide](https://lucide.dev) SVGs inlined at build time (no JavaScript); each catalog entry names its Lucide icon in the `lucide` field of `index.json`.

| Extensions | Lyrics | Skip intro | Add Subtitles |
|---|---|---|---|
| <img src="screenshots/extensions.png" width="220"> | <img src="screenshots/lyrics.png" width="220"> | <img src="screenshots/skip-intro.png" width="220"> | <img src="screenshots/add-subtitles.png" width="220"> |

## AI agents (MCP)

`vivi-mcp` is a small stdio [MCP](https://modelcontextprotocol.io) server, built alongside
Vivi, that lets Claude Desktop, Cursor and other agents control a running player (play,
seek, volume, open files/URLs, queue, library search, subtitles, EQ presets, now playing).
Enable *Settings → General → Remote control* (or run `VIVI_IPC=1 vivi`), then add
`{"mcpServers": {"vivi": {"command": "vivi-mcp"}}}` to your client. See
[docs/mcp.md](mcp.md).

## Architecture
`src/player.rs` — libmpv wrapper (event thread → channel) · `src/render.rs` — mpv rendered under Slint ·
`src/viz/` — music visualizer (an mpv `af` lavfi tap on the real output, after EQ/filters, sending per-band levels with timestamps over a localhost socket and synced to `time-pos` → rustfft analysis → modes behind the `Visual` trait) ·
`src/plugins/` — plugin registry (`Plugin` trait, manifests, config-backed enable/settings, mpv script loader) and the built-in plugins ·
`ui/*.slint` — screens and dialogs · `ui/core/` — design tokens (`Theme` colours, `Metrics` spacing / radii / font sizes), `Icons`, `Format` and the shared components (buttons, rows, slider, modal, form controls) · `assets/icons` — SVGs extracted from Figma.
Design notes: [docs/design/FIGMA.md](design/FIGMA.md) · research: [docs/RESEARCH.md](RESEARCH.md).

## Roadmap & status
Stages are tracked as [milestones](https://github.com/Loafer19/vivi/milestones), tasks as [issues](https://github.com/Loafer19/vivi/issues).
- [x] M1 Playback
- [x] M2 Figma UI & icons
- [x] M3 Side panel & tracks
- [x] M4 Queue / playlists / resume
- [x] M5 Settings & customization
- [x] M6 Library
- [x] M7 Packaging
- [x] v0.7 Desktop integration (Flatpak moved to v0.9)
- [x] v0.8 Figma fidelity: settings window, visualizer, plugin system, menus, library, icons
- [ ] v0.9 Figma fidelity II: equalizer & filter panels, dialogs, miniplayer queue, remaining frames, Flatpak
- [ ] Plugins: 6 built-in plugins and 4 downloadable packages done, more planned (subtitle auto-sync, Discord, remote control, Last.fm, SponsorBlock, …) — label [`plugin`](https://github.com/Loafer19/vivi/labels/plugin)

## License
MIT — see [LICENSE](../LICENSE).

## Credits
- UI design and app icons: Figma community file “VLC Redesign” by [Juxtopposed](https://www.figma.com/@juxtopposed)
- Site icons: [Lucide](https://lucide.dev) (ISC license)
- Screenshot video: [Sintel](https://durian.blender.org) © Blender Foundation, [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) — trailer from download.blender.org
- Screenshot music: “Local Forecast – Elevator” by Kevin MacLeod ([incompetech.com](https://incompetech.com)), [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); the lyrics in the screenshots are demo text written for them
- Playback: [mpv](https://mpv.io) · UI toolkit: [Slint](https://slint.dev)
