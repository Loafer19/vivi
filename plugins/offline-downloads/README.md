# Offline Downloads

A download queue. Paste a URL in the settings and press **Add**, press **Alt+D** with a URL
in the clipboard (or while an online item plays), or send
`script-message-to offline_downloads enqueue <url>` from another script.

| URL | How |
| --- | --- |
| YouTube, SoundCloud, Bandcamp, Jamendo / archive.org pages, … | yt-dlp |
| Direct files (`.mp3`, `.flac`, `.ogg`, `.mp4`, …, Jamendo storage, `archive.org/download/…`) | curl, resumable, progress from Content-Length |
| Radio (Icecast/Shoutcast, `.pls` / `.m3u`) | recorded with curl for the set length |
| HLS (`.m3u8`) | recorded with ffmpeg (needs one already installed) |

One download runs at a time. **Show** (Alt+Shift+D) lists the queue with its state
(pending / active / paused / done / failed) and progress; **Pause / Resume**, **Cancel**,
**Retry**, **Clear** and **Open** act on it. The queue is saved in
`plugin-data/offline-downloads/queue.json`.

Nothing extra is downloaded: yt-dlp is your system copy or the one the YouTube & yt-dlp
plugin manages (`plugin-data/youtube/bin`); ffmpeg (for merging best video + audio) is a
system one or one another Vivi plugin already installed.
