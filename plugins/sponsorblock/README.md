# SponsorBlock

Skips crowd-sourced segments ([SponsorBlock](https://sponsor.ajay.app)) in YouTube videos opened as a stream (Open Network Stream with a `youtube.com/watch?v=`, `youtu.be/`, `/shorts/`, `/live/` or `/embed/` URL; needs yt-dlp). No account or key. Implements #64.

- **Categories**: space-separated list, default `sponsor selfpromo interaction` (also `intro`, `outro`, `preview`, `music_offtopic`, `filler`).
- **When a segment starts**: skip automatically (a toast says what was skipped), or ask: a toast offers `Alt+K` to skip.
- **Mark segments as chapters**: adds chapters at segment start/end so they show on the seek bar (`Sponsor`, `End of sponsor`, …).

Privacy: lookups use the hash-prefix API (`/api/skipSegments/<first 4 hex of sha256(video id)>`), so the server does not learn which video you watch. Hashing uses `sha256sum` / `shasum` when present, else the plain `?videoID=` API. Requests go through `curl`.
