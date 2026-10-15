# Scene Chapters & Thumbnails

For local videos without chapters, finds scene changes with ffmpeg (`select='gt(scene,T)'` on a 160 px wide copy, no audio) in the background and adds them as chapters (`Scene 1`, `Scene 2`, …, at least *Minimum chapter length* apart). For every chapter (real or detected) it saves a 320 px JPEG thumbnail. Implements #62.

- `Alt+J` or *Detect scenes now*; or turn on *Detect scenes automatically*.
- Cache: `~/.local/share/vivi/plugin-data/scene-chapters/<key>/` with `scenes.json` and `chapter-<n>.jpg`; the key is derived from the path, size and modification time, so a changed file is analysed again. *Forget cached scenes* deletes it.
- Needs `ffmpeg` in `PATH`; without it a toast says so.

The thumbnails are written to the cache for the chapter list; showing them inside Vivi's Chapters panel needs app UI (tracked in #62). Binding names: `script-binding scene_chapters/detect`.
