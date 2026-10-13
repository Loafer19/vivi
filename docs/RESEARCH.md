# Research (short)

## Juxtopposed's "VLC redesign" video
Key ideas: keep the orange cone and a "rectangular but not sharp" look; keep Play at the bottom but larger,
with prev/next beside it; subtitles/audio/fullscreen on the right; a thick seek bar with previews; bars
that appear on hover; separate **queue** from **playlists**; one menu instead of duplicates; a side panel
(Subtitles / Audio / Video / Playback) with per-show presets, a delay slider and a subtitle timeline;
a library (video, music, streams, playlists) plus a compact library inside the player; bookmarks and
chapters; ~12 settings categories that use words instead of numbers; drag & drop toolbar customization,
profiles and onboarding.

## Figma
See [design/FIGMA.md](design/FIGMA.md): frames, tokens, icons (`assets/icons`).

## Rust GUI / "Craft"
- **craft-gui** (crate `craft_gui`) — experimental reactive GUI; no video/SVG support — not suitable.
- **storytold Crafting Apps** (PhotoCraft, FilmCraft…) — pure Rust, egui + wgpu, shared `craft-libs`
  (`craft-ui`, `craft-settings`). FilmCraft has its own Rust decoders. ArtCraft is Tauri + React.
- **Decision:** Slint + libmpv (full format coverage, small binary). Borrow: engine/UI separation,
  performance budgets, TOML settings with atomic saves.
