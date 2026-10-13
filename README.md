# Vivi

Швидкий і легкий медіаплеєр на Rust (Slint + libmpv), натхненний редизайном VLC від
[Juxtopposed](https://www.youtube.com/watch?v=ERFeOgqk324). Ціль — менше 100 МБ, миттєвий запуск,
простий інтерфейс із глибокою кастомізацією.

![Vivi](docs/screenshots/player.png)

## Можливості (v0.1.0)
- Відтворення майже будь-яких форматів через libmpv (OpenGL render API), `hwdec=auto-safe`
- Wayland (Hyprland) та X11 — Slint winit + femtovg
- Інтерфейс за Figma «VLC Redesign»: товстий seek bar з підказкою часу, іконки з Figma, темна тема
- Автоховання панелей і курсора після 2 с бездіяльності
- Відкриття: діалог (XDG portal), аргумент командного рядка, drag&drop (X11)
- Порожній екран з «Open file» і списком нещодавніх (заглушка)

<img src="docs/screenshots/empty.png" width="480">

## Встановлення та збірка (Arch / Omarchy)
```sh
sudo pacman -S --needed mpv rust base-devel libxkbcommon wayland mesa xdg-desktop-portal
git clone https://github.com/Loafer19/vivi && cd vivi
cargo build --release
./target/release/vivi ~/Videos/film.mkv
```
Debian/Ubuntu: `sudo apt install libmpv-dev libxkbcommon-x11-0 libfontconfig1-dev`.
Готові збірки: [Releases](https://github.com/Loafer19/vivi/releases).

Налагодження: `VIVI_LOG=1` (позиція), `VIVI_MPV_DEBUG=1` (лог mpv), `WINIT_UNIX_BACKEND=x11` (XWayland).

## Гарячі клавіші
| Клавіша | Дія |
|---|---|
| Space / K | Відтворення / пауза |
| ← / → | −5 / +5 с (з Shift — 60 с) |
| ↑ / ↓ | Гучність ±5 |
| F, подвійний клік | Повний екран |
| M | Без звуку |
| Ctrl+O | Відкрити файл |
| Esc | Вийти з повного екрана |

## Архітектура
`src/player.rs` — обгортка libmpv (потік подій → канал) · `src/render.rs` — mpv під Slint ·
`ui/app.slint` — UI і дизайн-токени · `assets/icons` — SVG з Figma.
Далі: `library`, `config`, `system`. Дизайн: [docs/design/FIGMA.md](docs/design/FIGMA.md),
дослідження: [docs/RESEARCH.md](docs/RESEARCH.md).

## Roadmap
Етапи — у [Milestones](https://github.com/Loafer19/vivi/milestones), задачі — в [Issues](https://github.com/Loafer19/vivi/issues).
- [x] M1 Playback
- [x] M2 Figma UI & icons (базово)
- [ ] M3 Side panel & tracks
- [ ] M4 Queue / playlists / resume
- [ ] M5 Settings
- [ ] M6 Library
- [ ] M7 Packaging

## Ліцензія
MIT — див. [LICENSE](LICENSE). Іконки — з Figma-файлу «VLC Redesign» (Juxtopposed, Figma Community).
