#!/usr/bin/env python3
"""Build the docs site into _site/ (or the directory given as the first argument).

Copies docs/site/, adds the logo and screenshots, and renders plugins.html (the plugin
catalog) from plugins/index.json into docs/site/plugins.template.html.
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
IMAGES = ["player", "extensions", "lyrics", "skip-intro", "add-subtitles", "visualizer", "library-home",
          "settings", "panel-subtitles", "menu-main", "miniplayer"]


def icon(name: str) -> str:
    """Inline Lucide SVG for `name` (falls back to "puzzle")."""
    f = ICON_DIR / f"{name}.svg"
    if not f.exists():
        f = ICON_DIR / "puzzle.svg"
    svg = re.sub(r"<!--.*?-->\s*", "", f.read_text(), flags=re.S).strip()
    return svg.replace('class="lucide', 'aria-hidden="true" class="lucide', 1)


def inline_icons(text: str) -> str:
    return re.sub(r'<i data-lucide="([a-z0-9-]+)"></i>', lambda m: icon(m.group(1)), text)


def card(p: dict) -> str:
    e = html.escape
    settings = "".join(
        f"<li><code>{e(s['key'])}</code> {e(s['title'])} <span class=meta>({e(s['type'])})</span></li>"
        for s in p.get("settings", [])
    )
    kind = p.get("kind", "builtin")
    src = p.get("source")
    link = (
        f' · <a href="https://github.com/Loafer19/vivi/blob/master/{e(src)}">source</a>' if src else ""
    )
    return f"""<div class="card plugin" id="{e(p['id'])}">
  <div class=top><div class=icon>{icon(p.get('lucide', 'puzzle'))}</div>
    <div><h3>{e(p['name'])}</h3>
    <div class=meta>{e(p['id'])} · v{e(p.get('version', ''))} · {e(p.get('author', ''))}{link}</div></div></div>
  <p>{e(p['description'])}</p>
  <div><span class="tag {'script' if kind == 'script' else ''}">{'mpv script' if kind == 'script' else 'built-in'}</span>
  {'' if p.get('default_enabled', True) else '<span class=tag>off by default</span>'}</div>
  {f'<ul>{settings}</ul>' if settings else ''}
</div>"""


def main() -> None:
    idx = json.loads((ROOT / "plugins/index.json").read_text())
    if OUT.exists():
        shutil.rmtree(OUT)
    shutil.copytree(ROOT / "docs/site", OUT, ignore=shutil.ignore_patterns("*.template.html", "icons"))
    (OUT / "LICENSE-lucide.txt").write_text((ICON_DIR / "LICENSE").read_text())
    img = OUT / "img"
    img.mkdir(exist_ok=True)
    shutil.copy(ROOT / "packaging/vivi.svg", img / "vivi.svg")
    for name in IMAGES:
        shutil.copy(ROOT / f"docs/screenshots/{name}.png", img / f"{name}.png")
    shutil.copy(ROOT / "plugins/index.json", OUT / "index.json")
    tpl = (ROOT / "docs/site/plugins.template.html").read_text()
    cards = "\n".join(card(p) for p in idx["plugins"])
    (OUT / "plugins.html").write_text(tpl.replace("<!--PLUGINS-->", cards).replace("<!--COUNT-->", str(len(idx["plugins"]))))
    for page in OUT.glob("*.html"):
        page.write_text(inline_icons(page.read_text()))
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
