#!/usr/bin/env python3
"""Bump the distribution packaging to a release (run by the release workflow, job "packaging").

    scripts/bump-packaging.py <version> <dist-dir-with-SHA256SUMS> <commit>

Updates packaging/aur/*/PKGBUILD + .SRCINFO, packaging/flathub/ (git tag/commit,
cargo-sources.json, metainfo releases and screenshot URLs), packaging/winget/ and
packaging/scoop/. The release itself is not touched; nothing is published.
"""
import datetime, hashlib, os, pathlib, re, shutil, sys

ver, dist, commit = sys.argv[1], pathlib.Path(sys.argv[2]), sys.argv[3]
root = pathlib.Path(__file__).resolve().parent.parent
P = root / "packaging"
sums = {}
for line in (dist / "SHA256SUMS").read_text().splitlines():
    h, name = line.split(maxsplit=1)
    sums[name.lstrip("*")] = h
tar = sums[f"vivi-v{ver}-linux-x86_64.tar.gz"]
win = sums[f"vivi-v{ver}-windows-x86_64.zip"]
meta_src = pathlib.Path(os.environ.get("META_SRC") or P / "flatpak/io.github.loafer19.vivi.metainfo.xml")
meta_sha = hashlib.sha256(meta_src.read_bytes()).hexdigest()


def sub(path, pairs):
    s = path.read_text()
    for pat, rep in pairs:
        s, n = re.subn(pat, rep, s, flags=re.M)
        if not n:
            sys.exit(f"{path}: no match for {pat}")
    path.write_text(s)


old = re.search(r"^pkgver=(.*)$", (P / "aur/vivi-bin/PKGBUILD").read_text(), re.M).group(1)
for f in ("PKGBUILD", ".SRCINFO"):
    p = P / "aur/vivi-bin" / f
    s = p.read_text().replace(f"v{old}", f"v{ver}").replace(f"vivi-v{old}-", f"vivi-v{ver}-")
    p.write_text(s)
pb = (P / "aur/vivi-bin/PKGBUILD").read_text()
old_meta = re.search(r"^sha256sums=\('([0-9a-f]+)'\)", pb, re.M).group(1)
old_tar = re.search(r"^sha256sums_x86_64=\('([0-9a-f]+)'\)", pb, re.M).group(1)
for f in ("PKGBUILD", ".SRCINFO"):
    p = P / "aur/vivi-bin" / f
    p.write_text(p.read_text().replace(old_meta, meta_sha).replace(old_tar, tar))
sub(P / "aur/vivi-bin/PKGBUILD", [(r"^pkgver=.*", f"pkgver={ver}"), (r"^pkgrel=.*", "pkgrel=1")])
sub(P / "aur/vivi-bin/.SRCINFO", [(r"^\tpkgver = .*", f"\tpkgver = {ver}"), (r"^\tpkgrel = .*", "\tpkgrel = 1")])
gitver = f"{ver}.r0.g{commit[:7]}"
sub(P / "aur/vivi-git/PKGBUILD", [(r"^pkgver=.*", f"pkgver={gitver}")])
sub(P / "aur/vivi-git/.SRCINFO", [(r"^\tpkgver = .*", f"\tpkgver = {gitver}")])

fh = P / "flathub"
sub(fh / "io.github.loafer19.vivi.yml", [
    (r"(url: https://github.com/Loafer19/vivi.git\n\s+tag: )v[^\n]+(\n\s+commit: )[0-9a-f]+", f"\\g<1>v{ver}\\g<2>{commit}")])
shutil.copy(os.environ.get("CARGO_SOURCES") or P / "flatpak/cargo-sources.json", fh / "cargo-sources.json")
fm = fh / "io.github.loafer19.vivi.metainfo.xml"
s = fm.read_text()
s = re.sub(r"(raw\.githubusercontent\.com/Loafer19/vivi/)[^/]+/", f"\\g<1>v{ver}/", s)
if f'<release version="{ver}"' not in s:
    m = re.search(rf'<release version="{re.escape(ver)}"[^>]*?(/>|>.*?</release>)', meta_src.read_text(), re.S)
    rel = m.group(0) if m else f'<release version="{ver}"/>'
    s = s.replace("<releases>\n", f"<releases>\n    {rel}\n", 1)
fm.write_text(s)

w = P / "winget"
for f in w.glob("*.yaml"):
    s = re.sub(r"^PackageVersion: .*", f"PackageVersion: {ver}", f.read_text(), flags=re.M)
    s = re.sub(r"v\d+\.\d+\.\d+", f"v{ver}", s)
    s = re.sub(r"^ReleaseDate: .*", f"ReleaseDate: {datetime.date.today()}", s, flags=re.M)
    s = re.sub(r"^InstallerSha256: .*", f"InstallerSha256: {win.upper()}", s, flags=re.M)
    f.write_text(s)
sc = P / "scoop/vivi.json"
s = sc.read_text()
s = re.sub(r'"version": "[^"]+"', f'"version": "{ver}"', s, count=1)
s = re.sub(r'"hash": "[0-9a-f]+"', f'"hash": "{win}"', s, count=1)
s = re.sub(r"v\d+\.\d+\.\d+", f"v{ver}", s)
sc.write_text(s)
print(f"packaging bumped to {ver} ({commit[:7]})")
