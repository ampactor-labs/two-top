#!/usr/bin/env python3
"""Silhouette-distinctness gate — the panel's flood test, committed.

The Cur (P0) and Stag (P1) must read apart as solid-black shapes, not just by
color (audit D-VQ-01). This floods both idle silhouettes and fails if they are
too similar — so a colorblind player or a compressed stream can still tell the
two duelists apart. Pairs with scripts/check_palette.py as the art QA gate.

    python3 scripts/check_silhouettes.py
"""
from __future__ import annotations

import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    sys.exit("Pillow required: pip install pillow")

ROOT = Path(__file__).resolve().parent.parent
S = 48  # player source cell (cloaked-drifter rig, HLD overhaul)
CUR = ROOT / "assets/sprites/players/duelist_a_sheet.png"
STAG = ROOT / "assets/sprites/players/duelist_b_sheet.png"
# Two cues must both hold. XOR ratio = share of the combined silhouette that
# differs (the broad round hood vs the tall peaked hood + scarf diverge hard).
# Body thickness = median per-row solid-pixel count — the Cur is a broad cloak,
# the Stag a narrow column, and unlike a bounding-box width this ignores the
# thin trailing scarf that would otherwise inflate the Stag's footprint.
MIN_XOR_RATIO = 0.18
MIN_THICKNESS_DELTA = 4


def mask(path: Path, frame: int = 0) -> set[tuple[int, int]]:
    im = Image.open(path).convert("RGBA").crop((frame * S, 0, frame * S + S, S))
    px = im.load()
    return {(x, y) for y in range(S) for x in range(S) if px[x, y][3] > 0}


def thickness(m: set[tuple[int, int]]) -> float:
    """Median solid-pixel count over the rows the silhouette occupies — its
    typical body width, robust to a thin trailing scarf."""
    rows: dict[int, int] = {}
    for _, y in m:
        rows[y] = rows.get(y, 0) + 1
    counts = sorted(rows.values())
    return counts[len(counts) // 2] if counts else 0.0


def main() -> int:
    a, b = mask(CUR), mask(STAG)
    if not a or not b:
        print("FAIL: an idle silhouette is empty")
        return 1
    xor = a ^ b
    union = a | b
    ratio = len(xor) / len(union)
    ta, tb = thickness(a), thickness(b)
    dt = abs(ta - tb)
    print(f"Cur body thickness {ta:.0f}px, Stag {tb:.0f}px (Δ{dt:.0f}px); "
          f"silhouettes differ on {ratio:.0%} of their union.")
    if ratio < MIN_XOR_RATIO or dt < MIN_THICKNESS_DELTA:
        print(f"FAIL: too similar (need ≥{MIN_XOR_RATIO:.0%} XOR and ≥"
              f"{MIN_THICKNESS_DELTA}px thickness delta). The Cur and Stag must "
              f"read apart by body shape, not color alone (audit D-VQ-01).")
        return 1
    print("OK — the Cur and Stag read apart by silhouette.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
