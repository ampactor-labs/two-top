#!/usr/bin/env python3
"""Generate the Android launcher icon for the APK.

Until this existed the APK shipped with no `android:icon` at all, so the
launcher drew the system's generic green robot for 2-Top. Like the web
icons (`scripts/generate_web_icons.py`), everything here is built from the
game's own duelist sheet, bone-fang sprite and the locked 16-color palette,
so the icon can never drift off-palette or out of date with the sprites.

Two flavors, because Android has two icon systems:

* `mipmap-anydpi-v26/ic_launcher.xml` — an ADAPTIVE icon (Android 8+,
  i.e. every phone that realistically installs this). The launcher masks
  it to its own shape (circle on a Pixel, squircle elsewhere), so it is two
  layers: a flat palette background and a foreground whose art stays inside
  the 66/108 safe zone that survives every mask.
* `mipmap-<density>/ic_launcher.png` — the legacy square icon for Android
  7.x (minSdk 24), reusing the web icon's bordered-box composition.

Output lands in `crates/app/res/`, which `[package.metadata.android]
resources` hands to aapt.

Run: python3 scripts/generate_android_icons.py
"""

from pathlib import Path

from PIL import Image

from generate_web_icons import DEEP_ASH, build as build_square, idle_side

ROOT = Path(__file__).resolve().parent.parent
RES = ROOT / "crates" / "app" / "res"
FANG = ROOT / "assets/sprites/projectiles/bone_fang.png"

# Launcher densities: legacy icons are 48 dp, adaptive layers are 108 dp.
DENSITIES = {
    "mdpi": 1.0,
    "hdpi": 1.5,
    "xhdpi": 2.0,
    "xxhdpi": 3.0,
    "xxxhdpi": 4.0,
}

# The adaptive foreground's guaranteed-visible circle is 66 dp of 108; keep
# the art a little inside it so no launcher's mask ever nicks a duelist.
SAFE = 60 / 108


def scaled(sprite: Image.Image, height: int) -> Image.Image:
    w = max(1, round(sprite.width * height / sprite.height))
    return sprite.resize((w, height), Image.NEAREST)


def foreground(size: int) -> Image.Image:
    """The duel, squared up, with a bone fang spinning between them."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    safe = size * SAFE

    a = idle_side("duelist_a_sheet.png")
    b = idle_side("duelist_b_sheet.png").transpose(Image.FLIP_LEFT_RIGHT)
    duelist_h = round(safe * 0.78)
    a, b = scaled(a, duelist_h), scaled(b, duelist_h)

    gap = round(size * 0.02)
    total = a.width + b.width + gap
    x = (size - total) // 2
    # Sit the pair a touch low so the fang reads above their heads.
    y = (size - duelist_h) // 2 + round(safe * 0.10)
    img.alpha_composite(a, (x, y))
    img.alpha_composite(b, (x + a.width + gap, y))

    fang = Image.open(FANG).convert("RGBA")
    fang = fang.crop(fang.getbbox())
    fang = scaled(fang, round(safe * 0.26))
    fx = (size - fang.width) // 2
    fy = y - round(fang.height * 0.55)
    img.alpha_composite(fang, (fx, fy))
    return img


def write(path: Path, image: Image.Image) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, optimize=True)
    print(f"wrote {path.relative_to(ROOT)} ({image.width}x{image.height})")


def main() -> int:
    for density, scale in DENSITIES.items():
        mipmap = RES / f"mipmap-{density}"
        write(mipmap / "ic_launcher.png", build_square(round(48 * scale)))
        write(mipmap / "ic_launcher_foreground.png", foreground(round(108 * scale)))

    r, g, b, _ = DEEP_ASH
    colors = RES / "values" / "ic_launcher_colors.xml"
    colors.parent.mkdir(parents=True, exist_ok=True)
    colors.write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        "<resources>\n"
        f'    <color name="ic_launcher_background">#{r:02X}{g:02X}{b:02X}</color>\n'
        "</resources>\n"
    )
    print(f"wrote {colors.relative_to(ROOT)}")

    adaptive = RES / "mipmap-anydpi-v26" / "ic_launcher.xml"
    adaptive.parent.mkdir(parents=True, exist_ok=True)
    adaptive.write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
        '    <background android:drawable="@color/ic_launcher_background" />\n'
        '    <foreground android:drawable="@mipmap/ic_launcher_foreground" />\n'
        "</adaptive-icon>\n"
    )
    print(f"wrote {adaptive.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
