#!/usr/bin/env python3
"""Split two-tone icons into colour layers so Slint can tint each one from the theme.

For assets/icons/<p>.svg this writes assets/icons/layers/<p with / as _>-{b,a,d}.svg:
b = text-coloured parts, a = accent (#D37918), d = dim accent (#AB6314).
Parts of other layers get paint "none", so geometry and stacking order stay intact.
Run after changing an icon:  python3 tools/icon_layers.py
"""
import re, pathlib
ROOT = pathlib.Path(__file__).resolve().parent.parent / "assets/icons"
OUT = ROOT / "layers"
ACC, DIM = "#D37918", "#AB6314"
PAINT = re.compile(r'((?:fill|stroke)=")(#[0-9A-Fa-f]{3,8})(")')

def layer(svg, keep):
    def sub(m):
        c = m.group(2).upper()
        kind = "a" if c == ACC else "d" if c == DIM else "b"
        return m.group(0) if kind == keep else m.group(1) + "none" + m.group(3)
    return PAINT.sub(sub, svg)

def main():
    OUT.mkdir(exist_ok=True)
    (OUT / "empty.svg").write_text('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"/>')
    for p in sorted(ROOT.rglob("*.svg")):
        if OUT in p.parents:
            continue
        name = str(p.relative_to(ROOT).with_suffix("")).replace("/", "_")
        svg = p.read_text()
        if ACC not in svg.upper() and DIM not in svg.upper():
            continue
        for k in "bad":
            (OUT / f"{name}-{k}.svg").write_text(layer(svg, k))

if __name__ == "__main__":
    main()
