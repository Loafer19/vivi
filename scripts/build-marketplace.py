#!/usr/bin/env python3
"""Build the plugin marketplace (https://plugins.viviplayer.app) into _market/ (or argv[1]).

Input: plugins/index.json (the catalog the app reads; also copied to /index.json),
plugins/<id>/store.toml (changelog; categories/tags/screenshots are already in the index)
and screenshots from docs/screenshots/. Output: a static site with a filterable grid
(index.html, a few lines of JavaScript for search/filter/sort; without JavaScript every
card is still listed), one page per plugin (/p/<id>/), sitemap.xml, robots.txt and an
Atom feed of plugin updates (feed.xml). Standard library only.
"""
import html
import json
import re
import shutil
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "_market"
BASE = "https://plugins.viviplayer.app"
SITE = "https://viviplayer.app"
ICONS = ROOT / "docs/site/icons"
CATEGORIES = ["Music", "Video", "Subtitles", "Streaming", "Integrations", "Tools"]
CAT_ICON = {"Music": "music", "Video": "film", "Subtitles": "captions", "Streaming": "radio",
            "Integrations": "plug", "Tools": "wrench"}
PERM = {"network": ("globe", "Internet access"), "filesystem": ("folder-open", "Reads and writes files outside its folder"),
        "subprocess": ("terminal", "Runs other programs")}
PLAT = {"linux-x86_64": "Linux x86_64", "linux-aarch64": "Linux ARM64", "windows-x86_64": "Windows",
        "macos-x86_64": "macOS Intel", "macos-aarch64": "macOS Apple silicon"}
e = lambda s: html.escape(str(s), quote=True)


def icon(name: str) -> str:
    f = ICONS / f"{name}.svg"
    if not f.exists():
        f = ICONS / "puzzle.svg"
    svg = re.sub(r"<!--.*?-->\s*", "", f.read_text(), flags=re.S).strip()
    return svg.replace('class="lucide', 'aria-hidden="true" class="lucide', 1)


def kb(n: int) -> str:
    return f"{n / 1024:.0f} KB" if n < 1 << 20 else f"{n / (1 << 20):.1f} MB"


def platforms(p: dict) -> list[str]:
    if p.get("kind") == "builtin":
        return ["Built in"]
    if p.get("binaries"):
        return sorted({PLAT.get(b["platform"], b["platform"]) for b in p["binaries"]})
    return ["Linux", "Windows", "macOS"]


def changelog(p: dict) -> list[dict]:
    if p.get("changelog"):
        return p["changelog"]
    f = ROOT / "plugins" / p["id"] / "store.toml"
    return tomllib.loads(f.read_text()).get("changelog", []) if f.exists() else []


def shot_path(name: str) -> Path | None:
    for f in (ROOT / f"docs/screenshots/plugins/{name}.png", ROOT / f"docs/screenshots/{name}.png"):
        if f.exists():
            return f
    return None


def zip_link(c: dict) -> str:
    return f' · <a href="{e(c["url"])}">download zip</a>' if c.get("url") else ""


def head(title: str, desc: str, url: str, image: str, extra: str = "", depth: int = 0) -> str:
    up = "../" * depth
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{e(title)}</title>
<meta name="description" content="{e(desc)}">
<link rel="canonical" href="{url}">
<meta name="theme-color" content="#0e0e0e">
<meta property="og:type" content="website"><meta property="og:site_name" content="Vivi plugins">
<meta property="og:title" content="{e(title)}"><meta property="og:description" content="{e(desc)}">
<meta property="og:url" content="{url}"><meta property="og:image" content="{image}">
<meta name="twitter:card" content="summary_large_image"><meta name="twitter:title" content="{e(title)}">
<meta name="twitter:description" content="{e(desc)}"><meta name="twitter:image" content="{image}">
<link rel="alternate" type="application/atom+xml" title="Vivi plugin updates" href="{BASE}/feed.xml">
<link rel="icon" href="{up}img/vivi.svg">
<link rel="stylesheet" href="{up}market.css">
{extra}
</head>
<body>
<header class="nav"><div class="in">
  <a class="brand" href="{up}./"><img src="{up}img/vivi.svg" alt="" width="26" height="26">Vivi plugins</a>
  <nav>
    <a href="{SITE}/">{icon('house')}<span class="wide">Vivi</span></a>
    <a href="{SITE}/plugin-dev.html">{icon('code-xml')}<span class="wide">Write a plugin</span></a>
    <a href="{up}feed.xml">{icon('rss')}<span class="wide">Updates</span></a>
    <a href="https://github.com/Loafer19/vivi">{icon('git-fork')}<span class="wide">GitHub</span></a>
  </nav>
