#!/usr/bin/env python3
"""Build the docs site into _site/ (or the directory given as the first argument).

Copies docs/site/ and adds the logo and screenshots. The plugin catalog, marketplace and plugin
developer docs live on plugins.viviplayer.app (built from https://github.com/Loafer19/vivi-plugins),
not on this site.
Icons are Lucide (https://lucide.dev, ISC license) SVGs kept in docs/site/icons/; every
<i data-lucide="name"></i> placeholder is replaced by the inline SVG, so the site needs no JavaScript.
Standard library only, so it runs as-is in GitHub Actions.
"""
import html
import os
import re
import json
import shutil
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "_site"
ICON_DIR = ROOT / "docs/site/icons"
SITE = "https://viviplayer.app"
MARKET = "https://plugins.viviplayer.app"
# Per page: (description, sitemap priority). Canonical URLs always point at viviplayer.app,
# also on the GitHub Pages mirror (loafer19.github.io/vivi), so search engines index one copy.
PAGES = {
    "index.html": ("Vivi is a fast, lightweight desktop media player for Linux written in Rust on top of mpv: "
                   "plugins, music visualizer, synced lyrics, library and a clean dark UI.", "1.0"),
    "ai-agents.html": ("Control the Vivi media player from Claude, Cursor, Claude Code and other AI agents with the vivi-mcp MCP server: example prompts, setup and all 47 tools.", "0.5"),
    "compare.html": ("How the Vivi media player compares with mpv, VLC, MPC-HC, IINA, Celluloid and foobar2000: features, measured size, start time and memory, licenses and platforms.", "0.6"),
}
JSONLD = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    "name": "Vivi",
    "applicationCategory": "MultimediaApplication",
    "operatingSystem": "Linux, Windows, macOS",
    "description": PAGES["index.html"][0],
    "url": SITE + "/",
    "downloadUrl": "https://github.com/Loafer19/vivi/releases/latest",
    "installUrl": SITE + "/install.sh",
    "image": SITE + "/img/og.png",
    "screenshot": [SITE + "/img/player.png", SITE + "/img/library-home.png", SITE + "/img/visualizer.png"],
    "license": "https://opensource.org/licenses/MIT",
    "codeRepository": "https://github.com/Loafer19/vivi",
    "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"},
}


def seo(name: str, text: str) -> str:
    """Description, canonical, Open Graph / Twitter tags (and JSON-LD on the home page)."""
    desc, _ = PAGES.get(name, (None, None))
    title = re.search(r"<title>(.*?)</title>", text, re.S).group(1)
    url = SITE + ("/" if name == "index.html" else f"/{name}")
    text = re.sub(r'\s*<meta (name="description"|property="og:[a-z]+")[^>]*>', "", text)
    e = lambda v: html.escape(v, quote=True)
    tags = [f'<link rel="canonical" href="{url}">']
    if desc:
        tags.append(f'<meta name="description" content="{e(desc)}">')
    tags += [
        '<meta property="og:type" content="website">',
        '<meta property="og:site_name" content="Vivi">',
        f'<meta property="og:title" content="{e(title)}">',
        f'<meta property="og:description" content="{e(desc or title)}">',
        f'<meta property="og:url" content="{url}">',
        f'<meta property="og:image" content="{SITE}/img/og.png">',
        '<meta property="og:image:width" content="1200"><meta property="og:image:height" content="630">',
        '<meta property="og:image:alt" content="Vivi media player playing a film">',
        '<meta name="twitter:card" content="summary_large_image">',
        f'<meta name="twitter:title" content="{e(title)}">',
        f'<meta name="twitter:description" content="{e(desc or title)}">',
        f'<meta name="twitter:image" content="{SITE}/img/og.png">',
        '<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>',
    ]
    if name == "index.html":
        tags.append('<script type="application/ld+json">' + json.dumps(JSONLD) + "</script>")
    return text.replace("</title>", "</title>\n" + "\n".join(tags), 1)


def sitemap(out: Path) -> None:
    urls = "".join(
        f"<url><loc>{SITE}{'/' if n == 'index.html' else '/' + n}</loc><priority>{pr}</priority></url>"
        for n, (_, pr) in PAGES.items()
        if (out / n).exists()
    )
    (out / "sitemap.xml").write_text(
        f'<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</urlset>\n'
    )
    (out / "robots.txt").write_text(f"User-agent: *\nAllow: /\nSitemap: {SITE}/sitemap.xml\nSitemap: {MARKET}/sitemap.xml\n")


