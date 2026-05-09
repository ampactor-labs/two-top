#!/usr/bin/env python3
"""Superseded by `generate_polished_assets.py`.

The Phase 15 asset pipeline graduated from rough deterministic
placeholders to hand-designed polished pixel art (still deterministic,
still palette-locked, still no external libs). The polished generator
writes to the same atlas paths.

Use:

    python3 scripts/generate_polished_assets.py

This stub exists so old shell history / docs that referenced the
placeholder script don't accidentally overwrite the polished art with
rough seed sprites.
"""

from __future__ import annotations

import sys


def main() -> int:
    print(__doc__, file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
