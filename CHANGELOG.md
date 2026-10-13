# Changelog

## [0.1.0] — 2026-10-13
Перша збірка, що відтворює відео.
### Added
- Відтворення через libmpv (render API, OpenGL) під Slint UI (winit + femtovg, Wayland/X11).
- Нижня панель за Figma: товстий seek bar з підказкою часу, play/prev/next, гучність, повний екран.
- 201 SVG-іконка з Figma «VLC Redesign».
- Автоховання панелей і курсора (2 с), верхня панель із назвою.
- Порожній екран (Open file, підказка drag&drop, Recent-заглушка); drag&drop на X11.
- Гарячі клавіші: Space/K, ←/→ (Shift ×60 с), ↑/↓, F, M, Ctrl+O, Esc.
- hwdec=auto-safe, fallback на null-аудіо.