</div></header>
<main>
"""


FOOT = f"""</main>
<footer>Vivi plugin marketplace · <a href="{SITE}/">viviplayer.app</a> · catalog as <a href="/index.json">index.json</a> ·
icons from <a href="https://lucide.dev">Lucide</a> (ISC) · screenshots show <a href="https://durian.blender.org">Sintel</a> (© Blender Foundation, CC BY 3.0)</footer>
</body>
</html>
"""


def card(p: dict) -> str:
    cats = p.get("categories", [])
    shots = [s for s in p.get("screenshots", []) if shot_path(s)]
    thumb = (f'<img class="thumb" src="img/shots/{e(shots[0])}.webp" alt="{e(p["name"])} screenshot" loading="lazy" width="640" height="360">'
             if shots else f'<div class="thumb ph">{icon(p.get("lucide", "puzzle"))}</div>')
    size = kb(p["size"]) if p.get("size") else "built in"
    kind = "Built-in" if p.get("kind") == "builtin" else ("Example script" if p.get("kind") == "script" else "Download")
    date = (changelog(p) or [{}])[0].get("date", "")
    return f"""<a class="card" href="p/{e(p['id'])}/" data-cats="{e('|'.join(cats))}" data-tags="{e(' '.join(p.get('tags', [])))}"
 data-name="{e(p['name'].lower())}" data-text="{e((p['name'] + ' ' + p['description'] + ' ' + ' '.join(p.get('tags', []))).lower())}"
 data-size="{p.get('size', 0)}" data-date="{e(date)}">
  {thumb}
  <div class="body">
    <div class="title"><span class="ic">{icon(p.get('lucide', 'puzzle'))}</span><h2>{e(p['name'])}</h2></div>
    <p>{e(p['description'])}</p>
    <div class="meta"><span>v{e(p.get('version', ''))}</span><span>{e(size)}</span><span>{e(', '.join(platforms(p)))}</span></div>
    <div class="chips"><span class="chip k">{e(kind)}</span>{''.join(f'<span class="chip">{e(c)}</span>' for c in cats)}</div>
  </div>
</a>"""


def page(p: dict) -> str:
    pid = p["id"]
    url = f"{BASE}/p/{pid}/"
    shots = [s for s in p.get("screenshots", []) if shot_path(s)]
    image = f"{BASE}/img/shots/{shots[0]}.png" if shots else f"{SITE}/img/og.png"
    desc = p["description"]
    ld = {"@context": "https://schema.org", "@type": "SoftwareApplication", "name": p["name"],
          "applicationCategory": "MultimediaApplication", "applicationSubCategory": ", ".join(p.get("categories", [])),
          "operatingSystem": ", ".join(platforms(p)), "softwareVersion": p.get("version", ""), "description": desc,
          "url": url, "image": image, "author": {"@type": "Organization", "name": p.get("author", "Vivi")},
          "isPartOf": {"@type": "SoftwareApplication", "name": "Vivi", "url": SITE + "/"},
          "offers": {"@type": "Offer", "price": "0", "priceCurrency": "USD"}}
    if p.get("url"):
        ld["downloadUrl"] = p["url"]
    out = head(f"{p['name']} – Vivi plugin", desc, url, image,
               f'<script type="application/ld+json">{json.dumps(ld)}</script>', depth=2)
    gallery = "".join(
        f'<figure><a href="../../img/shots/{e(s)}.png"><img src="../../img/shots/{e(s)}.webp" alt="{e(p["name"])} in Vivi ({i + 1})" '
        f'loading="{"eager" if i == 0 else "lazy"}" width="1280" height="720"></a></figure>' for i, s in enumerate(shots))
    perms = p.get("permissions", [])
    perm_html = "".join(f"<li>{icon(PERM.get(x, ('shield', x))[0])}{e(PERM.get(x, ('', x))[1])}</li>" for x in perms) \
        or f"<li>{icon('shield-check')}No special permissions: runs inside Vivi's mpv only</li>"
    if p.get("kind") == "builtin":
        perm_html = f"<li>{icon('shield-check')}Part of Vivi itself; turn it on or off in Settings → Extensions</li>"
    cl = changelog(p)
    cl_html = "".join(
        f"<li><div class=v><b>v{e(c['version'])}</b> <time datetime=\"{e(c.get('date', ''))}\">{e(c.get('date', ''))}</time>"
        f"{zip_link(c)}{' · <em>latest</em>' if c['version'] == p.get('version') else ''}</div>"
        f"<p>{e(c.get('notes', ''))}</p></li>" for c in cl) or "<li><p>Ships with Vivi; changes are listed in the <a href=\"https://github.com/Loafer19/vivi/blob/master/CHANGELOG.md\">Vivi changelog</a>.</p></li>"
    settings = "".join(f"<li><b>{e(s['title'])}</b> <span class=muted>({e(s['type'])})</span></li>" for s in p.get("settings", []) if s["type"] != "action")
    install = ""
    if p.get("kind") == "package":
        install = f"""<a class="btn primary" href="vivi://plugin/{e(pid)}">{icon('external-link')}Open in Vivi</a>
