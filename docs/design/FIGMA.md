# Figma: "VLC Redesign (Community)"

Source: a local copy of the `.fig` file (not committed). Format: zip → `canvas.fig` (fig-kiwi, schema +
data, deflate), `meta.json`, `thumbnail.png`, `images/`. Parsed with a small script on `kiwi-schema` +
`pako` (11,613 nodes). Icons: `assets/icons/*.svg` (201).
Frames were not rendered to PNG (no open-source .fig renderer).

## Frames (Page 1)
- **Player** 1920×1080 (dark, Light, hide UI, Music), **Player - customize**, **Profiles**
- **Controls** catalogue, **volume**, **play**, **Button**, **Toggle**, **Current Time**, **Duration**
- **sidebar**, **Miniplayer**
- 400 px panels: Audio – Equalizer / Compressor / Spatializer / Stereo Widener; Video – Essential / Crop / Colors
- Dropdowns: EQ presets, Audio Visualizer, HDR Options, Setting Preset, sub filename
- **Settings** (1048×865): global, interface, privacy, playback, video, audio, subtitles, controls, library, network, performance, recording, extensions
- **Keyboard Shortcuts**, **Add Subtitles to Playlist/Folder**, **Open network stream**, **new playlist**, **Onboarding** (11 steps)
- Menus: menu, right-click menu, context / file / folder / playlist context menu, Sorting Options, View Options

Player frame measurements: top bar 40 px `#191919` (padding 6, gap 10, title Inter 14 `#8F8F8F`);
Controls 54 px `#191919` @93% (padding 20/11, gap 20).

## Tokens (dark theme)
| Token | Hex |
|---|---|
| Accent/Main | `#D37918` |
| Accent/Bright | `#FC9729` |
| Accent/Dim | `#AB6314` |
| BG/Accent Bright / Dark | `#371F06` / `#1A1611` |
| BG/A1…A5 | `#222222` `#1F1F1F` `#1B1B1B` `#191919` `#151515` |
| BG/B1, B2 | `#0E0E0E`, `#090909` |
| FG/1, FG/2 | `#EAEAEA`, `#C6C6C6` |
| FG/A1…A4 | `#969696` `#8F8F8F` `#6B6B6B` `#404040` |
| Divider Bright / Dim | `#373737` / `#222222` |

Light theme: accent `#E8851A`, backgrounds `#EDEDED`…`#888888`, text `#3C3C3C`/`#2E2E2E`.
Type: Inter (Regular 14 primary; 12/16; Semi Bold for headings), DM Mono 14 for timecodes.
Spacing: gap 10 (also 8/5/16/20). Radii: 4 and 6 (also 2, 12, 100 for pills).