# PNGs the pages or the SEO tags use (og image, JSON-LD screenshots, full-size links).
IMAGES = ["og", "player", "library-home", "visualizer", "streams"]


def icon(name: str) -> str:
    """Inline Lucide SVG for `name` (falls back to "puzzle")."""
    f = ICON_DIR / f"{name}.svg"
    if not f.exists():
        f = ICON_DIR / "puzzle.svg"
    svg = re.sub(r"<!--.*?-->\s*", "", f.read_text(), flags=re.S).strip()
    return svg.replace('class="lucide', 'aria-hidden="true" class="lucide', 1)


def inline_icons(text: str) -> str:
    return re.sub(r'<i data-lucide="([a-z0-9-]+)"></i>', lambda m: icon(m.group(1)), text)


def release_version() -> str:
    """Latest app release: the vivi-bin pkgver, which the release workflow bumps on every release."""
    return re.search(r"^pkgver=(\S+)$", (ROOT / "packaging/aur/vivi-bin/PKGBUILD").read_text(), re.M).group(1)


REPO = "Loafer19/vivi"
# Release asset per download kind: the current name first, then the name used up to v0.16.4.
ASSETS = {
    "appimage_x64": ["vivi-{v}-linux-x64.AppImage", "Vivi-v{v}-x86_64.AppImage"],
    "appimage_arm64": ["vivi-{v}-linux-arm64.AppImage"],
    "tar_x64": ["vivi-{v}-linux-x64.tar.gz", "vivi-v{v}-linux-x86_64.tar.gz"],
    "tar_arm64": ["vivi-{v}-linux-arm64.tar.gz"],
    "flatpak": ["vivi-{v}-linux-x64.flatpak", "vivi-v{v}-x86_64.flatpak"],
    "setup_x64": ["vivi-{v}-windows-x64-setup.exe"],
    "setup_arm64": ["vivi-{v}-windows-arm64-setup.exe"],
    "portable_x64": ["vivi-{v}-windows-x64-portable.zip", "vivi-v{v}-windows-x86_64.zip"],
    "portable_arm64": ["vivi-{v}-windows-arm64-portable.zip"],
    "mac": ["vivi-{v}-macos-arm64.zip", "vivi-v{v}-macos-arm64.zip"],
}


def latest_release() -> tuple[str, set[str] | None]:
    """(version, asset names) of the latest GitHub release; names are None when offline.
    VIVI_RELEASE_ASSETS (a JSON file with {"tag_name", "assets": [{"name"}]}) replaces the
    API call for local previews."""
    data = None
    if os.environ.get("VIVI_RELEASE_ASSETS"):
        data = json.loads(Path(os.environ["VIVI_RELEASE_ASSETS"]).read_text())
    else:
        req = urllib.request.Request(f"https://api.github.com/repos/{REPO}/releases/latest",
                                     headers={"Accept": "application/vnd.github+json"})
        token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(req, timeout=20) as r:
                data = json.load(r)
        except OSError as e:
            print(f"warning: latest release not readable ({e}); using the vivi-bin pkgver and current asset names")
    if not data:
        return release_version(), None
    return data["tag_name"].removeprefix("v"), {a["name"] for a in data.get("assets", [])}


def downloads(version: str, names: set[str] | None) -> dict[str, str]:
    """Asset file name per kind, for this release; kinds the release lacks are left out."""
    out = {}
    for kind, pats in ASSETS.items():
        cands = [p.format(v=version) for p in pats]
        found = cands[0] if names is None else next((c for c in cands if c in names), None)
        if found:
            out[kind] = found
    return out


