# Vivi

A fast, lightweight media player written in Rust (Slint + libmpv), inspired by
[Juxtopposed's VLC redesign](https://www.youtube.com/watch?v=ERFeOgqk324). Goals: under 100 MB,
instant startup, a simple interface by default with deep customization when you want it.

![Vivi](docs/screenshots/player.png)

## Features
- Plays nearly any format via libmpv (OpenGL render API), `hwdec=auto-safe`
- Wayland (Hyprland) and X11 — Slint winit + femtovg backend
- UI based on the Figma "VLC Redesign": thick seek bar with hover time, Figma icons, dark theme
- Controls and cursor auto-hide after 2 s of inactivity; subtitles move above the bar when it's shown
- Side panel with **Subtitles / Audio / Video / Playback** tabs:
  - subtitle & audio track selection, external subtitle files, delays, subtitle size/position
  - audio output device, volume boost up to 150%, EQ presets
  - aspect ratio, rotation, zoom, pan & scan, brightness/contrast/saturation/gamma, hwdec toggle
  - speed, A-B loop, sleep timer, chapters, bookmarks (Ctrl+B)
- Miniplayer (`P`): compact always-on-top window (391×339 by default, resizable down to 280×160, size and position remembered) with an optional queue view; tiny sizes show the video only, with hover controls
- Figma-matched menus (main right-click menu with submenus and shortcut hints, file/folder/playlist context menus, sort/view menus), dropdowns, cone volume control and icons, with comparisons in [docs/design/compare](docs/design/compare)
- Responsive layout down to 480×320: the bottom bar folds extra buttons into a “⋯” menu, the side panel overlays the video (full width below 800 px), the library switches to an icon rail and list view, Settings uses a category dropdown — see [docs/screenshots/responsive](docs/screenshots/responsive)
- Seek-bar hover preview: frame thumbnails while hovering the seek bar (needs `ffmpeg`)
- MPRIS2 (Linux): media keys, `playerctl`, Waybar/desktop widgets with title, artist and cover art
- Queue (add, drop, reorder, remove), saved `.m3u` playlists, resume where you left off, recent files
- Open via file dialog (XDG portal), command line (multiple files are queued), or drag & drop (X11, Windows, macOS; on Wayland see below)
- Settings (`Ctrl+,`) with search, Basic/Advanced mode and tooltips: dark/light theme, accent color, resume, default speed, hwdec, preferred audio/subtitle languages, volume cap, subtitle size, auto-load, privacy. Saved to `~/.config/vivi/config.toml`
- Library sidebar with an icon rail and history: home screen (Continue Watching, Listen Again, frequent folders/playlists/artists, Favorites, Recently Added/Played), Video folders with breadcrumb, Network streams, Music by songs/albums/artists/genres, playlists, search, sort, grid/list, right-click menu, thumbnails via ffmpeg
- Open Network Stream (URLs, m3u; YouTube etc. via yt-dlp)
- Customizable bottom bar (16 items, re-order/remove/add) with Minimal / Default / Power profiles
- First-run onboarding: theme, accent color, hotkeys intro (skippable)
- Music visualizer for audio-only files (no video track; cover art does not count): Spectrometer, Scope, Spectrum, Waveform, Radial, Particles and Pitch modes, drawn in the theme accent and reacting to frequencies, beats/onsets, loudness and the dominant note. Pick a mode from the dropdown at the top left or press `V` to cycle; it fills the window in fullscreen (`F`). See [docs/screenshots/visualizer.png](docs/screenshots/visualizer.png)
- Settings window rebuilt per Figma: grouped sidebar (App / Media / System), 13 pages with toggles, accent sliders, segmented controls and path fields, all backed by mpv options
- Plugins (Settings → Extensions): Skip Intro & Credits, synced karaoke Lyrics (LRCLIB / `.lrc`), Subtitle Search (OpenSubtitles), Auto-tagger (AcoustID + MusicBrainz + Cover Art Archive), Free Music (Internet Archive / Jamendo), Network Streams (Internet radio, M3U/IPTV, saved streams), Discord Rich Presence, Remote Control (phone web page), the visualizer, plus downloadable plugins installed and updated from Settings → Extensions → Browse (A-B Loop, Night Mode, Playback Statistics, ListenBrainz Scrobbler, Subtitle Auto-Sync, SponsorBlock, Scene Chapters & Thumbnails) and mpv Lua/JS scripts from `~/.config/vivi/scripts`. See [Plugins](#plugins)
- Per-show presets: delays, subtitle size/position and track languages applied automatically to every episode

| Library | Side panel |
|---|---|
| <img src="docs/screenshots/library-home.png" width="420"> | <img src="docs/screenshots/panel-subtitles.png" width="420"> |
| **Settings** | **Main menu** |
| <img src="docs/screenshots/settings.png" width="420"> | <img src="docs/screenshots/menu-main.png" width="420"> |
| **Visualizer** | **Lyrics** |
| <img src="docs/screenshots/visualizer.png" width="420"> | <img src="docs/screenshots/lyrics.png" width="420"> |
| **Queue** | **Seek preview** |
| <img src="docs/screenshots/queue.png" width="420"> | <img src="docs/screenshots/seek-preview.png" width="420"> |
| **Customize player** | **Light theme** |
| <img src="docs/screenshots/customize.png" width="420"> | <img src="docs/screenshots/light.png" width="420"> |
| **Miniplayer** | **Onboarding** |
| <img src="docs/screenshots/miniplayer.png" width="260"> | <img src="docs/screenshots/onboarding.png" width="420"> |

All screenshots are in [docs/screenshots](docs/screenshots) (every settings page is there as `settings-<page>.png`).
They are captured on Xvfb from the release build with `VIVI_SCREENSHOT=1`, which enables the same
env-driven helpers as debug builds (`VIVI_SETTINGS`, `VIVI_PANEL`, `VIVI_LIBRARY`, `VIVI_MENU`, `VIVI_SUBS`, `VIVI_MINI`, …).

> **Design status:** not every frame of the Figma file has been ported yet. Still missing: the equalizer
> UI and filter panels from the components frames, the Open Network Stream / New Playlist dialogs as drawn,
> the miniplayer queue variant and a few other frames in [docs/design/frames](docs/design/frames).
> See the [v0.9 Figma fidelity II milestone](https://github.com/Loafer19/vivi/milestones).

## Install

| Platform | Package |
|---|---|
| Arch / Omarchy | `packaging/arch/vivi-bin` (prebuilt) or `packaging/arch/vivi` (from source): `makepkg -si`. The release also ships a ready `PKGBUILD` with checksums. |
| Any Linux | `Vivi-<version>-x86_64.AppImage` (slim libmpv bundled, ~35 MB): `chmod +x` and run |
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
| Space / K | Play / pause |
| ← / → | Seek −5 / +5 s (Shift: 60 s) |
| ↑ / ↓ | Volume ±5 |
| F, double-click | Fullscreen |
| M | Mute |
| V | Next visualizer mode (audio-only files) |
| P | Miniplayer on / off |
| Ctrl+O | Open file |
| Ctrl+B | Add bookmark |
| Ctrl+, | Settings |
| Esc | Leave miniplayer / fullscreen, close settings |

## Plugins
Turn plugins on or off and change their settings in **Settings → Extensions** (`Ctrl+,`). Each card has an ON/OFF toggle, and a plugin's settings appear below its card while it is on. The state is saved in `~/.config/vivi/config.toml` under `[plugins.<id>]`.

| Plugin | What it does |
|---|---|
| **Music Visualizer** | Visuals for audio-only files. Pick a mode with the top-left dropdown, or press `V` to cycle. |
| **Skip Intro & Credits** | Shows a *Skip intro* / *Skip credits* button inside chapters named Intro, Opening, OP, Credits, Ending or ED, or inside your own per-show ranges (`the wand = 0:30-2:00, 21:10-end`). To mark a range while watching, press **Mark** once at the start and again at the end. *Skip automatically* jumps over these parts without asking. |
| **Lyrics** | Synced lyrics as a karaoke overlay. It looks for `song.lrc` next to `song.flac` first, then a cache, then [LRCLIB](https://lrclib.net) using the file's tags (music only). Settings: online lookup, text size and clear cache. |
| **Subtitle Search** | Side panel → *Add subtitle file…* (or the Subtitle menu) opens the Add Subtitles dialog. It searches [OpenSubtitles](https://www.opensubtitles.com) by file hash and name, then downloads and loads the subtitle you pick. It needs a free API key from [opensubtitles.com/consumers](https://www.opensubtitles.com/consumers), entered in the plugin settings. Username and password are optional and raise the daily download limit. |
| **Auto-tagger** | *Tag file…* (playing file) or *Tag folder…* (a library folder) opens a preview dialog: songs are identified by an audio fingerprint on [AcoustID](https://acoustid.org) (Chromaprint: `fpcalc` if installed, otherwise built in) or, without a key, by their tags / file name on [MusicBrainz](https://musicbrainz.org). Pick a candidate to see a field-by-field diff (title, artist, album, album artist, date, track, genre, MusicBrainz ids), then *Write tags* (or *Write all* for a folder; only confident matches are listed). The front cover comes from the [Cover Art Archive](https://coverartarchive.org). The original tags are backed up once to `~/.local/share/vivi/tag-backups/`; *Restore* puts them back. Fingerprint matching needs a free AcoustID application key from [acoustid.org/new-application](https://acoustid.org/new-application) (plugin setting). |
| **Free Music** | *Browse…* searches Creative Commons / public-domain music on the [Internet Archive](https://archive.org) (no key) or [Jamendo](https://www.jamendo.com) (free client ID from [devportal.jamendo.com](https://devportal.jamendo.com)). Each result shows its licence; *Preview* streams it in the player, *Download* saves it to `~/Music/Free Music/<Artist>/` (configurable) with tags, cover, the licence URL and an attribution line in the tags and in a `.licence.txt` file next to it. |
| **Network Streams** | *Browse…* opens a streams browser (separate from *Open Network Stream*): Internet radio from [radio-browser.info](https://www.radio-browser.info) (search by name, top voted, by country or country code, by genre; no key), **M3U / IPTV** import from a file path or URL (`tvg-logo`, `group-title`), **Saved** and **Recent** lists. *Play* plays the selection, *Save* / *Remove* manage the saved list. *Stream info* shows the ICY now-playing title, codec, bitrate and buffer. Lists are kept in `~/.local/share/vivi/streams/`. |
| **Discord Rich Presence** | Off by default. Shows title, artist and progress (“Listening to …” for music, “Watching …” for video) in Discord over its local IPC socket (`$XDG_RUNTIME_DIR/discord-ipc-0`, Flatpak/Snap paths, Windows named pipe). Needs the **Application ID** of a Discord application you create at [discord.com/developers/applications](https://discord.com/developers/applications) (its name is what Discord shows). *Hide titles and file names* shows only “Watching a video” / “Listening to music”. Updates on file load, pause and seek; reconnects when Discord starts later. |
| **Remote Control** | Off by default. Serves a phone-friendly page at `http://<this computer's LAN IP>:8765` (port setting) with play/pause, ±10 s, seek, volume, previous/next, the queue (tap to play) and cover art. Only loopback, private and link-local addresses may connect; every API call needs the PIN (random 6 digits unless you set one; *Phone address* shows URL and PIN), 5 wrong PINs block an address for a minute. |

### Downloadable plugins
Settings → Extensions has two tabs. **Installed** lists built-in plugins, downloaded plugins (with Update, Roll back and Uninstall) and mpv scripts. **Browse** shows the catalog from <https://loafer19.github.io/vivi/index.json> with Install / Update / Uninstall buttons, version, author, size and the permissions each plugin asks for (you confirm them before installing). Downloads run in the background, are checked against the sha256 in the catalog and are installed atomically into `~/.local/share/vivi/plugins/<id>/<version>/`; the previous version is kept for rollback. Vivi checks for updates at most once a day on startup (can be turned off) or when you press *Check now*, and shows a notification when updates are available.

| Package | What it does |
| --- | --- |
| **A-B Loop & Frame Step** (`ab-loop`) | `Alt+A` / `Alt+B` set loop points, `Alt+C` clears, `Alt+←/→` nudge the last point by a frame, `,` / `.` step a frame while paused; an on-screen bar shows the loop. |
| **Night Mode** (`night-mode`) | Dynamic range compression (mpv `dynaudnorm`, or EBU R128 `loudnorm`) so dialogue stays audible at low volume. Strength setting, `Alt+N` toggles. |
| **ListenBrainz Scrobbler** (`scrobbler`) | Sends *now playing* and listens to [ListenBrainz](https://listenbrainz.org) after half the track or 4 minutes (tracks under 30 s skipped). Offline listens are queued and sent later. Needs your user token from [listenbrainz.org/settings](https://listenbrainz.org/settings/) and `curl`. |
| **Subtitle Auto-Sync** (`subtitle-sync`) | `Alt+Y` aligns the selected external subtitle to the audio with [alass](https://github.com/kaegi/alass) or [ffsubsync](https://github.com/smacke/ffsubsync) (install one: `cargo install alass-cli` or `pip install ffsubsync`) and loads `<name>.synced.<ext>`. |
| **SponsorBlock** (`sponsorblock`) | Skips [SponsorBlock](https://sponsor.ajay.app) segments in YouTube streams (auto or `Alt+K`), categories configurable, segments shown as chapters. No key; needs `curl` and yt-dlp for YouTube playback. |
| **Scene Chapters & Thumbnails** (`scene-chapters`) | `Alt+J` (or automatic) adds scene-change chapters with ffmpeg to videos without chapters and saves chapter thumbnails, cached on disk. Needs `ffmpeg`. |
| **Playback Statistics** (`playback-stats`) | Time watched/listened per day, top artists and shows, kept locally; `Alt+S` shows a summary, export to `~/vivi-stats.csv`, clear. |

Packages are mpv Lua scripts with a `manifest.toml`, kept in [plugins/](plugins) and published by the [Plugins workflow](.github/workflows/plugins.yml) as release assets tagged `plugin-<id>-v<version>`. Design and rationale: [docs/plugins-architecture.md](docs/plugins-architecture.md). How to publish your own: [Package & publish](https://loafer19.github.io/vivi/plugin-dev.html#package).

### mpv scripts
Any mpv Lua or JavaScript script placed in `~/.config/vivi/scripts/` shows up as an **external plugin** with its own toggle. It is loaded with mpv `load-script`, and turning one off takes effect after a restart. Optional `-- @name`, `@description`, `@version` and `@author` header lines fill in the card. See [plugins/examples/now-playing.lua](plugins/examples/now-playing.lua).

The machine-readable catalog is [plugins/index.json](plugins/index.json). The docs site in [docs/site](docs/site) has a landing page, the plugin catalog and a guide to writing plugins. Build it with `python3 scripts/build-site.py` (output in `_site/`). The [Pages workflow](.github/workflows/pages.yml) deploys it to **<https://loafer19.github.io/vivi/>** on every push to `master`. Site icons are [Lucide](https://lucide.dev) SVGs inlined at build time (no JavaScript); each catalog entry names its Lucide icon in the `lucide` field of `index.json`.

| Extensions | Lyrics | Skip intro | Add Subtitles |
|---|---|---|---|
| <img src="docs/screenshots/extensions.png" width="220"> | <img src="docs/screenshots/lyrics.png" width="220"> | <img src="docs/screenshots/skip-intro.png" width="220"> | <img src="docs/screenshots/add-subtitles.png" width="220"> |

## Architecture
`src/player.rs` — libmpv wrapper (event thread → channel) · `src/render.rs` — mpv rendered under Slint ·
`src/viz/` — music visualizer (a parallel symphonia decode synced to mpv `time-pos` → rustfft analysis → modes behind the `Visual` trait) ·
`src/plugins/` — plugin registry (`Plugin` trait, manifests, config-backed enable/settings, mpv script loader) and the built-in plugins ·
`ui/*.slint` — UI and design tokens · `assets/icons` — SVGs extracted from Figma.
Design notes: [docs/design/FIGMA.md](docs/design/FIGMA.md) · research: [docs/RESEARCH.md](docs/RESEARCH.md).

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
MIT — see [LICENSE](LICENSE).

## Credits
- UI design and app icons: Figma community file “VLC Redesign” by [Juxtopposed](https://www.figma.com/@juxtopposed)
- Site icons: [Lucide](https://lucide.dev) (ISC license)
- Screenshot video: [Sintel](https://durian.blender.org) © Blender Foundation, [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) — trailer from download.blender.org
- Screenshot music: “Local Forecast – Elevator” by Kevin MacLeod ([incompetech.com](https://incompetech.com)), [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); the lyrics in the screenshots are demo text written for them
- Playback: [mpv](https://mpv.io) · UI toolkit: [Slint](https://slint.dev)
