# Figma: «VLC Redesign (Community)» — розбір

Джерело: `docs/design/vlc-redesign.fig` (локальна копія, не в git). Формат: zip → `canvas.fig`
(fig-kiwi, schema + data, deflate), `meta.json`, `thumbnail.png`, `images/` (42 растрові картинки).
Розпарсено власним скриптом на `kiwi-schema` + `pako` (11 613 вузлів: 4570 FRAME, 2539 INSTANCE,
1945 TEXT, 1121 VECTOR, 332 SYMBOL). Збережено: `figma/thumbnail.png`, `figma/images/*`,
`figma/tokens.txt`.

**Рендер фреймів у PNG не виконано**: відкритого рендерера .fig немає (потрібно відкрити файл у Figma
й експортувати; або Figma REST API з токеном). Є лише `thumbnail.png` (400×134).

## Сторінки та фрейми (Page 1)
- **Player** 1920×1080 (×7 варіантів), **Player - Light**, **Player - hide UI**, **Player - Music** (×2)
- **Player - customize** (×9) — drag&drop редактор панелі; **Profiles** 651×633
- **Controls** 1960×11139 — каталог контролів; **volume**, **play**, **Button**, **Toggle**, **Values**, **Current Time**, **Duration**
- **sidebar** 4492×1028 (вкладки бічної панелі), **Miniplayer** 431×2816
- Панелі 400 px: Audio – Equalizer / Compressor / Spatializer / Stereo Widener; Video – Essential / Crop / Colors
- Дропдауни 406 px: EQ presets, Audio Visualizer, HDR Options, Setting Preset, sub filename
- **Settings** 1048×865: global, interface, privacy, playback, video, audio, subtitles, controls, library, network, performance, recording, extensions
- **Keyboard Shortcuts** 1048×800, **Add Subtitles to Playlist/Folder** (×4), **Open network stream**, **new playlist**
- **Onboarding** 1090×759 (×11 кроків)
- Меню: **menu**, **right-click menu**, context / file / folder / playlist context menu, **Sorting Options**, **View Options**
- Картки: **media**, **playlist**, **thumbnail**, **setting category**
- Компоненти (Internal canvas): ~60 іконок `name=… (play, pause, next, previous, ±10/15 s, subtitles, audio, playlist, settings, fullscreen, sidebar, home, folder, streaming, songs, videos, album, volume, chevron/arrow ×4, vlc)` з варіантами `state=default/active`, `two-tone=yes/no`.

## Дизайн-токени
### Кольори (Dark — основна тема)
| Токен | Hex |
|---|---|
| Accent/Main, Accent/Logo | `#D37918` |
| Accent/Bright | `#FC9729` |
| Accent/Dim | `#AB6314` (alt `#955F24`) |
| BG/Accent Bright / Dark | `#371F06` / `#1A1611` |
| BG/A1…A5 | `#222222` `#1F1F1F` `#1B1B1B` `#191919` `#151515` |
| BG/B1, B2 | `#0E0E0E`, `#090909` |
| BG/A3·B1 Transparent | `#191919`/`#0E0E0E` @95%; B2 Transparent `#000000` @80% |
| FG/1, FG/2 | `#EAEAEA`, `#C6C6C6` |
| FG/A1…A4 | `#969696` `#8F8F8F` `#6B6B6B` `#404040` |
| FG/B1 | `#161616` |
| Divider Bright / Dim | `#373737` / `#222222` |

Light: Accent `#E8851A` / Bright `#FFA13B` / Logo `#C2711A` / Dim `#965916`; BG A1–A5 `#EDEDED #DDDDDD #D5D5D5 #B3B3B3 #888888`, B1/B2 `#E7E7E7/#DEDEDE`; FG A1–A4 `#3C3C3C #2E2E2E #6B6B6B #3B3B3B`; Divider `#9B9B9B/#ACACAC`. Ефекти: `bg blur`, `bg blur + shadow`.

### Типографіка
Inter (Regular 14 — основний, 1376 використань; 12, 16; Medium 16; Semi Bold 14/16; Light 16/24/32), DM Mono 14 для таймкодів.

### Відступи / радіуси
- Gap (auto-layout): **10** (домінує), 8, 5, 16, 20, 12, 14
- Радіуси: **4** і **6** (основні), 2, 12, 14, 100 (pill/кола) — «прямокутно, але не гостро».
