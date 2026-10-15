#!/usr/bin/env bash
# Build a slim libmpv for the AppImage: FFmpeg with decoders/demuxers only
# (no encoder libraries such as x264/x265/SVT-AV1/aom/rav1e, no flite, codec2,
# lapack, OpenCV, ...), then libmpv without the mpv CLI, JavaScript, Blu-ray,
# DVD, CD audio or libarchive. Everything lands in $PREFIX (default
# $PWD/deps); point PKG_CONFIG_PATH/LD_LIBRARY_PATH there before building vivi.
#
# Kept on purpose: Lua (mpv's ytdl_hook needs it for yt-dlp URLs), GnuTLS
# (https streams), dav1d/vpx/opus/vorbis decoders, VA-API/VDPAU/Vulkan hwdec,
# the png/mjpeg/libwebp encoders (screenshots) and all built-in lavfi filters.
set -euo pipefail

FFMPEG_VERSION=${FFMPEG_VERSION:-7.1.2}
MPV_VERSION=${MPV_VERSION:-0.40.0}
PREFIX=${PREFIX:-$PWD/deps}
SRC=${SRC:-$PWD/deps-src}
JOBS=${JOBS:-$(nproc)}

mkdir -p "$SRC" "$PREFIX"
cd "$SRC"

if [ ! -d "ffmpeg-$FFMPEG_VERSION" ]; then
  curl -fsSL "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" | tar xJ
fi
if [ ! -d "mpv-$MPV_VERSION" ]; then
  curl -fsSL "https://github.com/mpv-player/mpv/archive/refs/tags/v$MPV_VERSION.tar.gz" | tar xz
fi

# FFmpeg's Vulkan hwdec needs recent headers (Ubuntu 24.04 is too old); mpv
# still renders through Vulkan/OpenGL via libplacebo either way.
VULKAN=--disable-vulkan
pkg-config --atleast-version=1.3.277 vulkan 2>/dev/null && VULKAN=--enable-vulkan

(
  cd "ffmpeg-$FFMPEG_VERSION"
  ./configure --prefix="$PREFIX" --libdir="$PREFIX/lib" \
    --enable-shared --disable-static --disable-programs --disable-doc \
    --disable-debug --enable-stripping --enable-pic \
    --disable-encoders --enable-encoder=png,mjpeg,libwebp \
    --disable-muxers --disable-indevs --disable-outdevs \
    --disable-postproc --disable-autodetect \
    --enable-gnutls --enable-zlib --enable-bzlib --enable-lzma \
    --enable-libdav1d --enable-libvpx --enable-libopus --enable-libvorbis \
    --enable-libwebp --enable-libxml2 \
    --enable-vaapi --enable-vdpau "$VULKAN" --enable-libdrm \
    --enable-xlib
  make -j"$JOBS"
  make install
)

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
(
  cd "mpv-$MPV_VERSION"
  rm -rf build
  meson setup build --prefix="$PREFIX" --libdir=lib --buildtype=release \
    -Dstrip=true -Db_lto=true \
    -Dlibmpv=true -Dcplayer=false -Dtests=false -Dfuzzers=false \
    -Dmanpage-build=disabled -Dhtml-build=disabled -Dpdf-build=disabled \
    -Dlua=enabled -Djavascript=disabled -Dlibarchive=disabled \
    -Dlibbluray=disabled -Ddvdnav=disabled -Dcdda=disabled \
    -Dsdl2-audio=disabled -Dsdl2-video=disabled -Dsdl2-gamepad=disabled \
    -Dopenal=disabled -Dsndio=disabled -Djack=disabled -Dcaca=disabled \
    -Dsixel=disabled -Drubberband=disabled -Dzimg=disabled -Dvapoursynth=disabled \
    -Duchardet=enabled -Dpulse=enabled -Dalsa=enabled -Dpipewire=disabled \
    -Dvulkan=enabled -Dvaapi=enabled -Dvdpau=enabled
  meson compile -C build -j "$JOBS"
  meson install -C build
)
echo "libmpv installed to $PREFIX"
