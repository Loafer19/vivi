#!/usr/bin/env python3
"""Package downloadable plugins and regenerate their plugins/index.json entries.

Every plugins/<id>/ directory with a manifest.toml is one package. It is zipped
deterministically (sorted names, fixed timestamps, stored = no compression) into
dist/plugins/<id>-<version>.zip, so the same sources always give the same sha256 here
and in CI. Its index entry gets the download URL of the GitHub release asset
(tag plugin-<id>-v<version>), the sha256 and the size. Built-in entries are kept.

    scripts/package-plugins.py            # build zips, rewrite plugins/index.json
    scripts/package-plugins.py --check    # build zips, fail if index.json is stale
    scripts/package-plugins.py --list     # print "<id> <version> <tag> <zip>" per package

Packages with native helpers (`binaries = ["vivi-discord"]` in manifest.toml) get one
release asset per helper and platform, `<name>-<platform>[.exe]`. The helpers are built by
CI into dist/helpers/<platform>/<name>[.exe]; when those files exist their sha256/size
go into the index, otherwise the entries already in index.json for the same version are
kept (so --check works without the binaries).

    scripts/package-plugins.py --assets <id>  # print the helper files to upload for <id>

Marketplace metadata lives next to the manifest in plugins/<id>/store.toml (categories,
tags, screenshots, changelog). It is not part of the zip, so listings can change without a
new plugin version; categories, tags and screenshots are copied into the index entry.

Standard library only (Python 3.11+ for tomllib).
"""
import hashlib
import json
import re
import sys
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPO = "Loafer19/vivi"
DIST = ROOT / "dist/plugins"
INDEX = ROOT / "plugins/index.json"
SEMVER = re.compile(r"^\d+\.\d+\.\d+$")
ID = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
PERMISSIONS = {"network", "filesystem", "subprocess"}
CATEGORIES = {"Music", "Video", "Subtitles", "Streaming", "Integrations", "Tools"}
NOT_PACKAGED = {"store.toml"}
HELPERS = ROOT / "dist/helpers"
PLATFORMS = ["linux-x86_64", "linux-aarch64", "windows-x86_64", "macos-x86_64", "macos-aarch64"]
RESERVED = {"pkgdir"}
REQUIRED = ["id", "name", "version", "description", "author", "kind", "entry", "min_vivi"]


def manifests() -> list[tuple[Path, dict]]:
    out = []
    for f in sorted(ROOT.glob("plugins/*/manifest.toml")):
        m = tomllib.loads(f.read_text())
        d = f.parent
        for k in REQUIRED:
            if not m.get(k):
                sys.exit(f"{f}: missing {k}")
        if m["id"] != d.name or not ID.match(m["id"]):
            sys.exit(f"{f}: id must match the directory name and be lowercase-with-dashes")
        if not SEMVER.match(m["version"]) or not SEMVER.match(m["min_vivi"]):
            sys.exit(f"{f}: version and min_vivi must be MAJOR.MINOR.PATCH")
        if m["kind"] not in ("mpv-lua", "mpv-js"):
            sys.exit(f"{f}: kind must be mpv-lua or mpv-js")
        if not (d / m["entry"]).is_file():
            sys.exit(f"{f}: entry {m['entry']} not found")
        for b in m.get("binaries", []):
            if not re.match(r"^[A-Za-z0-9_-]+$", b):
                sys.exit(f"{f}: invalid helper name {b!r}")
        if RESERVED & {x["key"] for x in m.get("settings", [])}:
            sys.exit(f"{f}: setting keys {sorted(RESERVED)} are reserved")
        bad = set(m.get("permissions", [])) - PERMISSIONS
        if bad:
            sys.exit(f"{f}: unknown permissions {sorted(bad)}")
        m["_store"] = store(d)
        out.append((d, m))
    return out


def store(d: Path) -> dict:
    f = d / "store.toml"
    s = tomllib.loads(f.read_text()) if f.exists() else {}
    bad = set(s.get("categories", [])) - CATEGORIES
    if bad:
        sys.exit(f"{f}: unknown categories {sorted(bad)} (use {sorted(CATEGORIES)})")
    return s


