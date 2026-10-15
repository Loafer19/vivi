# Remote Control

Control Vivi from your phone: open `http://<this computer's LAN IP>:8765` for play/pause, ±10 s, seek, volume, previous/next, the queue (tap to play) and cover art. Implements #58; was built into Vivi up to 0.9.0.

- **Port**: default 8765 (the helper restarts when it changes).
- **PIN**: asked once by the phone. Left empty, a random 6-digit PIN is created and saved. *Phone address and PIN → Show* displays both.
- Only loopback, private (RFC 1918 / unique-local) and link-local addresses can connect; every API call needs the PIN, and 5 wrong PINs block an address for a minute. Commands must be JSON posts (a cross-site form cannot send them).

How it works: `remote-control.lua` turns on mpv's JSON IPC (`input-ipc-server`) and starts the native helper `bin/vivi-remote` (sources: `helpers/src/remote.rs`, `helpers/src/bin/vivi-remote.rs`). The helper serves the page, reads the player state over IPC twice a second and sends the page's commands back. Embedded covers are extracted with mpv's `screenshot-to-file … video`; `cover/folder/front.jpg|png` next to the file are used otherwise.
