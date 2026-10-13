#!/usr/bin/env bash
# Generate a small media tree for testing the library (needs ffmpeg).
# Usage: scripts/sample-media.sh [dir]   (default: /tmp/vivi-media)
set -euo pipefail
D=${1:-/tmp/vivi-media}
mkdir -p "$D/Videos/The Wand/Season 1" "$D/Videos/Movies" "$D/Videos/Clips" \
         "$D/Music/Aurora Lane/Night Drive" "$D/Music/Mono Field/Low Tide"
f() { ffmpeg -v error -y "$@"; }
vid() { f -f lavfi -i "$1=size=640x360:rate=25" -f lavfi -i "sine=frequency=330:duration=$3" -t "$3" -c:v libx264 -preset ultrafast -c:a aac "$2"; }
for i in 1 2 3 4; do vid testsrc2 "$D/Videos/The Wand/Season 1/The Wand S01E0$i.mp4" 8; done
vid smptebars "$D/Videos/Movies/Big Color Bars (2024).mp4" 20
vid mandelbrot "$D/Videos/Movies/Fractal Dreams.mp4" 12
vid rgbtestsrc "$D/Videos/Clips/rgb test.mkv" 6
f -f lavfi -i "gradients=size=300x300:duration=1" -frames:v 1 "$D/cover.png"
n=0
for t in Neon Highway Afterglow; do n=$((n+1))
  f -f lavfi -i "sine=frequency=$((400+n*40)):duration=12" -i "$D/cover.png" -map 0 -map 1 -c:a libmp3lame -c:v mjpeg \
    -id3v2_version 3 -disposition:v attached_pic -metadata title=$t -metadata artist="Aurora Lane" \
    -metadata album="Night Drive" -metadata genre=Synthwave -metadata track=$n "$D/Music/Aurora Lane/Night Drive/0$n $t.mp3"
done
f -f lavfi -i "sine=frequency=220:duration=12" -metadata title=Undertow -metadata artist="Mono Field" -metadata album="Low Tide" \
  -metadata genre=Ambient -metadata track=1 "$D/Music/Mono Field/Low Tide/01 Undertow.flac"
f -f lavfi -i "sine=frequency=247:duration=12" -metadata title=Shoreline -metadata artist="Mono Field" -metadata album="Low Tide" \
  -metadata genre=Ambient -metadata track=2 "$D/Music/Mono Field/Low Tide/02 Shoreline.ogg"
rm "$D/cover.png"
echo "Sample media in $D — add $D/Videos and $D/Music in Settings → Library."
