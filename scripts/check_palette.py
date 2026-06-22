#!/usr/bin/env python3
"""Palette-conformance gate — the consistency backbone of the pixel atelier.

Every shipped sprite/arena/hud pixel must be either fully transparent or one
of the locked 16 colors in `assets/palettes/two_top_16.gpl`. This is what makes
the art reliably *this game's* palette rather than drifting — and it catches
exactly the kind of off-palette blend the v2 audit found in the floor vignette.

Deterministic, stdlib + Pillow only. Exit 1 on any violation so it can gate CI.
Concept boards under assets/concepts/** are exempt (AI/reference source).

    python3 scripts/check_palette.py
"""
from __future__ import annotations

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    sys.exit("Pillow required: pip install pillow")

ROOT = Path(__file__).resolve().parent.parent
GPL = ROOT / "assets/palettes/two_top_16.gpl"
SCAN_DIRS = ["assets/sprites", "assets/arenas", "assets/hud"]
# training_floor's edge vignette intentionally blends toward Void; allow a tiny
# off-palette budget per file before failing (0 = strict). Tunable per policy.
MAX_OFF_PALETTE_PER_FILE = 0


def load_palette(path: Path) -> set[tuple[int, int, int]]:
    cols: set[tuple[int, int, int]] = set()
    for line in path.read_text().splitlines():
        parts = line.split()
        if len(parts) >= 3 and parts[0].isdigit():
            cols.add((int(parts[0]), int(parts[1]), int(parts[2])))
    return cols


def check(img_path: Path, palette: set[tuple[int, int, int]]) -> dict[tuple[int, int, int, int], int]:
    """Return a map of offending RGBA -> pixel count for one image."""
    im = Image.open(img_path).convert("RGBA")
    bad: dict[tuple[int, int, int, int], int] = {}
    for r, g, b, a in im.getdata():
        if a == 0:
            continue  # fully transparent is always fine
        if a != 255 or (r, g, b) not in palette:
            key = (r, g, b, a)
            bad[key] = bad.get(key, 0) + 1
    return bad


def main() -> int:
    palette = load_palette(GPL)
    if len(palette) != 16:
        print(f"!! expected 16 palette colors, got {len(palette)} in {GPL}")
        return 1
    files = sorted(
        p for d in SCAN_DIRS for p in (ROOT / d).rglob("*.png")
    )
    total_bad_files = 0
    for f in files:
        bad = check(f, palette)
        off = sum(bad.values())
        if off > MAX_OFF_PALETTE_PER_FILE:
            total_bad_files += 1
            rel = f.relative_to(ROOT)
            print(f"FAIL {rel}: {off} off-palette px across {len(bad)} colors")
            for (r, g, b, a), n in sorted(bad.items(), key=lambda kv: -kv[1])[:4]:
                tag = "semi-alpha" if a != 255 else "off-palette"
                print(f"       {n:>6}x  #{r:02x}{g:02x}{b:02x} a={a}  ({tag})")
    n = len(files)
    if total_bad_files:
        print(f"\n{total_bad_files}/{n} files violate the 16-color lock "
              f"(budget {MAX_OFF_PALETTE_PER_FILE}/file).")
        return 1
    print(f"OK — all {n} shipped assets are palette-locked to the 16 colors.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
