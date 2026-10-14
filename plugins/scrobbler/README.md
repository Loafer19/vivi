# ListenBrainz Scrobbler

Scrobbles the music you play in Vivi to [ListenBrainz](https://listenbrainz.org).

* **Now playing** is sent when a track starts (can be turned off).
* A **listen** is sent once the track has played for half its length or 4 minutes,
  whichever comes first. Tracks under 30 seconds are skipped; seeking does not count
  as listening. Artist, title and album come from the file tags (a MusicBrainz
  recording id is sent too when the file has one, e.g. after the Auto-tagger).
* **Offline queue**: listens that cannot be sent are kept in
  `~/.local/share/vivi/plugin-data/scrobbler/queue.jsonl` and sent in batches at start,
  after the next successful scrobble, every 10 minutes, or with *Send queued listens now*.
* Videos are skipped unless *Only music* is off.

## Setup

1. Sign in at listenbrainz.org and copy your **user token** from
   <https://listenbrainz.org/settings/>.
2. Settings → Extensions → ListenBrainz Scrobbler → paste it into *ListenBrainz user token*.

*API server* can point to another ListenBrainz-compatible server (for example a
self-hosted Maloja at `https://maloja.example/apis/listenbrainz`).

Needs `curl` (preinstalled on Linux, macOS and Windows 10+).

Last.fm is not supported by this package: its API needs a signed (MD5) desktop auth
flow through the browser, which an mpv script cannot do cleanly; it is planned as a
built-in plugin.
