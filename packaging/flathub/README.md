# Flathub submission

Files for the `io.github.loafer19.vivi` repo on Flathub:

- `io.github.loafer19.vivi.yml`: the manifest. Same modules as `packaging/flatpak/` but
  Vivi is built from the git tag (`tag` + `commit`), and every dependency is pinned to a
  commit. Builds offline from `cargo-sources.json`.
- `cargo-sources.json`: the cargo crates for that tag (flatpak-cargo-generator output,
  copied from `packaging/flatpak/` at the tag).
- `io.github.loafer19.vivi.metainfo.xml`: passes `appstreamcli validate --pedantic`;
  six 1280×720 screenshots pinned to the tag, release notes, branding colors.

CI (`.github/workflows/packaging.yml`) runs `flatpak-builder-lint manifest` and
`appstreamcli validate` on these. The release workflow (job `packaging`) moves the tag,
commit, cargo sources, screenshot URLs and the new `<release>` here on every release.

## Submit (once)

1. Read <https://docs.flathub.org/docs/for-app-authors/submission> and the requirements
   (<https://docs.flathub.org/docs/for-app-authors/requirements>). The app id
   `io.github.loafer19.vivi` requires the GitHub account `Loafer19` (verified by Flathub
   through GitHub login).
2. Test locally:
   ```sh
   flatpak install -y flathub org.flatpak.Builder
   cd packaging/flathub
   flatpak run org.flatpak.Builder --force-clean --sandbox --user --install \
     --install-deps-from=flathub --ccache --mirror-screenshots-url=https://dl.flathub.org/media/ \
     --repo=repo builddir io.github.loafer19.vivi.yml
   flatpak run io.github.loafer19.vivi
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest io.github.loafer19.vivi.yml
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo repo
   ```
3. Fork <https://github.com/flathub/flathub> (uncheck "copy the master branch only"),
   then:
   ```sh
   git clone --branch=new-pr git@github.com:<you>/flathub.git && cd flathub
   git checkout -b vivi
   cp <vivi>/packaging/flathub/{io.github.loafer19.vivi.yml,cargo-sources.json,io.github.loafer19.vivi.metainfo.xml} .
   git add . && git commit -m "Add io.github.loafer19.vivi" && git push -u origin vivi
   ```
   Open a pull request against the `new-pr` branch with the title
   "Add io.github.loafer19.vivi". Reviewers may ask about the `finish-args`
   (Discord IPC paths for the Discord plugin, removable media read-only, xdg folders).
4. After merge you get the `flathub/io.github.loafer19.vivi` repo; future releases: copy
   the three files from the bot's packaging commit there and push (or open a PR).

The Flathub build has no ffmpeg program and no host yt-dlp; the YouTube plugin downloads
its own (see [plugins/youtube/README.md](https://github.com/Loafer19/vivi-plugins/blob/master/plugins/youtube/README.md)).

## Winget and Scoop

`packaging/winget/` (manifest 1.10, portable zip) and `packaging/scoop/vivi.json` are kept
at the latest release by the same job. To publish: `wingetcreate submit packaging/winget`
(or a PR to microsoft/winget-pkgs under `manifests/l/Loafer19/Vivi/<ver>/`), and for Scoop
put `vivi.json` into your own bucket repo (`scoop bucket add vivi <repo-url>`) or a PR to
ScoopInstaller/Extras.
