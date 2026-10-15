# Subtitle Auto-Sync

Aligns the selected **external** subtitle to the audio track with [alass](https://github.com/kaegi/alass) or [ffsubsync](https://github.com/smacke/ffsubsync), whichever is installed (or the one picked in settings). Implements #56.

- `Alt+Y` or Settings → Extensions → *Sync current subtitle*: runs in the background (a toast says when it starts and finishes) and loads `<name>.synced.<ext>`, saved next to the original.
- *Check installed tools* reports which of `alass` / `alass-cli` / `ffsubsync` were found in `PATH`.
- If neither is installed you get a message with install hints: `cargo install alass-cli` or `pip install ffsubsync` (ffsubsync also needs `ffmpeg`).

Embedded subtitle tracks are not supported (extract them first). Binding name: `script-binding subtitle_sync/sync`.
