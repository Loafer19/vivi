#!/usr/bin/env python3
"""Build the docs site into _site/ (or the directory given as the first argument).

Copies docs/site/, adds the logo and screenshots, and renders plugins.html (the plugin
catalog) from the plugin catalog into docs/site/plugins.template.html. The catalog lives in
https://github.com/Loafer19/vivi-plugins (plugins/index.json); it is downloaded at build time
(or read from $VIVI_PLUGINS_INDEX, a local file). The catalog itself is served by
plugins.viviplayer.app and the vivi-plugins GitHub Pages, not by this site.
Icons are Lucide (https://lucide.dev, ISC license) SVGs kept in docs/site/icons/; every
<i data-lucide="name"></i> placeholder is replaced by the inline SVG, so the site needs no JavaScript.
Standard library only, so it runs as-is in GitHub Actions.
"""
import html
import re
import json
import shutil
import os
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "_site"
ICON_DIR = ROOT / "docs/site/icons"
SITE = "https://viviplayer.app"
MARKET = "https://plugins.viviplayer.app"
INDEX_URL = "https://raw.githubusercontent.com/Loafer19/vivi-plugins/master/plugins/index.json"
# Per page: (description, sitemap priority). Canonical URLs always point at viviplayer.app,
# also on the GitHub Pages mirror (loafer19.github.io/vivi), so search engines index one copy.
PAGES = {
    "index.html": ("Vivi is a fast, lightweight desktop media player for Linux written in Rust on top of mpv: "
                   "plugins, music visualizer, synced lyrics, library and a clean dark UI.", "1.0"),
    "plugins.html": ("All Vivi plugins: built-in and downloadable extensions for music, video, subtitles and streaming.", "0.8"),
    "plugin-dev.html": ("Write a Vivi plugin: mpv Lua and JavaScript scripts, manifest.toml, settings, packaging and publishing.", "0.6"),
    "ai-agents.html": ("Control the Vivi media player from AI agents with the vivi-mcp Model Context Protocol server.", "0.5"),
    "architecture.html": ("How Vivi distributes plugins: packages, catalog, checksums, permissions, updates and rollback.", "0.4"),
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


PERM_TEXT = {"network": "Internet access", "filesystem": "Files outside its folder", "subprocess": "Runs programs"}
KIND_TAG = {"script": ("script", "mpv script"), "package": ("package", "downloadable"), "builtin": ("", "built-in")}


def card(p: dict) -> str:
    e = html.escape
    settings = "".join(
        f"<li><code>{e(s['key'])}</code> {e(s['title'])} <span class=meta>({e(s['type'])})</span></li>"
        for s in p.get("settings", [])
    )
    kind = p.get("kind", "builtin")
    src = p.get("source")
    link = (
        f' · <a href="https://github.com/Loafer19/vivi-plugins/tree/master/{e(src)}">source</a>' if src else ""
    )
    cls, label = KIND_TAG.get(kind, KIND_TAG["builtin"])
    extra = ""
    if kind == "package":
        perms = p.get("permissions", [])
        ptxt = ", ".join(PERM_TEXT.get(x, x) for x in perms) or "No special permissions"
        extra = f"""<div class=install><span class="tag install">{icon('download')}Install in Vivi</span>
  <span class=meta>Settings → Extensions → Browse · {e(f"{p.get('size', 0) / 1024:.0f}")} KB ·
  <a href="{e(p['url'])}">zip</a> · needs Vivi {e(p.get('min_vivi', ''))}+</span></div>
  <div class=perms>{icon('shield-check')}{e(ptxt)}</div>"""
    return f"""<div class="card plugin" id="{e(p['id'])}">
  <div class=top><div class=icon>{icon(p.get('lucide', 'puzzle'))}</div>
    <div><h3>{e(p['name'])}</h3>
    <div class=meta>{e(p['id'])} · v{e(p.get('version', ''))} · {e(p.get('author', ''))}{link}</div></div></div>
  <p>{e(p['description'])}</p>
  <div><span class="tag {cls}">{label}</span>
  {'' if p.get('default_enabled', True) else '<span class=tag>off by default</span>'}</div>
  {extra}
  {f'<ul>{settings}</ul>' if settings else ''}
</div>"""


def catalog() -> str:
    local = os.environ.get("VIVI_PLUGINS_INDEX")
    if local:
        return Path(local).read_text()
    with urllib.request.urlopen(INDEX_URL, timeout=30) as r:
        return r.read().decode()


def main() -> None:
    raw = catalog()
    idx = json.loads(raw)
    if OUT.exists():
        shutil.rmtree(OUT)
    shutil.copytree(ROOT / "docs/site", OUT, ignore=shutil.ignore_patterns("*.template.html", "icons"))
    (OUT / "LICENSE-lucide.txt").write_text((ICON_DIR / "LICENSE").read_text())
    img = OUT / "img"
    img.mkdir(exist_ok=True)
    shutil.copy(ROOT / "packaging/vivi.svg", img / "vivi.svg")
    for name in IMAGES:
        shutil.copy(ROOT / f"docs/screenshots/{name}.png", img / f"{name}.png")
    # install.sh builds this on Arch (kept at the latest release by the release workflow).
    (OUT / "vivi-bin").mkdir(exist_ok=True)
    shutil.copy(ROOT / "packaging/aur/vivi-bin/PKGBUILD", OUT / "vivi-bin/PKGBUILD")
    tpl = (ROOT / "docs/site/plugins.template.html").read_text()
    cards = "\n".join(card(p) for p in idx["plugins"])
    (OUT / "plugins.html").write_text(tpl.replace("<!--PLUGINS-->", cards).replace("<!--COUNT-->", str(len(idx["plugins"]))))
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
    print(f"site built in {OUT} ({len(idx['plugins'])} plugins)")


if __name__ == "__main__":
    main()