def build_zip(d: Path, m: dict) -> Path:
    DIST.mkdir(parents=True, exist_ok=True)
    z = DIST / f"{m['id']}-{m['version']}.zip"
    files = sorted(p for p in d.rglob("*") if p.is_file() and not p.name.startswith(".") and p.name not in NOT_PACKAGED)
    with zipfile.ZipFile(z, "w", zipfile.ZIP_STORED) as zf:
        for p in files:
            info = zipfile.ZipInfo(p.relative_to(d).as_posix(), date_time=(1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o644 << 16
            info.create_system = 3
            zf.writestr(info, p.read_bytes())
    return z


def helper_files(m: dict) -> list[tuple[str, str, Path]]:
    """(name, platform, local file) of every helper CI built for this package."""
    out = []
    for name in m.get("binaries", []):
        for plat in PLATFORMS:
            exe = name + (".exe" if plat.startswith("windows") else "")
            f = HELPERS / plat / exe
            if f.is_file():
                out.append((name, plat, f))
    return out


def binaries(m: dict, tag: str, old: dict | None) -> list[dict]:
    if not m.get("binaries"):
        return []
    files = helper_files(m)
    if not files:
        return old.get("binaries", []) if old and old.get("version") == m["version"] else []
    out = []
    for name, plat, f in files:
        data = f.read_bytes()
        asset = f"{name}-{plat}" + (".exe" if plat.startswith("windows") else "")
        out.append({
            "name": name,
            "platform": plat,
            "url": f"https://github.com/{REPO}/releases/download/{tag}/{asset}",
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        })
    return out


def entry(m: dict, z: Path, old: dict | None = None) -> dict:
    data = z.read_bytes()
    tag = f"plugin-{m['id']}-v{m['version']}"
    e = {
        "id": m["id"],
        "name": m["name"],
        "description": m["description"],
        "version": m["version"],
        "author": m["author"],
        "icon": m.get("icon", "plugins"),
        "lucide": m.get("lucide", "puzzle"),
        "kind": "package",
        "runtime": m["kind"],
        "default_enabled": m.get("default_enabled", True),
        "min_vivi": m["min_vivi"],
        "permissions": m.get("permissions", []),
        "source": f"plugins/{m['id']}",
        "url": f"https://github.com/{REPO}/releases/download/{tag}/{z.name}",
        "sha256": hashlib.sha256(data).hexdigest(),
        "size": len(data),
        "settings": [
            {k: s[k] for k in ("key", "title", "type", "options", "default", "label") if k in s}
            for s in m.get("settings", [])
        ],
    }
    if "issue" in m:
        e["issue"] = m["issue"]
    for k in ("categories", "tags", "screenshots"):
        if m["_store"].get(k):
            e[k] = m["_store"][k]
    if m["_store"].get("changelog"):
        e["changelog"] = [
            {
                "version": c["version"],
                "date": c.get("date", ""),
                "notes": c.get("notes", ""),
                "url": f"https://github.com/{REPO}/releases/download/plugin-{m['id']}-v{c['version']}/{m['id']}-{c['version']}.zip",
            }
            for c in m["_store"]["changelog"]
        ]
    if m.get("binaries"):
        e["needs"] = m["binaries"]
        e["binaries"] = binaries(m, tag, old)
    if m.get("size_note"):
        e["size_note"] = m["size_note"]
    return e


def main() -> None:
    pk = [(d, m, build_zip(d, m)) for d, m in manifests()]
    if "--assets" in sys.argv:
        want = sys.argv[sys.argv.index("--assets") + 1]
        for _, m, _ in pk:
            if m["id"] == want:
                for name, plat, f in helper_files(m):
                    ext = ".exe" if plat.startswith("windows") else ""
                    dst = DIST / f"{name}-{plat}{ext}"
                    dst.write_bytes(f.read_bytes())
                    print(dst)
        return
    if "--list" in sys.argv:
        for _, m, z in pk:
            print(m["id"], m["version"], f"plugin-{m['id']}-v{m['version']}", z)
        return
    idx = json.loads(INDEX.read_text())
    old = {p["id"]: p for p in idx["plugins"] if p.get("kind") == "package"}
    keep = [p for p in idx["plugins"] if p.get("kind") != "package"]
    idx["plugins"] = keep + [entry(m, z, old.get(m["id"])) for _, m, z in pk]
    text = json.dumps(idx, indent=2, ensure_ascii=False) + "\n"
    if "--check" in sys.argv:
        if INDEX.read_text() != text:
            sys.exit("plugins/index.json is stale: run scripts/package-plugins.py and commit it")
        print("plugins/index.json is up to date")
        return
    INDEX.write_text(text)
    for _, m, z in pk:
        print(f"{z.relative_to(ROOT)}  {entry(m, z)['sha256']}")


if __name__ == "__main__":
    main()
