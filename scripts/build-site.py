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
import re
import json
import shutil
import sys
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
    "ai-agents.html": ("Control the Vivi media player from Claude, Cursor, Claude Code and other AI agents with the vivi-mcp MCP server: example prompts, setup and all 30 tools.", "0.5"),
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


IMAGES = ["og", "player", "extensions", "lyrics", "skip-intro", "add-subtitles", "visualizer", "library-home",
          "settings", "panel-subtitles", "menu-main", "miniplayer", "panel-audio", "network"]


def icon(name: str) -> str:
    """Inline Lucide SVG for `name` (falls back to "puzzle")."""
    f = ICON_DIR / f"{name}.svg"
    if not f.exists():
        f = ICON_DIR / "puzzle.svg"
    svg = re.sub(r"<!--.*?-->\s*", "", f.read_text(), flags=re.S).strip()
    return svg.replace('class="lucide', 'aria-hidden="true" class="lucide', 1)


def inline_icons(text: str) -> str:
    return re.sub(r'<i data-lucide="([a-z0-9-]+)"></i>', lambda m: icon(m.group(1)), text)


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
    for page in OUT.glob("*.html"):
        page.write_text(seo(page.name, inline_icons(page.read_text())))
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
