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
  - per-file subtitle offset: `Z` / `X` (or `Alt+Z` / `Alt+X`) move subtitles 0.1 s earlier / later (`Shift`: 1 s), also with the Offset buttons; saved per file and restored when it opens again, with or without the Subtitle Search plugin (Lyrics keep `[` / `]`)
  - audio output device, volume boost up to 150%, EQ presets
  - aspect ratio, rotation, zoom, pan & scan, brightness/contrast/saturation/gamma, hwdec toggle
  - speed, A-B loop, sleep timer, chapters, bookmarks (Ctrl+B)
- A-B loop and frame stepping: `Shift+L` cycles set A / set B / clear, `Alt+A` / `Alt+B` set a point at the playhead (a point on the wrong side of the other swaps them), `Alt+C` clears, `Alt+←` / `Alt+→` move the point set last by one frame, `,` / `.` step one frame while paused (1 s while playing). The loop shows on the seek bar (drag its edges to adjust) and in the side panel; changes are confirmed by a toast, and points are cleared when the next file loads
- Miniplayer (`P`): compact always-on-top window (391×339 by default, resizable down to 280×160, size and position remembered) with an optional queue view; tiny sizes show the video only, with hover controls
- Figma-matched menus (main right-click menu with submenus and shortcut hints, file/folder/playlist context menus, sort/view menus), dropdowns, cone volume control and icons, with comparisons in [docs/design/compare](design/compare)
- Responsive layout down to 480×320: the bottom bar folds extra buttons into a “⋯” menu, the side panel overlays the video (full width below 800 px), the library switches to an icon rail and list view, Settings uses a category dropdown — see [docs/screenshots/responsive](screenshots/responsive)
- Seek-bar hover preview: frame thumbnails while hovering the seek bar, from the Chapter Thumbnails extension (the player offers it the first time you hover the seek bar of a video)
- MPRIS2 (Linux): media keys, `playerctl`, Waybar/desktop widgets with title, artist and cover art
- Queue (add, drop, reorder, remove), saved `.m3u` playlists, resume where you left off, recent files
- Open via file dialog (XDG portal), command line (multiple files are queued), or drag & drop (X11, Windows, macOS; on Wayland see below)
- Settings (`Ctrl+,`) with search, Basic/Advanced mode and tooltips: dark/light theme, accent color, resume, default speed, hwdec, preferred audio/subtitle languages, volume cap, subtitle size, auto-load, privacy. Saved to `~/.config/vivi/config.toml`
- Library sidebar with an icon rail and history: home screen (Continue Watching, Listen Again, frequent folders/playlists/artists, Favorites, Recently Added/Played), Video folders with breadcrumb, Streams, Music by songs/albums/artists/genres, playlists, search, sort, grid/list, right-click menu, thumbnails via ffmpeg
- Streams hub (Library → Streams): open a URL (m3u, YouTube etc. via yt-dlp) or search from one bar with history; Recent, Favorites, Radio, TV and Music tabs with the same cards and ★ everywhere; a now-playing card with ★, lyrics, record and download
- Customizable bottom bar (16 items, re-order/remove/add) with Minimal / Default / Power profiles
- Player bar icons (Settings → General → Appearance): the icons of the player's top and bottom bars, including the top-right close button, are two-tone by default or single-tone; applies at once and is saved as `interface.bar_icons`
- First-run onboarding: theme, accent color, hotkeys intro (skippable)
- Music visualizer for audio-only files and streams (no video track; cover art does not count), fed from what mpv actually plays, so EQ and audio filters show up: Spectrometer, Scope, Spectrum, Waveform, Radial, Particles and Pitch modes, drawn in the theme accent (or the main color of the song's album art) and reacting to frequencies, beats/onsets, loudness and the dominant note. Pick a mode from the dropdown at the top left, the side panel (Playback → Display) or the command palette, or press `V` / `Shift+V` to cycle; it fills the window in fullscreen (`F`). See [docs/screenshots/visualizer.png](screenshots/visualizer.png)
- Settings window rebuilt per Figma: grouped sidebar (App / Media / System), 13 pages with toggles, accent sliders, segmented controls and path fields, all backed by mpv options
- Plugins (Settings → Extensions): Skip Intro & Credits, synced karaoke Lyrics (LRCLIB / `.lrc`), Subtitle Search (OpenSubtitles), Auto-tagger (AcoustID + MusicBrainz + Cover Art Archive), Free Music (Internet Archive / Jamendo), Network Streams (Internet radio, iptv-org TV, music genres, SomaFM, M3U/IPTV, saved streams), the visualizer, plus downloadable plugins installed and updated from Settings → Extensions → Browse (Discord Rich Presence, Remote Control (phone web page), YouTube & yt-dlp downloads, Night Mode, Playback Statistics, ListenBrainz Scrobbler, Subtitle Auto-Sync, SponsorBlock, Chapters & Previews with scene chapters) and mpv Lua/JS scripts from `~/.config/vivi/scripts`. See [Plugins](#plugins)
- 10-band equalizer with VLC presets (plus Bass, Vocal and Night) and custom presets, `Alt+E` to cycle presets and `Alt+Shift+E` to turn it on / off, an *Auto pre-amp / prevent clipping* option (on by default: the preamp drops by the largest band boost), plus video filters (adjust, crop, colour effects) and audio effects (compressor, reverb, delay), saved in `~/.config/vivi/fx.toml`
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
| Alt+A | A-B loop: set A here |
| Alt+B | A-B loop: set B here |
| Alt+C | A-B loop: clear |
| Alt+← / Alt+→ | A-B loop: nudge the last set point by one frame |
| , / . | One frame back / forward (paused; 1 s while playing) |
| L | Karaoke lyrics view |
| Ctrl+L | Find lyrics |
| [ / ] | Lyrics timing offset ±0.1 s (Shift: 0.5 s) |
| Alt+Z / Alt+X | Subtitle offset ±0.1 s (Alt+Shift: 1 s), per file |
| Z / X | Subtitle offset ±0.1 s (Shift: 1 s), per file |
| V / Shift+V | Next / previous visualizer mode (music and radio) |
| R | Start / stop recording |
| Ctrl+\\ | Hide / show the interface |
| Ctrl+Q | Quit |
| Ctrl+O | Open file |
| Ctrl+, | Settings |
| Ctrl+E | Extensions |
| Ctrl+B | Add bookmark |
| ? | Keyboard shortcuts |
| Alt+E | Next equalizer preset (turns it on) |
| Alt+Shift+E | Equalizer on / off |
| Alt+K | Skip intro, credits or segment (the Skip button) |
| Alt+Shift+K | Undo the last skip or mute |
| Esc | Close the top dialog or panel, then leave miniplayer / fullscreen |
| Double-click | Fullscreen |
| Click | Play / pause |
| Right-click | Context menu |
| Drop files | Open files or add them to the queue |

## Streams

**Library → Streams** (the antenna icon in the sidebar) is the home of everything you stream:

- **URL bar**: paste a link (http, rtmp, rtsp, m3u, YouTube and other yt-dlp sites) and press Enter to play it, or type words to search radio stations. The clock button lists the last 20 URLs; ⋯ opens the full *Open URL* dialog (several URLs at once, playlist name).
- **Tabs**: **Recent** (the last 50 streams, any kind), **Favorites**, **Radio** and **TV** (from the built-in Network Streams plugin; hidden when it is off) and **Music** (a switcher with Free Music and installed music services such as YouTube Music). Every card looks the same and has a ★; starring works the same in every tab and in the now-playing card.
- **Filters**: Radio by country, genre, language, minimum bitrate and top / most played / trending / new; TV by country, group and the program guide.
- **Now-playing card**: the station or title, its logo, ★, *Lyrics* (when there are lyrics), *Record* (live streams, saves with mpv's stream recorder) and *Download* (when a download plugin can save it).
- **Service pages**: a service's sidebar icon (Free Music, YouTube Music, Radio & TV, …) opens its own page in the Library with a back arrow to Streams; the hub hosts the services that have no icon of their own.
- **Home** shows a *Streams* row with your favorites and recent streams.

The palette (Ctrl+K) has *Streams: Recent / Favorites / Radio / TV / Music / open a URL* and one entry per music service. For one release the old *Open Network Stream* palette command and menu row still work and open the hub at the URL bar. Favorites and history move once from Network Streams' lists into `~/.local/share/vivi/streams/` (`favorites.json`, `history.json`, `urls.json`); the old files stay untouched.

## Plugins
Turn plugins on or off and change their settings in the **Extensions** window (`Ctrl+E`, the plugin icon at the bottom of the sidebar, or Settings → Extensions → Open Extensions): one tab per plugin with what it does, where it shows up and its actions, an ON/OFF switch and a gear for its settings, plus **All / Marketplace** to install, update and remove plugins. Source plugins (Network Streams, Free Music, YouTube) are also listed under **Sources** in the sidebar and on Home; hide or reorder them in their settings (gear). Feature plugins add entries where they belong, e.g. Subtitle Search adds *Search online…* to the Subtitles menu. The state is saved in `~/.config/vivi/config.toml` under `[plugins.<id>]`.

| Plugin | What it does |
|---|---|
| **Music Visualizer** | Visuals for music and radio (anything without a video track; cover art does not count), measured from mpv's real output after the equalizer and audio filters: Spectrometer, Scope, Spectrum, Waveform, Radial, Particles and Pitch (with the note name). Pick a mode with the top-left dropdown, the side panel (Playback → Display → Audio Visualizer), the command palette (*Next / Previous visualizer mode*) or `V` / `Shift+V`; the choice is remembered. **Colors**: *Theme accent* (default) or *Album art*, which takes the main color of the playing song's cover (local files and music-service covers), adjusted to stay readable on the light and dark themes and eased in on song changes; black-and-white covers keep the accent. Runs at ~30 fps on a small canvas scaled to the window and stops drawing ~1.5 s after a pause. AI agents and scripts: `set_visualizer` (vivi-mcp) / `{"cmd":"visualizer"}` on the control socket. |
| **Skip Intro & Credits** | One Skip button for everything worth skipping: chapters named Intro, Opening, OP, Credits, Ending or ED (markers such as *End of intro* and post-credits scenes are not skipped), your own per-show ranges (`the wand = 0:30-2:00, 21:10-end`; to mark one while watching press **Mark** at its start and again at its end) and segments from extensions such as SponsorBlock (*Skip sponsor*, *Mute self-promotion*). A segment found twice gets one button. `Alt+K` presses the button, `Alt+Shift+K` undoes the last skip (or unmutes), also from the command palette. *Skip automatically* jumps over intros and credits and leaves *Skipped intro · Undo* for a few seconds; *Per category or source* sets skip / ask / off for single kinds (`intro=skip sponsorblock:filler=off`). After you skip an intro by hand the button offers *Always skip the intro of this show*; remembered shows are listed in the settings and *Forget this show* clears them. |
| **Lyrics** | Synced lyrics as a karaoke overlay, full screen with `L` (side panel → Playback → Lyrics), or as a list in the player's side drawer: the current line large in the accent colour with a word-by-word highlight for enhanced LRC. **Click a line** of synced lyrics (karaoke view, overlay or drawer) to jump there. Sources: your manual pick (`Ctrl+L` *Find Lyrics*: edit artist/title, pick a result by duration and synced/plain), `song.lrc` next to `song.flac`, lyrics embedded in the tags (USLT/SYLT/LYRICS/©lyr), the cache, then [LRCLIB](https://lrclib.net) using the file's tags (music only; plain lyrics when nothing synced exists). Songs a **music service** extension describes (YouTube Music and others that send `vivi-media-meta`) and untagged songs the **Auto-tagger** identified are looked up the same way, so a service's song plays as normal karaoke from its start instead of the radio list. Tracks LRCLIB lists as **instrumental** say so instead of "no lyrics"; songs not found are asked again after a week. `[` / `]` shift the timing by 0.1 s (`Shift`: 0.5 s), saved per track (only for synced lyrics). *Save as .lrc* / *Embed in tags* keep the lyrics with the song (optionally automatic; not offered when the lyrics already come from that place). On radio streams it looks up the current stream title ("Artist - Title", ads and station IDs skipped, results cached). Where in the song the stream is cannot be known (you may tune in mid-song, stations update the title late), so lyrics start as a scrollable list without highlight. With synced lyrics the list says *click the line being sung to sync*: click that line (or Tab to it and press `Enter`) and its timestamp becomes "now"; from then on the lyrics highlight and scroll like any karaoke (word wipe, long lines shrunk to fit, header and hints auto-hiding). The usual offset control and `[` / `]` fine-tune the sync (not saved; disabled until synced), clicking another line re-syncs, and a new stream title (next song) goes back to the list. Plain lyrics stay a list. Settings: online lookup, keep downloaded lyrics, radio lyrics mode (*Click a line to sync*, *Plain lyrics*, *Off*), text size and clear cache. |
| **Subtitle Search** | Side panel → Subtitles → *Find online…* (or the Subtitle menu) opens the Add Subtitles dialog. It searches [OpenSubtitles](https://www.opensubtitles.com) by the file's **hash first** (✓ *Exact match*: made for this very release, no syncing needed), then by name, an edited query or an **IMDb id** (`tt0133093` or an imdb.com link). A name with an episode (`Show S01E05`, `Show 1x05`) or a year (`Movie (2020)`) is searched as that title and season / episode or year, and as plain text again when that finds nothing. Pick languages with the chips (several at once, remembered). Results show downloads, rating, FPS and *HI* (hearing impaired), *Trusted* and *Machine* (translated) badges; the side pane has the details and *Preview text*, and *Load* (or double-click) loads it. The pick is remembered per file and loaded again next time; *Save next to the video* keeps it as `Movie.en.srt`. *Auto-load exact matches* (dialog footer, or *Load exact matches automatically* in the plugin settings; off by default) loads the most downloaded exact match when a local video opens without subtitles in your languages: one download per new file, tried once per file and session, with a toast. The per-file subtitle offset (`Z` / `X`, side panel) is built into the player and works with this plugin off. *Auto-sync to audio* runs the Subtitle Auto-Sync plugin (alass / ffsubsync with the shared media ffmpeg). Needs a free API key from [opensubtitles.com/consumers](https://www.opensubtitles.com/consumers) in the plugin settings (or `VIVI_OPENSUBTITLES_KEY`); username and password are optional and raise the daily download limit (a login is reused for 12 hours; VIP accounts use their own API host). The key and login stay in `config.toml` (0600) and the request headers: never in a URL, the log or an error message. `VIVI_OPENSUBTITLES_API` points the plugin at another API base (https, or http on this machine only, e.g. a local mock for testing). MCP: `search_subtitles`, `load_subtitle`, `set_subtitle_offset`, `sync_subtitles`. |
| **Auto-tagger** | *Tag file…* (playing file) or *Tag folder…* (a library folder or queue folder) identifies songs by an audio fingerprint on [AcoustID](https://acoustid.org) (Chromaprint: the system `fpcalc` when installed — no extra size — otherwise the built-in port) or, without a key, by their tags / file name on [MusicBrainz](https://musicbrainz.org). Matches are enriched from **Deezer** and **iTunes** (no key), **Last.fm** (genre, API key) and **Discogs** (year, style, cover, token), merged and **ranked by confidence** (score + agreeing sources). The **preview** shows current → proposed per field with the source badge of every value; tick fields on/off or click **⇄** to switch to another source's value; pick a cover candidate (size and resolution shown, click 🔍 to enlarge); see whether **synced or plain LRCLIB lyrics** will be embedded (only when *Embed synced lyrics* is on). Fingerprint matches also propose a **file name** from your pattern (default `{artist} - {title}`) — renaming is an opt-in checkbox, never overwrites a file and replaces characters not allowed in file names. Folders: progress, confident matches only, *Apply all* / *Skip*; every write is backed up (`~/.local/share/vivi/tag-backups/`) and **Undo** restores tags, covers and names. Keys come from the plugin settings or the environment (`VIVI_ACOUSTID_KEY`, `VIVI_LASTFM_KEY`, `VIVI_DISCOGS_TOKEN`). **Identify untagged songs while playing** (Vivi 0.13, off by default): a local song without title / artist tags (and not in the library) is fingerprinted in the background, at low priority, after it has played a few seconds; when AcoustID is at least 70% sure, the player title, the now-playing card, MPRIS and extensions show the identified title, artist, album and length, and a toast offers **Review tags** (the usual review — the file is never changed on its own). Each file's AcoustID answer is **cached** by path, size and modification time (at most 300 files, `~/.local/share/vivi/tagger-cache.json`, owner-only), so identifying it again, playing it again or tagging its folder needs no new fingerprint; *Forget cached results* clears it. This replaces the AcoustID plugin, now deprecated. ![Auto-tagger review](screenshots/tagger-review.png) |
| **Free Music** | *Browse…* searches and browses Creative Commons / public-domain music from [Jamendo](https://www.jamendo.com) (free client ID from [devportal.jamendo.com](https://devportal.jamendo.com)), the [Internet Archive](https://archive.org) and the Free Music Archive (via its Internet Archive mirror; both need no key), merged in one list with source badges, covers and licence chips. Genre chips, Popular / New / Featured, mood, licence (commercial use, CC BY, BY-SA, CC0), length and sort filters, infinite scroll, album / artist pages, *Similar* (radio-like), queue, *Copy credit*, *Open on Jamendo / Internet Archive* (the track's page). Selecting a track shows its large cover, licence (what it allows), attribution and source page. Playing tracks show the music now-playing card (title, artist, cover, and the source and licence on its info line) and reach MPRIS like tagged songs. *Download* saves to `~/Music/Free Music/<Artist>/` (configurable, or hands off to Offline Downloads) with tags, cover, the licence URL and an attribution line in the tags and in a `.licence.txt` file next to it; downloaded tracks get a check mark in the list and play (or queue) from the file. Jamendo's client ID is never stored in the stream URLs (Recent, history, playlists, MPRIS). |
| **Network Streams** | The **Radio** and **TV** tabs of the Streams hub (Library → Streams; turning the plugin off hides them). Radio filters: country, genre, language, minimum bitrate and top / most played / trending / new; TV: country, group and the program guide on or off. Its sidebar icon opens the full browser as a Library page (*Radio & TV*): Internet radio from [radio-browser.info](https://www.radio-browser.info) (search by name, top voted, by country or country code, by genre; no key), **M3U / IPTV** import from a file path or URL (`tvg-logo`, `group-title`; a plain word searches iptv-org channels), **TV** by country or category from [iptv-org](https://github.com/iptv-org/iptv) (cached playlists, filter as you type, channel logos), **Music** genres (popular, trending, top stations per tag) and [SomaFM](https://somafm.com) channels, **Favorites** (pinned on top with a ★) and **Recent**, **My playlists** (your M3U URLs or files, refreshed every 24 h / 6 h / 7 days, keeping the last good copy), an XMLTV program guide (now / next), shown as a list or as a grid of logos (header toggle, remembered; arrow keys, Home / End and Enter work in both). *Play* plays the selection (also *Add to queue* and *Mini player*), *Favorite* / *Remove* manage favorites. *Stream info* shows the ICY now-playing title, codec, bitrate and buffer; the side drawer panel shows the same for the stream playing, with the number of favorites and recent streams. **Search all** looks through favorites, iptv-org TV, your playlists, SomaFM and radio-browser at once (also MCP `search_streams`, `kind` radio / tv). radio-browser mirrors are discovered from `all.api.radio-browser.info` (shuffled, the next one tried when one is down) and plays are counted by station id. A path to a playlist file that does not exist shows an error instead of a channel search. Lists are kept in `~/.local/share/vivi/streams/`; logos, guides and playlist copies are cached under stable names in `~/.cache/vivi/`. |

### Downloadable plugins
Settings → Extensions has two tabs. **Installed** lists built-in plugins, downloaded plugins (with Update, Roll back and Uninstall) and mpv scripts. **Browse** shows the catalog from <https://plugins.viviplayer.app/index.json> (falls back to <https://loafer19.github.io/vivi-plugins/index.json>) with Install / Update / Uninstall buttons, version, author, size and the permissions each plugin asks for (you confirm them before installing). Downloads run in the background (the size shown includes a plugin's native helper for your platform), are checked against the sha256 in the catalog and are installed atomically into `~/.local/share/vivi/plugins/<id>/<version>/`; the previous version is kept for rollback. Vivi checks for updates at most once a day on startup (can be turned off) or when you press *Check now*, and shows a notification when updates are available.

| Package | What it does |
| --- | --- |
| **Discord Rich Presence** (`discord`) | Off by default. Shows title, artist and progress (“Listening to …” / “Watching …”) in Discord. Works right after install: Vivi ships with its own Discord application (“Listening to Vivi”, with the Vivi logo; a song's cover or a YouTube thumbnail becomes the picture and the logo its small badge), so no Discord app needs to be registered. A *Custom Discord application ID* from [discord.com/developers/applications](https://discord.com/developers/applications) is optional (your own name and art assets); *Hide titles and file names* shows only “Watching a video” / “Listening to music”. Ships the native helper `vivi-discord` (Linux x86_64/aarch64, Windows x64, macOS), which talks to Discord's IPC socket (`$XDG_RUNTIME_DIR/discord-ipc-0`, Flatpak/Snap paths, Windows named pipe) and reconnects when Discord starts later. |
| **Remote Control** (`remote-control`) | Off by default. A phone-friendly page at `http://<this computer's LAN IP>:8765` (port setting) with play/pause, ±10 s, seek, volume, previous/next, the queue (tap to play) and cover art, served by the native helper `vivi-remote`. Only loopback, private and link-local addresses may connect; every API call needs the PIN (random 6 digits, saved, unless you set one; *Show* gives URL and PIN); 5 wrong PINs block an address for a minute. |
| **YouTube & yt-dlp** (`youtube`) | Version 1.4.0, needs Vivi 0.13 or newer. **Search**: *YouTube* in the sidebar, *Search YouTube* in the command palette or *Search…* in its settings opens Vivi's own dialog: type words to search YouTube or paste a video, playlist or channel link to list it (list or grid view, with thumbnail, channel, views and duration); *Play*, *Queue* / *Queue all*, *Download* / *Download audio*. Selecting a video shows a **details pane** with its channel, length, views, upload date and the **estimated size of each quality** and of audio only (yt-dlp `filesize` / `filesize_approx` of the formats yt-dlp picks); the size of what *Download* will fetch is marked. A *Download quality* dropdown above the results sets the quality, and the button says which one it takes (e.g. *Download 1080p*). A pasted URL (settings, or copy it and press `Alt+U`) can also be added to the playlist or measured with *Show sizes*; *Download this video* (player menu → Plugins, palette) takes only the playing video. Downloads save video (quality choice) or audio only (original, mp3, m4a or opus) into a folder you pick (*Choose…*; default `~/Downloads/Vivi`). They run one at a time, each a row in the **Downloads panel** with thumbnail, progress, speed and time left, Cancel / Retry, and Play / Show in folder when done; **Cancel** really stops yt-dlp (the panel's button, or *Cancel* in the settings for the running and queued downloads). **yt-dlp**: a system yt-dlp released in the last 90 days if there is one (PATH and common folders), otherwise its own managed copy from the [official releases](https://github.com/yt-dlp/yt-dlp/releases) (checked against `SHA2-256SUMS`, 18–40 MB, size announced first) in `~/.local/share/vivi/plugin-data/youtube/bin`, updated daily (`yt-dlp -U`; *yt-dlp to use*: auto / system / managed). The chosen yt-dlp is also used for streaming URLs, and Vivi finds the managed copy on its own (this is how the Flatpak gets yt-dlp). **ffmpeg** is needed to merge separate video and audio streams and to convert audio: a system ffmpeg is used when there is one, otherwise Vivi's own verified minimal static build (about 6 MB, release `helper-ffmpeg-v8.0-1`) is fetched **on demand**, only when a download needs it: 1080p and above, any quality (720p and lower too) when YouTube offers no single file with both video and audio, or mp3/m4a/opus conversion. It is kept up to date. |
| **A-B Loop & Frame Step** (`ab-loop`, deprecated) | Built into Vivi since 0.13.0 with the same keys (see the shortcut table); the core takes `Alt+A` / `Alt+B` / `Alt+C` / `Alt+←/→` and `,` / `.` before the script sees them. Uninstall it in Extensions. |
| **Night Mode** (`night-mode`) | Dynamic range compression (mpv `dynaudnorm`, or EBU R128 `loudnorm`) so dialogue stays audible at low volume. Strength setting, `Alt+N` toggles. |
| **ListenBrainz Scrobbler** (`scrobbler`) | Sends *now playing* and listens to [ListenBrainz](https://listenbrainz.org) after half the track or 4 minutes (tracks under 30 s skipped). Offline listens are queued and sent later. Needs your user token from [listenbrainz.org/settings](https://listenbrainz.org/settings/) and `curl`. |
| **Subtitle Auto-Sync** (`subtitle-sync`) | `Alt+Y` aligns the selected external subtitle to the audio with [alass](https://github.com/kaegi/alass) or [ffsubsync](https://github.com/smacke/ffsubsync) (install one: `cargo install alass-cli` or `pip install ffsubsync`) and loads `<name>.synced.<ext>`. The manual offset keys `Alt+Z` / `Alt+X` are the player's own (saved per file); offsets the plugin kept itself on older Vivi are imported once. |
| **SponsorBlock** (`sponsorblock`) | Skips [SponsorBlock](https://sponsor.ajay.app) segments in YouTube streams (auto or `Alt+K`), categories configurable, segments shown as chapters. No key; needs `curl` and yt-dlp for YouTube playback. |
| **Scene Chapters** (`scene-chapters`, deprecated) | Scene detection is now part of Chapters & Previews (`chapter-thumbnails` 1.2): `Alt+J` (or automatic) adds scene-change chapters with ffmpeg to videos without chapters. |
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
