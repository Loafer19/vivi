#!/bin/sh
# Vivi installer: curl -fsSL https://loafer19.github.io/vivi/install.sh | sh
#
#   sh install.sh               install or update the latest release
#   sh install.sh --uninstall   remove what this script installed (settings and library stay)
#   VIVI_VERSION=0.9.1 sh install.sh   a specific release
#
# Arch Linux (and derivatives with pacman + makepkg): builds the release's vivi-bin PKGBUILD
# with `makepkg -si` (asks for your sudo password via pacman).
# Other distributions: unpacks the release tarball into ~/.local (bin/vivi, bin/vivi-mcp,
# the desktop file and icons). Every download is checked against the release SHA256SUMS.
set -eu

REPO=Loafer19/vivi
GH=https://github.com/$REPO
SITE=${VIVI_SITE:-https://loafer19.github.io/vivi}
PREFIX=${VIVI_PREFIX:-$HOME/.local}
DATA=${XDG_DATA_HOME:-$HOME/.local/share}
MANIFEST=$DATA/vivi-installer/files

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

is_arch() { have pacman && have makepkg; }

uninstall() {
  if is_arch && pacman -Q vivi-bin >/dev/null 2>&1; then
    say "Removing the vivi-bin package"
    sudo pacman -R --noconfirm vivi-bin
  fi
  if [ -f "$MANIFEST" ]; then
    say "Removing files installed under $PREFIX"
    while IFS= read -r f; do
      [ -n "$f" ] && rm -f "$f"
    done < "$MANIFEST"
    rm -rf "$DATA/vivi-installer"
    have update-desktop-database && update-desktop-database "$DATA/applications" >/dev/null 2>&1 || true
  elif ! is_arch; then
    warn "nothing to uninstall (no $MANIFEST)"
  fi
  say "Done. Settings and library (~/.config/vivi, ~/.local/share/vivi) were kept."
}

fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }

sha_of() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
  else die "need sha256sum or shasum to verify the download"
  fi
}

# verify <dir> <file name>: compare with the release SHA256SUMS in <dir>
verify() {
  want=$(awk -v n="$2" '$2 == n || $2 == "*" n { print $1 }' "$1/SHA256SUMS")
  [ -n "$want" ] || die "$2 is not listed in SHA256SUMS"
  got=$(sha_of "$1/$2")
  [ "$got" = "$want" ] || die "checksum mismatch for $2 (expected $want, got $got)"
  say "Checksum OK: $2"
}

latest_version() {
  # The app release is the repository's "latest" (plugin releases never are).
  url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$GH/releases/latest") || die "cannot reach GitHub"
  v=${url##*/tag/v}
  case $v in
    [0-9]*) printf '%s\n' "$v" ;;
    *) die "cannot find the latest release ($url)" ;;
  esac
}

install_arch() {
  say "Arch Linux: building vivi-bin $VER with makepkg"
  # The vivi-bin PKGBUILD from packaging/aur, pinned to the release tarball's sha256, which
  # must match the release SHA256SUMS (makepkg then checks the download against it).
  fetch "$SITE/vivi-bin/PKGBUILD" "$TMP/PKGBUILD"
  name=vivi-v$VER-linux-x86_64.tar.gz
  want=$(awk -v n="$name" '$2 == n || $2 == "*" n { print $1 }' "$TMP/SHA256SUMS")
  [ -n "$want" ] || die "$name is not listed in SHA256SUMS"
  pkgver=$(sed -n 's/^pkgver=//p' "$TMP/PKGBUILD")
  if [ "$pkgver" != "$VER" ]; then
    # Another release than the PKGBUILD's: its metainfo checksum is unknown here.
    sed -e "s/^pkgver=.*/pkgver=$VER/" -e "s/^sha256sums=.*/sha256sums=('SKIP')/" \
      -e "s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$want')/" "$TMP/PKGBUILD" > "$TMP/PKGBUILD.new"
    mv "$TMP/PKGBUILD.new" "$TMP/PKGBUILD"
  fi
  grep -q "^sha256sums_x86_64=('$want')" "$TMP/PKGBUILD" || die "the vivi-bin PKGBUILD does not pin the release checksum"
  say "Checksum pinned: $name"
  (cd "$TMP" && makepkg -si --noconfirm --needed)
}

