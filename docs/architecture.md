# Architecture overview

Vivi is one Rust binary: libmpv plays, Slint draws, and the mpv render API paints video
as an underlay beneath the Slint scene. This page maps the source tree; plugin
distribution and the package format are in [plugins-architecture.md](plugins-architecture.md).

## Process and threads

* The **UI thread** owns the Slint `AppWindow` and every `Rc` model. All Slint calls
  happen here; timers poll background work.
* The **player event thread** (`player.rs`) forwards libmpv events over a channel; the
  UI drains them in `app/wiring/events.rs`.
* **Background jobs** (network lookups, scans, downloads) run on plain threads and hand
  one result back through [`util::Task<T>`](../src/util.rs) (spawn, then `poll` from a
  timer) or, for long-lived workers, a channel of messages.

## `src/`

| Path | Role |
| --- | --- |
| `main.rs` | Module list and `main`: build the window, run startup, enter the event loop. |
| `app/` | Application wiring split out of `main`: `startup.rs` (environment, backend, per-feature setup), `session.rs` (loading media, queue, resume, recent files), `panel.rs` (side panel models), `keys.rs` (mpv key translation), `debug.rs` (screenshot / test state). |
| `app/wiring/` | One file per callback family: `events.rs` (player events, script messages), `transport.rs` (play, seek, volume ...), `keys.rs` (window key callback), `autohide.rs`, `window.rs` (drag & drop). |
| `settings/` | Settings dialog: `mod.rs` (wiring, setup), `rows.rs` (row builder, categories, search), `extensions.rs` (installed cards, catalog), `apply.rs` (theme, mpv options, presets, toolbar), `customize.rs`, `onboarding.rs`, `schema.rs` (id → getter / setter). |
| `plugins/` | Plugin host (`mod.rs`), wire formats (`protocol.rs`), packages and scripts, shared HTTP (`web.rs`), and the built-in plugins (below). |
| `viz/` | Music visualizer: audio tap, FFT analysis, modes, software canvas. |
| `config.rs`, `store.rs` | Settings file and small XDG state files. |
| `player.rs`, `render.rs` | libmpv wrapper and the OpenGL render underlay. |
| `library.rs`, `libview.rs` | Media library (SQLite cache, scans, thumbnails) and its UI controller. |
| `menus.rs`, `dialogs.rs`, `overlays.rs`, `mini.rs`, `loading.rs`, `preview.rs` | Menus, small dialogs, Esc handling, miniplayer, stream loading overlay, seekbar preview. |
| `fx.rs`, `peaks.rs`, `scenes.rs` | Equalizer / filters, seekbar waveform, package thumbnails. |
| `ipc.rs`, `mpris.rs`, `desktop.rs`, `about.rs` | Control socket (for `vivi-mcp`), MPRIS2, desktop glue, About and update check. |
| `keymap.rs` | Every shortcut, in match order. |
| `util.rs` | `clock` / `hms` formatting, Slint string models, `Task<T>`. |

## `ui/`

* `app.slint`: `AppWindow`, which only keeps window-level properties and the `key`,
  `activity` and `plugin-command` callbacks.
* `ui/state/*.slint`: one global per feature (`Playback`, `Panel`, `Library`, `Settings`,
  `Customize`, `Lyrics`, `Overlays`, ...). Rust reaches them as
  `app.global::<crate::Playback>()`; members never repeat the global's name
  (`Library.items`, not `Library.lib-items`).
* `ui/core/`: design tokens (`theme.slint`), typed enums (`types.slint`) and shared
  components (buttons, rows, dropdown, slider, modal, icons).
* Feature components: `panel.slint`, `library.slint`, `settings.slint`, `toolbar.slint`,
  `plugins.slint`, `dialogs.slint`, `filters.slint`, `menu.slint`, `viz.slint`, ...

## Accessibility and keyboard

* Slint's `accessibility` feature is on: AccessKit exposes the UI over AT-SPI (D-Bus,
  through zbus) on Linux. It needs no extra system libraries.
* Shared controls in `ui/core/` (buttons, toggle, segmented, dropdown, slider, rows) set
  `accessible-role`, `accessible-label` and their checked/value state, and take a `label`
  where the visible text is not the name.
* Every other clickable `TouchArea` names itself: `accessible-role` (button, tab,
  list-item, checkbox, slider, combobox) plus `accessible-label`; rows add
  `accessible-item-selected`, and double-click actions also get `accessible-action-default`.
  Icon-only buttons also show a `Tooltip` with the same text.
* Focusable controls wrap a `FocusScope` (Space/Enter activate) and draw the ring from
  `Theme.focus-ring` / `Metrics.focus-ring-w`. Window chrome sets `tab-focus: false`
  because its keys are shortcuts. In the main view Tab is a player key; when a dialog
  is open it moves focus instead (`dialog-open`).
* Blockers (scrims, "swallow clicks" areas) and the video surface stay unnamed.

## Screenshot hooks

Debug builds (or `VIVI_SCREENSHOT=1`) read env vars in `src/app/debug.rs` to open a UI
state directly: `VIVI_SETTINGS=<n>`, `VIVI_PANEL=<n>`, `VIVI_CUSTOMIZE=<profile>`,
`VIVI_LIB`, `VIVI_URL`, `VIVI_MUSIC`, `VIVI_STREAM_ERROR=<url>` (the failed-stream
banner with Retry / Copy), and others listed there.

## Conventions

* No colour, radius or font size literals outside `ui/core/theme.slint`.
* UI state crosses the boundary typed (`BarStyle`, `PanelTab`, `TagStatus`, ...), not as
  display strings.
* A plugin's module, struct and ID agree (`network_streams::NetworkStreams`,
  `network-streams`); IDs never change because settings are stored under them.
