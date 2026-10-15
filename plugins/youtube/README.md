# YouTube & yt-dlp

List, size and download videos and playlists from YouTube and the other sites yt-dlp supports.

- **URL**: paste it into the settings, or copy it and press `Alt+U` in the player (mpv `clipboard/text`).
- **Add to playlist**: playlist entries (`yt-dlp --flat-playlist -J`) are appended to the Vivi playlist with their titles; a single video is appended too.
- **Show sizes**: estimated size per quality and for audio only, from the formats' `filesize` / `filesize_approx` (bitrate × duration when both are missing); the same estimate is shown when a download starts. For a playlist URL the first video is measured.
- **Download**: video at the chosen quality (`bv*[height<=H]+ba/b[height<=H]`) or audio only (original, or mp3/m4a/opus via `-x`), into the folder from *Choose…* (default `~/Downloads/Vivi`), named `[NN - ]Title [id].ext`. Progress toasts every 2 s (percentage, size, ETA, entry n/N) on Linux and macOS; on Windows only start and end. *Cancel* stops it. Without `ffmpeg` only single-file formats are used and audio is not converted.

## Which yt-dlp

*yt-dlp to use*:
- **Auto** (default): a system yt-dlp released within the last 90 days (found in `PATH` and in `~/.local/bin`, `/usr/local/bin`, `/usr/bin`, Homebrew, Snap, WinGet/Scoop/Chocolatey folders; inside the Flatpak also on the host via `flatpak-spawn --host`, which only works if you grant `--talk-name=org.freedesktop.Flatpak`), otherwise the managed copy.
- **System**: always the system one (warns when it is old).
- **Managed copy**: always the plugin's own.

The managed copy is downloaded on first use from <https://github.com/yt-dlp/yt-dlp/releases/latest> (`yt-dlp_linux`, `yt-dlp_linux_aarch64`, `yt-dlp_macos`, `yt-dlp.exe`, …; 18–40 MB), checked against the release's `SHA2-256SUMS` (needs `sha256sum`, `shasum` or `certutil`) and kept in `$XDG_DATA_HOME/vivi/plugin-data/youtube/bin/`. It updates itself with `yt-dlp -U` once a day (*Update the managed yt-dlp daily*) or with *Update*; a system yt-dlp is never touched. *Which yt-dlp is used → Show* tells which one and its version.

The chosen yt-dlp is also given to mpv's ytdl_hook (`ytdl_hook-ytdl_path`), so streaming URLs use it. Vivi itself appends the managed folder to `PATH`, which is how the Flatpak (no yt-dlp of its own) streams YouTube once this plugin has downloaded yt-dlp.

Needs `curl` (and `sh` on Linux/macOS).
