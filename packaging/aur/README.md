# AUR packages

| Package | Source | Builds |
|---|---|---|
| `vivi-bin` | release tarball `vivi-v<ver>-linux-x86_64.tar.gz` (x86_64) | nothing, installs the prebuilt `vivi` and `vivi-mcp` |
| `vivi-git` | `master` from git | `cargo build --release` (x86_64, aarch64) |

Both depend on `mpv` (for `libmpv.so`), `sqlite`, `fontconfig`, `libxkbcommon` and
`hicolor-icon-theme`, and install `/usr/bin/vivi`, `/usr/bin/vivi-mcp`, `vivi.desktop`,
the scalable and 256 px icons, the AppStream metainfo and the license. They conflict with
each other and with `vivi`.

CI (`.github/workflows/packaging.yml`) builds both in an Arch Linux container, runs
`namcap` on the PKGBUILD and the package, and checks that `.SRCINFO` matches
`makepkg --printsrcinfo`. On every release, the release workflow (job `packaging`) bumps
`pkgver` and `sha256sums` in `vivi-bin` (and the `pkgver` shown by `vivi-git`) through
`scripts/bump-packaging.py` and commits that to master.

## Publish (from your own AUR account, once)

1. Create an account on <https://aur.archlinux.org>, add your SSH public key under
   *My Account*, and add to `~/.ssh/config`:
   ```
   Host aur.archlinux.org
     User aur
     IdentityFile ~/.ssh/aur
   ```
2. For each package (an AUR repo is created by the first push):
   ```sh
   git clone ssh://aur@aur.archlinux.org/vivi-bin.git aur-vivi-bin
   cp packaging/aur/vivi-bin/PKGBUILD packaging/aur/vivi-bin/.SRCINFO aur-vivi-bin/
   cd aur-vivi-bin
   makepkg -si            # optional local test
   namcap PKGBUILD *.pkg.tar.zst
   makepkg --printsrcinfo > .SRCINFO
   git add PKGBUILD .SRCINFO && git commit -m "vivi-bin 0.9.1-1" && git push
   ```
   Same with `vivi-git`.
3. Put your name in the `# Maintainer:` line if it should not say Loafer19.

## Update after a release

The bot commit "Packaging: AUR, Flathub, winget and Scoop manifests for v<ver>" already
has the new files. Copy `PKGBUILD` and `.SRCINFO` into the AUR clone, commit, push.
`vivi-git` only needs a push when the PKGBUILD itself changes (its users rebuild to update).

`packaging/arch/` holds the in-repo PKGBUILDs (`vivi` from the source tarball, and the
`vivi-bin` attached to each GitHub release); `packaging/aur/` is what goes to the AUR.
