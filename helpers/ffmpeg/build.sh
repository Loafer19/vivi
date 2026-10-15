#!/usr/bin/env bash
# Minimal static ffmpeg + ffprobe for the YouTube plugin: only what yt-dlp needs to
# merge / remux mp4, webm, mkv, m4a and to convert audio to mp3 (libmp3lame), aac,
# opus (libopus) and m4a. No network, no devices, no video codecs (streams are copied).
#
#   helpers/ffmpeg/build.sh <out-dir> [cross-prefix]
#
# cross-prefix e.g. x86_64-w64-mingw32 (Windows from Linux). Needs a C compiler, make,
# pkg-config and curl. Linux CI runs it in Alpine (musl) for fully static binaries.
set -euo pipefail

FFMPEG_VER=8.0
OPUS_VER=1.5.2
LAME_VER=3.100
SRC=(
  "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VER.tar.xz b2751fccb6cc4c77708113cd78b561059b6fa904b24162fa0be2d60273d27b8e"
  "https://downloads.xiph.org/releases/opus/opus-$OPUS_VER.tar.gz 65c1d2f78b9f2fb20082c38cbe47c951ad5839345876e46941612ee87f9a7ce1"
  "https://downloads.sourceforge.net/project/lame/lame/$LAME_VER/lame-$LAME_VER.tar.gz ddfe36cab873794038ae2c1210557ad34857a4b6bdc515785d1da9e175b1da1e"
)

OUT=$(mkdir -p "$1" && cd "$1" && pwd)
CROSS=${2:-}
WORK=${WORK:-$PWD/ffmpeg-build}
PREFIX=$WORK/prefix
JOBS=$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu)
mkdir -p "$WORK" "$PREFIX"
cd "$WORK"

sha() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
for s in "${SRC[@]}"; do
  set -- $s
  f=${1##*/}
  [ -f "$f" ] || curl -fsSL --retry 3 -o "$f" "$1"
  [ "$(sha "$f")" = "$2" ] || { echo "checksum mismatch: $f" >&2; exit 1; }
  rm -rf "${f%.tar.*}" && tar -xf "$f"
done

host=()
[ -n "$CROSS" ] && host=(--host="$CROSS")
export PKG_CONFIG_PATH=$PREFIX/lib/pkgconfig PKG_CONFIG_LIBDIR=$PREFIX/lib/pkgconfig
export CFLAGS="-Os -ffunction-sections -fdata-sections"

(cd opus-$OPUS_VER && ./configure ${host[@]+"${host[@]}"} --prefix="$PREFIX" --enable-static --disable-shared \
  --disable-doc --disable-extra-programs && make -j"$JOBS" && make install)
(cd lame-$LAME_VER && ./configure ${host[@]+"${host[@]}"} --prefix="$PREFIX" --enable-static --disable-shared \
  --disable-frontend --disable-decoder --disable-gtktest && make -j"$JOBS" && make install)

cross=()
case "$CROSS" in
  *mingw32) cross=(--enable-cross-compile --cross-prefix="$CROSS-" --pkg-config=pkg-config --target-os=mingw32 --arch=x86_64) ;;
esac
ldflags="-L$PREFIX/lib"
case "$(uname -s)" in
  Darwin) ldflags="$ldflags -Wl,-dead_strip" ;;
  *) ldflags="$ldflags -Wl,--gc-sections -static" ;;
esac

cd ffmpeg-$FFMPEG_VER
./configure ${cross[@]+"${cross[@]}"} --prefix="$PREFIX" --pkg-config-flags=--static \
  --extra-cflags="-I$PREFIX/include $CFLAGS" --extra-ldflags="$ldflags" \
  --disable-everything --disable-autodetect --disable-network --disable-doc --disable-debug \
  --disable-ffplay --disable-avdevice --disable-swscale \
  --enable-small --enable-static --disable-shared \
  --enable-libmp3lame --enable-libopus \
  --enable-protocol=file,pipe \
  --enable-demuxer=mov,matroska,webm_dash_manifest,mp3,aac,ogg,wav,flac,mpegts,hls,concat \
  --enable-muxer=mp4,mov,ipod,matroska,webm,mp3,adts,opus,ogg,wav,flac,mpegts,null \
  --enable-decoder=aac,mp3,mp3float,opus,libopus,vorbis,flac,pcm_s16le \
  --enable-encoder=libmp3lame,aac,libopus,pcm_s16le,flac \
  --enable-parser=aac,h264,hevc,vp8,vp9,av1,opus,vorbis,mpegaudio,flac \
  --enable-bsf=aac_adtstoasc,extract_extradata,vp9_superframe,h264_mp4toannexb,hevc_mp4toannexb \
  --enable-filter=aresample,anull,aformat,abuffer,abuffersink,null,buffer,buffersink,format,atrim,trim,volume,anullsrc \
  || { tail -50 ffbuild/config.log; exit 1; }
make -j"$JOBS"
exe=; [ -f ffmpeg.exe ] && exe=.exe
strip=${CROSS:+$CROSS-}strip
for p in ffmpeg ffprobe; do cp "$p$exe" "$OUT/"; "$strip" "$OUT/$p$exe" || true; done
./ffmpeg$exe -hide_banner -version 2>/dev/null | head -1 || true
ls -l "$OUT"