<a class="btn ghost" href="{e(p['url'])}">{icon('download')}Download zip ({e(kb(p['size']))})</a>
<p class="muted small">"Open in Vivi" shows the plugin in Settings → Extensions → Browse, where you confirm its permissions and install it.
Needs Vivi {e(p.get('min_vivi', ''))} or newer. No Vivi yet? <code>curl -fsSL {SITE}/install.sh | sh</code></p>"""
    elif p.get("kind") == "builtin":
        install = f"""<a class="btn primary" href="vivi://plugin/{e(pid)}">{icon('external-link')}Open in Vivi</a>
<p class="muted small">Built into Vivi: enable it in Settings → Extensions. <a href="{SITE}/#install">Install Vivi</a></p>"""
    else:
        install = f"""<a class="btn ghost" href="https://github.com/Loafer19/vivi/tree/master/{e(p.get('source', ''))}">{icon('code-xml')}View the script</a>"""
    src = f' · <a href="https://github.com/Loafer19/vivi/tree/master/{e(p["source"])}">source</a>' if p.get("source") else ""
    out += f"""<nav class="crumbs"><a href="../../">All plugins</a> / {e(p['name'])}</nav>
<section class="phead">
  <div class="bigic">{icon(p.get('lucide', 'puzzle'))}</div>
  <div><h1>{e(p['name'])}</h1>
  <p class="lead">{e(desc)}</p>
  <div class="meta"><span>v{e(p.get('version', ''))}</span><span>{e(kb(p['size']) if p.get('size') else 'built in')}</span>
  <span>{e(', '.join(platforms(p)))}</span><span>by {e(p.get('author', ''))}{src}</span></div>
  <div class="chips">{''.join(f'<a class="chip" href="../../?cat={e(c)}">{e(c)}</a>' for c in p.get('categories', []))}
  {''.join(f'<span class="chip t">#{e(t)}</span>' for t in p.get('tags', []))}</div>
  <div class="actions">{install}</div></div>
