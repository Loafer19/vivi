# Discord Rich Presence

Shows what Vivi plays in your Discord status: title and artist (or only "Watching a video" / "Listening to music" with *Hide titles and file names*), pause state and progress. Implements #57; was built into Vivi up to 0.9.0.

Setup: create an application at <https://discord.com/developers/applications>, paste its **Application ID** in the plugin settings (its name is what Discord shows, e.g. "Listening to Vivi"). *Large image key* optionally names an art asset uploaded to that application. *Connection status → Show* tells whether the helper reached Discord.

How it works: `discord.lua` turns on mpv's JSON IPC (`input-ipc-server`) and starts the native helper `bin/vivi-discord` (sources: `helpers/src/bin/vivi-discord.rs`). The helper polls mpv once a second over IPC and sends `SET_ACTIVITY` frames to Discord's local IPC socket (`$XDG_RUNTIME_DIR/discord-ipc-N`, Flatpak/Snap Discord paths, `\\.\pipe\discord-ipc-N` on Windows), retrying every 15 s while Discord is closed. Flatpak Vivi has access to `xdg-run/discord-ipc-0` and `xdg-run/app/com.discordapp.Discord`.
