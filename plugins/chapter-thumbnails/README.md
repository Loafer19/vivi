# Chapter Thumbnails

Seek-bar hover previews and chapter pictures for local videos. Implements #75 (follow-up
to #62). Needs Vivi 0.9.2 or later, which displays the images.

* **Seek bar**: 50 frames (25 / 100 selectable), 192×108, evenly spaced; hovering the seek
  bar shows the nearest one. Generated in the background with fast keyframe seeks, two
  at a time; the preview fills in as they arrive.
* **Chapters**: one 320 px picture per chapter (one second after its start) in the side
  panel's Chapters tab, also for chapters added later by Scene Chapters or SponsorBlock.
  `Alt+H` shows the chapter list (time, title, current chapter) on screen.
* **Cache**: `~/.local/share/vivi/plugin-data/chapter-thumbnails/<key>/` (`seek-NN.jpg`,
  `chapter-N.jpg`), key = hash of path, size and modification time, so an opened-again
  file shows its previews instantly. The 200 most recent files are kept.
* **ffmpeg**: a system `ffmpeg` is used when present. Otherwise (and in the Flatpak)
  Vivi's **media ffmpeg build** is downloaded once from release `helper-ffmpeg-v8.0-2`
  (Linux x86_64 / ARM64, Windows x64, macOS Apple silicon; about 15 MB for ffmpeg +
  ffprobe, checked against the sha256 pinned in the script) into
  `plugin-data/shared/ffmpeg-media`, shared with Subtitle Auto-Sync. It decodes H.264,
  HEVC, VP8/VP9, MPEG-1/2/4 and MJPEG (no AV1).

Without the plugin Vivi still makes its own in-memory seek previews when an `ffmpeg`
program is installed; the plugin adds the on-disk cache, chapter pictures and the
download for systems without ffmpeg.
