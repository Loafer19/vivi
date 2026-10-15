# AcoustID Tagger

Identifies a song by its sound and hands the result to Vivi's Auto-tagger review.
Implements #76 (follow-up to #65). Needs Vivi 0.9.2 or later and the built-in Auto-tagger turned on.

1. `Alt+I` (or *Identify*) fingerprints the playing local file with **fpcalc**
   (Chromaprint): an installed `fpcalc` is used first (`chromaprint` /
   `libchromaprint-tools` / `brew install chromaprint`). Otherwise the official static
   Chromaprint 1.6.2 build is downloaded once from
   <https://github.com/acoustid/chromaprint/releases> and checked against the sha256
   pinned in the script: 2.4 MB (Linux x86_64), 1.4 MB (Linux ARM64), 2.7 MB (macOS
   universal), 1.8 MB (Windows x64). Stored in `~/.local/share/vivi/plugin-data/acoustid/`.
2. The fingerprint is looked up on **AcoustID** (`/v2/lookup`, `meta=recordings`).
3. The Auto-tagger looks the recordings up on **MusicBrainz** (title, artist, album,
   album artist, date, track, genre per release, cover candidates) and shows its
   candidates list and **before/after review**. A **File name** row proposes the new name
   from *New file name* (default `{artist} - {title}`; fields `{artist}`, `{title}`,
   `{album}`, `{albumartist}`, `{year}`, `{date}`, `{track}` (two digits), `{genre}`).
   Characters not allowed in file names become `_`; the extension is kept and an
   existing file is never replaced. Untick the row to keep the name. *Undo* restores the
   tags and renames the file back.

## API key

AcoustID only answers registered applications, and Vivi does not ship a key. Register a
free application at <https://acoustid.org/new-application> (name: anything, e.g. "Vivi
for me") and paste its **API key** into *AcoustID application API key*. It is only sent
to api.acoustid.org.

The built-in Auto-tagger can fingerprint on its own (fpcalc or its bundled Rust
Chromaprint) with its own key setting; this package adds the rename proposal, the
one-key identify and the managed fpcalc download.
