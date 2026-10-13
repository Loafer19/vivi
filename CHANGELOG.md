# Changelog

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
