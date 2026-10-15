# YouTube & yt-dlp

List, size and download videos and playlists from YouTube and the other sites yt-dlp supports.

- **URL**: paste it into the settings, or copy it and press `Alt+U` in the player (mpv `clipboard/text`).
- **Add to playlist**: playlist entries (`yt-dlp --flat-playlist -J`) are appended to the Vivi playlist with their titles; a single video is appended too.
- **Show sizes**: estimated size per quality and for audio only, from the formats' `filesize` / `filesize_approx` (bitrate × duration when both are missing); the same estimate is shown when a download starts. For a playlist URL the first video is measured.
- **Download**: video at the chosen quality (`bv*[height<=H]+ba/b[height<=H]`) or audio only (original, or mp3/m4a/opus via `-x`), into the folder from *Choose…* (default `~/Downloads/Vivi`), named `[NN - ]Title [id].ext`. Progress toasts every 2 s (percentage, size, ETA, entry n/N) on Linux and macOS; on Windows only start and end. *Cancel* stops it. Audio files get a ` (audio)` suffix.

## Which yt-dlp

*yt-dlp to use*:
- **Auto** (default): a system yt-dlp released within the last 90 days (found in `PATH` and in `~/.local/bin`, `/usr/local/bin`, `/usr/bin`, Homebrew, Snap, WinGet/Scoop/Chocolatey folders), otherwise the managed copy. The Flatpak cannot see host programs, so it always uses the managed copy.
- **System**: always the system one (warns when it is old).
- **Managed copy**: always the plugin's own.

The managed copy is downloaded on first use from <https://github.com/yt-dlp/yt-dlp/releases/latest> (`yt-dlp_linux`, `yt-dlp_linux_aarch64`, `yt-dlp_macos`, `yt-dlp.exe`, …; 18–40 MB), checked against the release's `SHA2-256SUMS` (needs `sha256sum`, `shasum` or `certutil`) and kept in `$XDG_DATA_HOME/vivi/plugin-data/youtube/bin/`. It updates itself with `yt-dlp -U` once a day (*Update the managed yt-dlp daily*) or with *Update*; a system yt-dlp is never touched. *Which yt-dlp is used → Show* tells which one and its version.

The chosen yt-dlp is also given to mpv's ytdl_hook (`ytdl_hook-ytdl_path`), so streaming URLs use it. Vivi itself appends the managed folder to `PATH`, which is how the Flatpak (no yt-dlp of its own) streams YouTube once this plugin has downloaded yt-dlp.

## ffmpeg

Merging separate best video + audio streams and converting audio need ffmpeg. A system `ffmpeg` (PATH, `/usr/bin`, `/usr/local/bin`, Homebrew, MacPorts, Snap) is used when there is one; the Flatpak has none. Otherwise a static build is downloaded **only when a download needs it**: video at 1080p or above (or *Best available*), or audio converted to mp3/m4a/opus. Below 1080p without ffmpeg, single-file formats are used.

| Platform | Source | Verified with |
| --- | --- | --- |
| Linux x86_64 / aarch64, Windows x64 / arm64 | [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases/tag/latest), newest `ffmpeg-n<ver>-latest-<platform>-gpl-<ver>` archive (125–195 MB) | the release's `checksums.sha256` (archive) |
| macOS arm64 / Intel | [osxexperts.net](https://www.osxexperts.net) `ffmpeg…arm/intel.zip` + `ffprobe…zip` | the SHA256 listed on that page (binaries) |

`ffmpeg` and `ffprobe` (about 345 MB unpacked on Linux) go to `$XDG_DATA_HOME/vivi/plugin-data/youtube/ffmpeg/` and are passed to yt-dlp with `--ffmpeg-location`. The download size is announced before it starts. Only this managed copy is updated: *Update the managed ffmpeg* re-downloads when the published build changed, and with auto-update on that is checked every 30 days. *Which yt-dlp and ffmpeg are used → Show* tells which copies are used.

Needs `curl` and `tar` (and `sh` on Linux/macOS).
