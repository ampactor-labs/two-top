#!/usr/bin/env python3
"""Generate the PWA / home-screen icons for the web build.

The browser build is how anyone without an Android phone plays (iPhone
especially), and "Add to Home Screen" is the closest thing it has to an
install. That needs real icons, so they are generated here from the
game's own duelist sheet and the locked 16-color palette rather than
drawn by hand — the icon can never drift off-palette or out of date with
the sprites.

Output lands in `web/` (NOT `assets/`): these are page chrome, not game
art, and `scripts/check_palette.py` deliberately only scans the shipped
sprite/arena/hud dirs.

Run: python3 scripts/generate_web_icons.py
"""

from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PLAYERS = ROOT / "assets/sprites/players"
OUT = ROOT / "web"

# From assets/palettes/two_top_16.gpl.
VOID = (16, 14, 34, 255)
DEEP_ASH = (32, 28, 66, 255)
HOT_BONE = (255, 243, 202, 255)

CELL = 48
FACING_SIDE_ROW = 0
# iOS masks the icon to a squircle with no say from us, so everything that
# must survive stays inside the middle. 0.80 is Android's maskable safe
# zone and comfortably clears the iOS mask too.
SAFE = 0.80


def idle_side(sheet_name: str) -> Image.Image:
    """Frame 0 of the side-facing row, cropped to its own ink."""
    sheet = Image.open(PLAYERS / sheet_name).convert("RGBA")
    cell = sheet.crop((0, FACING_SIDE_ROW * CELL, CELL, FACING_SIDE_ROW * CELL + CELL))
    return cell.crop(cell.getbbox())


def build(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size), VOID)

    # The bordered-box language every button in the game wears, inset so
    # the corner mask never eats the frame.
    pad = round(size * 0.055)
    stroke = max(2, round(size * 0.018))
    box = Image.new("RGBA", (size - 2 * pad, size - 2 * pad), HOT_BONE)
    img.paste(box, (pad, pad))
    inner = Image.new(
        "RGBA",
        (size - 2 * (pad + stroke), size - 2 * (pad + stroke)),
        DEEP_ASH,
    )
    img.paste(inner, (pad + stroke, pad + stroke))

    # The duel: P0 on the left, P1 on the right, squared up at each other.
    # The side row is drawn facing right, so only the right-hand duelist
    # is mirrored. Nearest-neighbour keeps them pixel art at every scale.
    a = idle_side("duelist_a_sheet.png")
    b = idle_side("duelist_b_sheet.png").transpose(Image.FLIP_LEFT_RIGHT)

    target_h = round(size * SAFE * 0.72)
    scaled = []
    for sprite in (a, b):
        w = max(1, round(sprite.width * target_h / sprite.height))
        scaled.append(sprite.resize((w, target_h), Image.NEAREST))

    gap = round(size * 0.055)
    total = sum(s.width for s in scaled) + gap
    x = (size - total) // 2
    y = (size - target_h) // 2
    for sprite in scaled:
        img.alpha_composite(sprite, (x, y))
        x += sprite.width + gap
    return img


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    # 192/512 are the PWA manifest's two required sizes; 180 is what iOS
    # reads from <link rel="apple-touch-icon">.
    for size, name in ((512, "icon-512.png"), (192, "icon-192.png"), (180, "apple-touch-icon.png")):
        path = OUT / name
        build(size).save(path)
        print(f"wrote {path.relative_to(ROOT)} ({size}x{size})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
