# AUR packages

| Package | Source | Builds |
|---|---|---|
| `vivi-bin` | release tarball `vivi-v<ver>-linux-x86_64.tar.gz` (x86_64) | nothing, installs the prebuilt `vivi` and `vivi-mcp` |
| `vivi-git` | `master` from git | `cargo build --release` (x86_64, aarch64) |

Both depend on `mpv` (for `libmpv.so`; libxkbcommon and the Wayland/X11 libraries come
with it and are loaded at runtime), `fontconfig`, `libgcc`, `glibc` and
`hicolor-icon-theme`; SQLite is compiled in since 0.16.3. They install `/usr/bin/vivi`,
`/usr/bin/vivi-mcp`, `vivi.desktop`, the scalable and 256 px icons, the AppStream metainfo
and the license.

They do **not** `provide` `vivi`: the AUR already has an unrelated package called `vivi`
(a client for the Vivi wireless screen-sharing service), so claiming that name would let
its dependents resolve to this player. Both still `conflict` with `vivi` (it may install
the same `/usr/bin/vivi` / `vivi.desktop`) and with each other.

CI (`.github/workflows/packaging.yml`) builds both in an Arch Linux container, runs
`namcap` on the PKGBUILD and the package, and checks that `.SRCINFO` matches
`makepkg --printsrcinfo`. On every release, the release workflow (job `packaging`) bumps
`pkgver` and `sha256sums` in `vivi-bin` (and the `pkgver` shown by `vivi-git`) through
`scripts/bump-packaging.py` and commits that to master.

## Publish (from your own AUR account, once)

> **AUR registration is closed for now.** After the 2026 AUR malicious-packages incident
> (<https://archlinux.org/news/active-aur-malicious-packages-incident/>), aurweb 6.5.0
> (11 Aug 2026) restored pushes for existing accounts but kept new registrations closed,
> with no manual process (aur-general, 13 Aug 2026). Publishing needs an existing, verified
> AUR account; check <https://aur.archlinux.org/register> first.

1. With an AUR account, add an SSH public key under *My Account* (a dedicated key, e.g.
   `ssh-keygen -t ed25519 -f ~/.ssh/aur`) and add to `~/.ssh/config`:
   ```
   Host aur.archlinux.org
     User aur
     IdentityFile ~/.ssh/aur
   ```
2. Test and push `vivi-bin` (the first push creates the package):
   ```sh
   git clone ssh://aur@aur.archlinux.org/vivi-bin.git aur-vivi-bin
   cp packaging/aur/vivi-bin/PKGBUILD packaging/aur/vivi-bin/.SRCINFO aur-vivi-bin/
   cd aur-vivi-bin
   namcap PKGBUILD
   extra-x86_64-build                         # clean chroot (devtools); or makepkg -s
   namcap vivi-bin-*.pkg.tar.zst
   makepkg --printsrcinfo | diff - .SRCINFO   # must print nothing
   git add PKGBUILD .SRCINFO && git commit -m "vivi-bin <ver>-1" && git push origin master
   ```
   `vivi-git` the same way, later if wanted (users build it from source).
3. Put your name in the `# Maintainer:` line if it should not say Loafer19.

## Update after a release

The bot commit "Packaging: AUR, Flathub, winget and Scoop manifests for v<ver>" already
has the new files. Copy `PKGBUILD` and `.SRCINFO` into the AUR clone, commit, push.
`vivi-git` only needs a push when the PKGBUILD itself changes (its users rebuild to update).

`packaging/arch/` holds the in-repo PKGBUILDs (`vivi` from the source tarball, and the
`vivi-bin` attached to each GitHub release); `packaging/aur/` is what goes to the AUR.
