# Packaging

| Path | What |
|---|---|
| `vivi.desktop`, `vivi.svg`, `vivi.png` | Desktop entry and icons shared by all Linux packages |
| `arch/` | `vivi-bin` (prebuilt tarball) and `vivi` (from source) PKGBUILDs |
| `appimage/build-libmpv.sh` | Builds a slim libmpv into `./deps`: FFmpeg with decoders/demuxers only, mpv without CLI/JS/Blu-ray/DVD/CD/libarchive |
| `appimage/build-appimage.sh` | Bundles `target/release/vivi` + slim libmpv with linuxdeploy, drops host-provided libraries, packs with zstd -19 |
| `flatpak/` | Flatpak manifest (`io.github.loafer19.vivi`), AppStream metainfo, generated cargo sources |

## AppImage

```sh
packaging/appimage/build-libmpv.sh            # once; ~2 min on 8 cores
PKG_CONFIG_PATH=$PWD/deps/lib/pkgconfig LIBRARY_PATH=$PWD/deps/lib cargo build --release
packaging/appimage/build-appimage.sh Vivi-dev-x86_64.AppImage
```

Size history: v0.8.0 was 105.7 MB (Ubuntu's libmpv pulled in x264/x265/SVT-AV1/aom/rav1e
encoders, codec2, flite voices, ICU data, LAPACK/Fortran, librsvg, SDL2, Blu-ray/DVD, ...).
With the slim libmpv it is about 44 MB (v0.9.0: 43.8 MB). Kept: Lua (yt-dlp hook), GnuTLS (https streams),
VA-API/VDPAU, libplacebo (Vulkan/OpenGL), png/mjpeg/webp encoders for screenshots.
Host-provided: glibc, GL/EGL, X11/xcb core, Wayland, D-Bus, systemd/udev, PulseAudio/ALSA, libdrm.

## Flatpak

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --install-deps-from=flathub --force-clean --repo=repo build-dir \
  packaging/flatpak/io.github.loafer19.vivi.yml
flatpak build-bundle repo vivi.flatpak io.github.loafer19.vivi
```

After changing `Cargo.lock`, regenerate the offline cargo sources with
[flatpak-cargo-generator](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo):
`python3 flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/cargo-sources.json`.
FFmpeg comes from the freedesktop runtime (H.264/HEVC via its `codecs-extra` extension);
yt-dlp is not bundled in the sandbox.
