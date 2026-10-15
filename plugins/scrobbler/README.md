# Scrobbler (Last.fm & ListenBrainz)

Scrobbles the music you play in Vivi to [Last.fm](https://www.last.fm) and/or
[ListenBrainz](https://listenbrainz.org). Either service, or both, can be set up.

* **Now playing** is sent when a track starts (can be turned off).
* A **scrobble** is sent once the track has played for half its length or 4 minutes,
  whichever comes first. Tracks under 30 seconds are skipped; seeking does not count
  as listening. Artist, title and album come from the file tags (a MusicBrainz
  recording id is sent too when the file has one, e.g. after the Auto-tagger).
* **Offline queue**: scrobbles that cannot be sent are kept in
  `~/.local/share/vivi/plugin-data/scrobbler/queue.jsonl` (ListenBrainz) and
  `lastfm-queue.jsonl` (Last.fm) and sent in batches (100 / 50) at start, after the next
  successful scrobble, every 10 minutes, or with *Send queued listens now*.
* Videos are skipped unless *Only music* is off.

## ListenBrainz

1. Sign in at listenbrainz.org and copy your **user token** from
   <https://listenbrainz.org/settings/>.
2. Settings → Extensions → Scrobbler → paste it into *ListenBrainz user token*.

*API server* can point to another ListenBrainz-compatible server (for example a
self-hosted Maloja at `https://maloja.example/apis/listenbrainz`).

## Last.fm

Last.fm only hands out user sessions to registered API applications, and Vivi does not
ship a shared key, so you use your own (free, takes a minute):

1. Create an API account at <https://www.last.fm/api/account/create> (any name, no
   callback URL needed) and copy the **API key** and **shared secret** into the settings.
2. Press **Connect Last.fm**: your browser opens Last.fm's authorisation page; allow
   access.
3. Press **Finish connecting**. The session key is saved in *Last.fm session key*; your
   password never passes through Vivi. Clear that field to disconnect.

Requests are signed (`api_sig`, MD5 implemented in the script) and sent to
`https://ws.audioscrobbler.com/2.0/` (`track.updateNowPlaying`, `track.scrobble`).

Needs `curl` (preinstalled on Linux, macOS and Windows 10+).
