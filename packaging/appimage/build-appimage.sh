#!/usr/bin/env bash
# Assemble the AppImage from target/release/vivi and the slim libmpv built by
# build-libmpv.sh. Usage: build-appimage.sh <output.AppImage>
# Env: DEPS (prefix of build-libmpv.sh, default $PWD/deps), TOOLS (download dir).
set -euo pipefail

OUT=${1:?usage: build-appimage.sh <output.AppImage>}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DEPS=${DEPS:-$PWD/deps}
TOOLS=${TOOLS:-$PWD/.appimage-tools}
APPDIR=${APPDIR:-$PWD/AppDir}
export APPIMAGE_EXTRACT_AND_RUN=1

mkdir -p "$TOOLS"
fetch() { [ -x "$TOOLS/$1" ] || { curl -fsSLo "$TOOLS/$1" "$2"; chmod +x "$TOOLS/$1"; }; }
fetch linuxdeploy https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
fetch appimagetool https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage

rm -rf "$APPDIR"
# linuxdeploy resolves libraries (skipping the AppImage excludelist: glibc,
# GL/EGL, libX11/xcb core, fontconfig/freetype, ...) and strips everything.
LD_LIBRARY_PATH="$DEPS/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  "$TOOLS/linuxdeploy" --appdir "$APPDIR" --executable "$ROOT/target/release/vivi" \
  --desktop-file "$ROOT/packaging/vivi.desktop" --icon-file "$ROOT/packaging/vivi.svg"

# Host-provided stacks that every desktop ships and that must match the user's
# session/drivers (per the AppImage excludelist spirit): Wayland, D-Bus,
# systemd/udev, PulseAudio/ALSA client libraries, libdrm, OpenCL ICD loader.
# libva/libvdpau/libvulkan loaders stay bundled: they are small and minimal
# systems may lack them, while the actual drivers still come from the host.
for lib in libOpenCL.so.1 libwayland-client.so.0 libwayland-cursor.so.0 \
           libwayland-egl.so.1 libdbus-1.so.3 libsystemd.so.0 libudev.so.1 \
           libpulse.so.0 libpulse-simple.so.0 libasound.so.2 libdrm.so.2; do
  rm -f "$APPDIR/usr/lib/$lib"
done
# libpulse's private dependencies are not needed once it comes from the host.
rm -f "$APPDIR"/usr/lib/libpulsecommon-*.so
for lib in libsndfile.so.1 libFLAC.so.* libmp3lame.so.0 libmpg123.so.0 libasyncns.so.0; do
  rm -f "$APPDIR"/usr/lib/$lib
done
# Belt and braces: strip anything linuxdeploy left with symbols.
find "$APPDIR/usr" -type f \( -name '*.so*' -o -path '*/bin/*' \) -exec strip --strip-unneeded {} + 2>/dev/null || true

# The binary and every bundled library must still resolve (host provides the rest).
for f in "$APPDIR"/usr/bin/vivi "$APPDIR"/usr/lib/*.so*; do
  if LD_LIBRARY_PATH="$APPDIR/usr/lib" ldd "$f" | grep "not found"; then
    echo "unresolved libraries in $f" >&2; exit 1
  fi
done

du -sh "$APPDIR"
ARCH=x86_64 "$TOOLS/appimagetool" --comp zstd --mksquashfs-opt -Xcompression-level --mksquashfs-opt 19 \
  --mksquashfs-opt -b --mksquashfs-opt 1M "$APPDIR" "$OUT"
ls -la "$OUT"
