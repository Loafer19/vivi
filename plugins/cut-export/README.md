# Cut & Export Clip

Cuts a fragment of the playing local file into a new file with ffmpeg.

1. Set **In** and **Out** (Alt+[ / Alt+], the settings buttons, or type `1:02.5`).
   Without them the A-B loop points (A-B Loop plugin, Alt+A / Alt+B) are used, else the
   whole file.
2. Pick **Stream copy** (fast, lossless, starts at the keyframe before In) or
   **Re-encode** (frame-accurate, H.264/AAC `.mp4`, or AAC `.m4a` for audio files).
3. Alt+W or **Export**. The clip is saved next to the original (or in the chosen folder)
   as `name [1m02.5s-1m10.0s].ext`.

ffmpeg: your system ffmpeg is used first. Otherwise Vivi's media ffmpeg build
(`helper-ffmpeg-v8.0-2`, about 15 MB, sha256 pinned) is downloaded once into
`plugin-data/shared/ffmpeg-media`, the same copy Chapter Thumbnails and Subtitle Auto-Sync
use. It has no video encoders, so video re-encoding falls back to stream copy (with a
notice) unless a system ffmpeg with libx264 is installed.