check_deps() {
  bin=$1
  if have ldd; then
    missing=$(ldd "$bin" 2>/dev/null | awk '/not found/ { print $1 }')
    if [ -n "$missing" ]; then
      warn "missing libraries: $(echo "$missing" | tr '\n' ' ')"
      case $missing in *libmpv*) warn "install mpv / libmpv, e.g. 'sudo apt install libmpv2' (Debian/Ubuntu), 'sudo dnf install mpv-libs' (Fedora), 'sudo zypper install libmpv2' (openSUSE)" ;; esac
      return 1
    fi
  fi
  have ffmpeg || say "Optional: ffmpeg (library thumbnails), yt-dlp (YouTube)"
}

install_local() {
  case $(uname -m) in
    x86_64) ;;
    *) die "release builds are x86_64 only; build from source: $GH#build-from-source-arch--omarchy" ;;
  esac
  name=vivi-v$VER-linux-x86_64
  say "Installing Vivi $VER into $PREFIX"
  fetch "$BASE/$name.tar.gz" "$TMP/$name.tar.gz"
  verify "$TMP" "$name.tar.gz"
  tar -xzf "$TMP/$name.tar.gz" -C "$TMP"
  src=$TMP/$name
  bindir=$PREFIX/bin
  apps=$DATA/applications
  icons=$DATA/icons/hicolor
  mkdir -p "$bindir" "$apps" "$icons/scalable/apps" "$icons/256x256/apps" "$DATA/vivi-installer"
  : > "$MANIFEST.new"
  put() { # put <mode> <src> <dst>
    cp "$2" "$3.tmp" && chmod "$1" "$3.tmp" && mv -f "$3.tmp" "$3"
    printf '%s\n' "$3" >> "$MANIFEST.new"
  }
  put 755 "$src/vivi" "$bindir/vivi"
  put 755 "$src/vivi-mcp" "$bindir/vivi-mcp"
  sed "s|^Exec=vivi|Exec=$bindir/vivi|" "$src/vivi.desktop" > "$TMP/vivi.desktop"
  put 644 "$TMP/vivi.desktop" "$apps/vivi.desktop"
  put 644 "$src/vivi.svg" "$icons/scalable/apps/vivi.svg"
  put 644 "$src/vivi.png" "$icons/256x256/apps/vivi.png"
  mv -f "$MANIFEST.new" "$MANIFEST"
  have update-desktop-database && update-desktop-database "$apps" >/dev/null 2>&1 || true
  have gtk-update-icon-cache && gtk-update-icon-cache -q -t "$icons" >/dev/null 2>&1 || true
  check_deps "$bindir/vivi" || warn "Vivi is installed but will not start until the libraries above are installed"
  case :$PATH: in
    *:"$bindir":*) ;;
    *) warn "$bindir is not in PATH; add it, e.g. echo 'export PATH=\"$bindir:\$PATH\"' >> ~/.profile" ;;
  esac
}

case ${1:-} in
  --uninstall|uninstall) uninstall; exit 0 ;;
  -h|--help) sed -n '2,11p' "$0" 2>/dev/null || true; exit 0 ;;
  "") ;;
  *) die "unknown option: $1 (use --uninstall)" ;;
esac

[ "$(uname -s)" = Linux ] || die "this installer is for Linux; see $GH/releases for Windows and macOS"
have curl || die "curl is required"
have tar || die "tar is required"

VER=${VIVI_VERSION:-$(latest_version)}
VER=${VER#v}
BASE=$GH/releases/download/v$VER
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM
fetch "$BASE/SHA256SUMS" "$TMP/SHA256SUMS" || die "release v$VER not found"

if is_arch && [ "$(uname -m)" = x86_64 ] && [ -z "${VIVI_PREFIX:-}" ]; then
  [ "$(id -u)" -ne 0 ] || die "makepkg cannot run as root; run this as your user"
  install_arch
else
  install_local
fi
say "Vivi $VER installed. Start it from your app menu or run: vivi"