</section>
{f'<h2>Screenshots</h2><div class="gallery">{gallery}</div>' if gallery else ''}
<div class="cols">
<section><h2>Changelog</h2><ol class="changelog">{cl_html}</ol></section>
<section><h2>Permissions</h2><ul class="perms">{perm_html}</ul>
{f'<h2>Settings</h2><ul class="settings">{settings}</ul>' if settings else ''}</section>
</div>
"""
    return out + FOOT.replace('href="/index.json"', 'href="../../index.json"')


def main() -> None:
    idx = json.loads((ROOT / "plugins/index.json").read_text())
    plugins = [p for p in idx["plugins"]]
    if OUT.exists():
        shutil.rmtree(OUT)
    (OUT / "img/shots").mkdir(parents=True)
    shutil.copy(ROOT / "plugins/index.json", OUT / "index.json")
    shutil.copy(ROOT / "docs/marketplace/market.css", OUT / "market.css")
    shutil.copy(ROOT / "packaging/vivi.svg", OUT / "img/vivi.svg")
    for p in plugins:
        for s in p.get("screenshots", []):
            f = shot_path(s)
            if f:
                shutil.copy(f, OUT / f"img/shots/{s}.png")
                w = f.parent / "webp" / f"{s}.webp"
                if w.exists():
                    shutil.copy(w, OUT / f"img/shots/{s}.webp")
                else:
                    shutil.copy(f, OUT / f"img/shots/{s}.webp")  # browsers sniff the PNG
    order = sorted(plugins, key=lambda p: (p.get("kind") == "script", p["name"].lower()))
    cats = "".join(f'<button type="button" data-cat="{c}">{icon(CAT_ICON[c])}{c}</button>' for c in CATEGORIES)
    desc = (f"{len(plugins)} plugins for the Vivi media player: music, video, subtitles, streaming, integrations and tools. "
            "Install them in one click from Vivi.")
    ld = {"@context": "https://schema.org", "@type": "ItemList", "name": "Vivi plugins",
          "itemListElement": [{"@type": "ListItem", "position": i + 1, "url": f"{BASE}/p/{p['id']}/", "name": p["name"]}
                              for i, p in enumerate(order)]}
    out = head("Vivi plugins – extensions for the Vivi media player", desc, BASE + "/", f"{SITE}/img/og.png",
               f'<script type="application/ld+json">{json.dumps(ld)}</script>')
    out += f"""<section class="hero"><h1>Plugins for <span>Vivi</span></h1>
<p>{len(plugins)} extensions. Built-in ones ship with Vivi; downloadable ones install from Settings → Extensions → Browse
with checksums verified and permissions shown before install.</p></section>
<div class="toolbar">
  <label class="search">{icon('search')}<input id="q" type="search" placeholder="Search plugins" aria-label="Search plugins"></label>
  <div class="cats" role="group" aria-label="Category"><button type="button" data-cat="" class="on">All</button>{cats}</div>
  <label class="sort">Sort <select id="sort" aria-label="Sort"><option value="name">Name</option><option value="date">Recently updated</option><option value="size">Size</option></select></label>
</div>
<p id="empty" class="muted" hidden>No plugins match.</p>
<div class="grid" id="grid">
{''.join(card(p) for p in order)}
</div>
<script src="market.js" defer></script>
""" + FOOT
    (OUT / "index.html").write_text(out)
    shutil.copy(ROOT / "docs/marketplace/market.js", OUT / "market.js")
    for p in plugins:
        d = OUT / "p" / p["id"]
        d.mkdir(parents=True)
        (d / "index.html").write_text(page(p))
    urls = [BASE + "/"] + [f"{BASE}/p/{p['id']}/" for p in order]
    (OUT / "sitemap.xml").write_text('<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
                                     + "".join(f"<url><loc>{u}</loc></url>" for u in urls) + "</urlset>\n")
    (OUT / "robots.txt").write_text(f"User-agent: *\nAllow: /\nSitemap: {BASE}/sitemap.xml\n")
    # Atom feed: one entry per plugin version, newest first.
    items = sorted(((c.get("date", ""), p, c) for p in plugins for c in changelog(p)), key=lambda t: (t[0], t[2]["version"]), reverse=True)
    stamp = lambda d: f"{d}T00:00:00Z" if d else datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    x = lambda s: html.escape(str(s), quote=False)
    entries = "".join(f"""<entry><title>{x(p['name'])} {x(c['version'])}</title>
<id>{BASE}/p/{p['id']}/#v{x(c['version'])}</id><link href="{BASE}/p/{p['id']}/"/>
<updated>{stamp(d)}</updated><summary>{x(c.get('notes', ''))}</summary></entry>
""" for d, p, c in items[:50])
    (OUT / "feed.xml").write_text(f"""<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Vivi plugin updates</title>
<id>{BASE}/feed.xml</id><link rel="self" href="{BASE}/feed.xml"/><link href="{BASE}/"/>
<updated>{stamp(items[0][0] if items else '')}</updated><author><name>Vivi</name></author>
{entries}</feed>
""")
    print(f"marketplace built in {OUT} ({len(plugins)} plugins, {len(items)} feed entries)")


if __name__ == "__main__":
    main()