def fill_downloads(text: str, version: str, files: dict[str, str]) -> str:
    """Download placeholders in a page:
    * @DL@: the release's download URL prefix; @F_<KIND>@: an asset file name, @STEM_<KIND>@:
      the same without extension (the folder inside a zip/tarball);
    * @DL_URLS@: JSON {system-arch: URL} for the OS-detecting Download button;
    * <!--if:kind--> ... <!--/if:kind-->: kept only when the release has that asset;
    * @PICK_APPIMAGE@ / @PICK_SETUP@ / @PICK_PORTABLE@: shell / PowerShell lines that choose the
      x64 or arm64 file at run time (only x64 when the release has no arm64 build)."""
    dl = f"https://github.com/{REPO}/releases/download/v{version}"
    url = lambda k: f"{dl}/{files[k]}"
    # Fallbacks for the button: arm64 -> x64, installer -> portable zip.
    pick = lambda *ks: next((url(k) for k in ks if k in files), f"https://github.com/{REPO}/releases/latest")
    urls = {
        "linux-x64": pick("appimage_x64", "tar_x64"),
        "linux-arm64": pick("appimage_arm64", "tar_arm64", "appimage_x64"),
        "windows-x64": pick("setup_x64", "portable_x64"),
        "windows-arm64": pick("setup_arm64", "portable_arm64", "setup_x64", "portable_x64"),
        "mac": pick("mac"),
    }
    cond = re.compile(r"<!--if:([a-z0-9_]+)-->(.*?)<!--/if:\1-->", re.S)
    while cond.search(text):  # nested blocks: the inner ones are left for the next pass
        text = cond.sub(lambda m: m.group(2) if m.group(1) in files else "", text)

    def chooser(x64, arm, shell):
        if x64 not in files:
            return ""
        a, b = files[x64], files.get(arm)
        if shell == "sh":
            return f"A={a}" + (f'; [ "$(uname -m)" = aarch64 ] &amp;&amp; A={b}' if b else "")
        return f"$f = '{a}'" + (f"; if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {{ $f = '{b}' }}" if b else "")

    text = text.replace("@PICK_APPIMAGE@", chooser("appimage_x64", "appimage_arm64", "sh"))
    text = text.replace("@PICK_SETUP@", chooser("setup_x64", "setup_arm64", "ps"))
    text = text.replace("@PICK_PORTABLE@", chooser("portable_x64", "portable_arm64", "ps"))
    text = text.replace("@DL_URLS@", html.escape(json.dumps(urls), quote=True)).replace("@DL@", dl)
    for kind, name in files.items():
        stem = re.sub(r"\.(tar\.gz|zip|exe|AppImage|flatpak)$", "", name)
        text = text.replace(f"@F_{kind.upper()}@", name).replace(f"@STEM_{kind.upper()}@", stem)
    left = sorted(set(re.findall(r"@(?:F|STEM)_[A-Z0-9_]+@", text)))
    if left:
        sys.exit(f"release v{version} has no asset for {left} (wrap the use in <!--if:kind-->)")
    return text


def main() -> None:
    if OUT.exists():
        shutil.rmtree(OUT)
    shutil.copytree(ROOT / "docs/site", OUT, ignore=shutil.ignore_patterns("icons"))
    (OUT / "LICENSE-lucide.txt").write_text((ICON_DIR / "LICENSE").read_text())
    img = OUT / "img"
    img.mkdir(exist_ok=True)
    shutil.copy(ROOT / "packaging/vivi.svg", img / "vivi.svg")
    for name in IMAGES:
        shutil.copy(ROOT / f"docs/screenshots/{name}.png", img / f"{name}.png")
    # install.sh builds this on Arch (kept at the latest release by the release workflow).
    (OUT / "vivi-bin").mkdir(exist_ok=True)
    shutil.copy(ROOT / "packaging/aur/vivi-bin/PKGBUILD", OUT / "vivi-bin/PKGBUILD")
    # Download links and commands follow the latest release and its asset names.
    version, names = latest_release()
    files = downloads(version, names)
    print(f"release v{version}: {', '.join(f'{k}={v}' for k, v in files.items())}")
    for page in OUT.glob("*.html"):
        text = fill_downloads(page.read_text().replace("@VIVI_VERSION@", version), version, files)
        page.write_text(seo(page.name, inline_icons(text)))
    sitemap(OUT)
    missing = [
        (page.name, ref)
        for page in OUT.glob("*.html")
        for ref in re.findall(r'(?:href|src)="([^"#:]+)"', page.read_text())
        if not (OUT / ref).exists()
    ]
    if missing:
        sys.exit(f"broken local links: {missing}")
    print(f"site built in {OUT}")


if __name__ == "__main__":
    main()
