#!/usr/bin/env python3
"""Generate visual target boards for 2-Top.

These boards are art-direction targets, not shippable sprites. They are
meant to compare palette balance, silhouette language, arena density,
projectile priority, and HUD/touch readability before final art is made.
"""

from __future__ import annotations

import os
import struct
import zlib


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = "assets/concepts/target_pack"

TRANSPARENT = (0, 0, 0, 0)


def rgba(hex_color: str) -> tuple[int, int, int, int]:
    hex_color = hex_color.removeprefix("#")
    return (
        int(hex_color[0:2], 16),
        int(hex_color[2:4], 16),
        int(hex_color[4:6], 16),
        255,
    )


class Canvas:
    def __init__(self, width: int, height: int, fill: tuple[int, int, int, int]) -> None:
        self.width = width
        self.height = height
        self.pixels = [fill for _ in range(width * height)]

    def set(self, x: int, y: int, color: tuple[int, int, int, int]) -> None:
        if 0 <= x < self.width and 0 <= y < self.height:
            self.pixels[y * self.width + x] = color

    def rect(self, x: int, y: int, w: int, h: int, color: tuple[int, int, int, int]) -> None:
        for yy in range(y, y + h):
            for xx in range(x, x + w):
                self.set(xx, yy, color)

    def frame(self, x: int, y: int, w: int, h: int, color: tuple[int, int, int, int]) -> None:
        self.rect(x, y, w, 2, color)
        self.rect(x, y + h - 2, w, 2, color)
        self.rect(x, y, 2, h, color)
        self.rect(x + w - 2, y, 2, h, color)

    def line(self, x0: int, y0: int, x1: int, y1: int, color: tuple[int, int, int, int]) -> None:
        dx = abs(x1 - x0)
        sx = 1 if x0 < x1 else -1
        dy = -abs(y1 - y0)
        sy = 1 if y0 < y1 else -1
        err = dx + dy
        while True:
            self.set(x0, y0, color)
            if x0 == x1 and y0 == y1:
                break
            e2 = 2 * err
            if e2 >= dy:
                err += dy
                x0 += sx
            if e2 <= dx:
                err += dx
                y0 += sy

    def blit(self, src: "Canvas", dx: int, dy: int, scale: int = 1) -> None:
        for y in range(src.height):
            for x in range(src.width):
                color = src.pixels[y * src.width + x]
                if color[3] == 0:
                    continue
                for sy in range(scale):
                    for sx in range(scale):
                        self.set(dx + x * scale + sx, dy + y * scale + sy, color)

    def crop(self, x: int, y: int, w: int, h: int) -> "Canvas":
        out = Canvas(w, h, TRANSPARENT)
        for yy in range(h):
            for xx in range(w):
                if 0 <= x + xx < self.width and 0 <= y + yy < self.height:
                    out.set(xx, yy, self.pixels[(y + yy) * self.width + (x + xx)])
        return out


def write_png(canvas: Canvas, rel_path: str) -> None:
    path = os.path.join(ROOT, rel_path)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    raw = bytearray()
    for y in range(canvas.height):
        raw.append(0)
        for x in range(canvas.width):
            raw.extend(canvas.pixels[y * canvas.width + x])

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", canvas.width, canvas.height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as fh:
        fh.write(png)


DIRECTIONS = {
    "bone_cathedral": {
        "bg": "#08090D",
        "floor": "#171922",
        "floor2": "#24242E",
        "line": "#34303D",
        "wall": "#0E0E14",
        "wall_hi": "#6E6556",
        "p0": "#D22F45",
        "p0_dark": "#6E1632",
        "p1": "#27C7D8",
        "p1_dark": "#0D6572",
        "bone": "#FFF1C2",
        "trail": "#FFD866",
        "recall": "#476CFF",
        "hit": "#F8F7E8",
    },
    "neon_coven": {
        "bg": "#07080F",
        "floor": "#101A24",
        "floor2": "#192B34",
        "line": "#263E4D",
        "wall": "#0B0F18",
        "wall_hi": "#7B4BE8",
        "p0": "#FF3B6E",
        "p0_dark": "#8F1741",
        "p1": "#21E6D4",
        "p1_dark": "#0C6F73",
        "bone": "#FFE7A3",
        "trail": "#FFB000",
        "recall": "#7B4BE8",
        "hit": "#F9F7FF",
    },
    "ash_chapel": {
        "bg": "#0C0A08",
        "floor": "#211B18",
        "floor2": "#2C241F",
        "line": "#4A3E35",
        "wall": "#130F0E",
        "wall_hi": "#B69A73",
        "p0": "#C23B30",
        "p0_dark": "#651D1A",
        "p1": "#62B9B0",
        "p1_dark": "#276560",
        "bone": "#F4D99A",
        "trail": "#F0A84C",
        "recall": "#7AA6D9",
        "hit": "#FFF6D8",
    },
    "bloodglass_pit": {
        "bg": "#090B13",
        "floor": "#12172A",
        "floor2": "#1A2240",
        "line": "#34415D",
        "wall": "#0B0D16",
        "wall_hi": "#A61F4B",
        "p0": "#F04452",
        "p0_dark": "#7A172E",
        "p1": "#36D7FF",
        "p1_dark": "#166278",
        "bone": "#F7F1D4",
        "trail": "#FFCE50",
        "recall": "#66A1FF",
        "hit": "#FFFFFF",
    },
}


def palette(direction: dict[str, str]) -> dict[str, tuple[int, int, int, int]]:
    return {key: rgba(value) for key, value in direction.items()}


def draw_arena(c: Canvas, p: dict[str, tuple[int, int, int, int]], x: int, y: int) -> None:
    c.rect(x, y, 260, 390, p["wall"])
    c.rect(x + 10, y + 10, 240, 370, p["floor"])
    for yy in range(y + 16, y + 376, 24):
        for xx in range(x + 16, x + 246, 24):
            if ((xx + yy) // 24) % 2 == 0:
                c.rect(xx, yy, 20, 20, p["floor2"])
            c.set(xx, yy, p["line"])
            c.set(xx + 19, yy + 19, p["line"])
    for i in range(16):
        c.rect(x + 8 + i * 16, y + 4, 8, 4, p["wall_hi"])
        c.rect(x + 8 + i * 16, y + 386, 8, 4, p["wall_hi"])
    for i in range(23):
        c.rect(x + 4, y + 12 + i * 16, 4, 8, p["wall_hi"])
        c.rect(x + 252, y + 12 + i * 16, 4, 8, p["wall_hi"])
    c.line(x + 130, y + 40, x + 130, y + 350, p["line"])
    for r in [36, 37, 38]:
        draw_diamond(c, x + 130, y + 195, r, p["line"])
    c.rect(x + 76, y + 185, 16, 16, p["p0_dark"])
    c.rect(x + 168, y + 185, 16, 16, p["p1_dark"])


# Persistent floor stains. Deterministic pseudo-random splat — each entry
# is (cx, cy, side, intensity) where intensity scales the splat radius.
# Positions are chosen so kills cluster near spawn marks and the duel
# diamond (the high-traffic kill zones).
ARENA_STAINS = [
    (44, 76, "p0", 4),
    (192, 96, "p1", 5),
    (106, 136, "p0", 3),
    (218, 172, "p1", 4),
    (62, 222, "p0", 6),  # heavy kill near P0 spawn
    (158, 248, "p1", 3),
    (196, 306, "p0", 4),
    (76, 314, "p1", 5),  # heavy kill near P1 spawn
    (134, 354, "p0", 3),
]

# Hand-placed splat patterns, ordered tightest-to-loosest. The core is
# a chunky 3-row cluster; mid/edge add the spray geometry. Designed to
# read as "blood splat" not "Perlin smear" at native pixel scale.
SPLAT_CORE = [
    (0, 0), (1, 0), (-1, 0), (2, 0),
    (0, 1), (1, 1), (-1, 1), (2, 1),
    (0, -1), (1, -1),
]
SPLAT_MID = [
    (3, 1), (-2, 1), (3, 0), (-2, -1),
    (1, 2), (0, 2), (2, 2), (-1, 2),
    (1, -2), (0, -2),
]
SPLAT_EDGE = [
    (4, 1), (-3, 0), (3, 2), (-3, 2), (4, 0),
    (5, 1), (-4, 1), (2, 3), (1, 3), (-2, 3),
    (4, -1), (-3, -1), (5, 2),
]
SPLAT_FLECK = [(6, 1), (-5, 1), (3, -3), (-2, 4), (7, 0), (0, -3), (-4, 3)]


def draw_floor_stain(
    c: Canvas,
    p: dict[str, tuple[int, int, int, int]],
    cx: int,
    cy: int,
    side: str,
    intensity: int,
) -> None:
    dark = p["p0_dark"] if side == "p0" else p["p1_dark"]
    blood = p["p0"] if side == "p0" else p["p1"]
    # Core — always present. The dried center of the stain.
    for dx, dy in SPLAT_CORE:
        c.set(cx + dx, cy + dy, dark)
    # A single saturated drop in the middle so the stain reads as "blood",
    # not as a rust patch.
    c.set(cx, cy, blood)
    # Mid ring grows with intensity.
    if intensity >= 2:
        for dx, dy in SPLAT_MID[: 4 + intensity]:
            c.set(cx + dx, cy + dy, dark)
    # Edge spray.
    if intensity >= 3:
        for dx, dy in SPLAT_EDGE[: intensity + 3]:
            c.set(cx + dx, cy + dy, dark)
    # Outer flecks for the heaviest kills.
    if intensity >= 5:
        for dx, dy in SPLAT_FLECK[: intensity - 2]:
            c.set(cx + dx, cy + dy, dark)
    # Heavy kills get a second saturated drop offset from the core.
    if intensity >= 4:
        c.set(cx + 2, cy + 1, blood)
    if intensity >= 6:
        c.set(cx - 1, cy + 2, blood)


def draw_arena_marked(c: Canvas, p: dict[str, tuple[int, int, int, int]], x: int, y: int) -> None:
    """Arena that has hosted prior kills. The cathedral remembers."""
    draw_arena(c, p, x, y)
    for sx, sy, side, intensity in ARENA_STAINS:
        draw_floor_stain(c, p, x + sx, y + sy, side, intensity)


def draw_diamond(c: Canvas, cx: int, cy: int, r: int, color: tuple[int, int, int, int]) -> None:
    c.line(cx, cy - r, cx + r, cy, color)
    c.line(cx + r, cy, cx, cy + r, color)
    c.line(cx, cy + r, cx - r, cy, color)
    c.line(cx - r, cy, cx, cy - r, color)


def draw_player(
    c: Canvas,
    p: dict[str, tuple[int, int, int, int]],
    x: int,
    y: int,
    side: str,
    pose: str,
    scale: int = 4,
) -> None:
    local = Canvas(24, 24, TRANSPARENT)
    main = p["p0"] if side == "p0" else p["p1"]
    dark = p["p0_dark"] if side == "p0" else p["p1_dark"]
    line = p["bg"]
    bone = p["bone"]
    accent = p["trail"] if side == "p0" else p["recall"]

    lean = 3 if pose == "throw" else 0
    dash = pose == "dash"
    if dash:
        for i in range(5):
            local.rect(2 + i * 2, 12 + i % 2, 7, 2, accent)

    if pose == "death":
        for i in range(12):
            local.rect(4 + (i * 5) % 16, 5 + (i * 7) % 15, 2, 2, main if i % 2 else accent)
        c.blit(local, x, y, scale)
        return

    cx = 12 + (2 if dash else 0)
    local.rect(cx - 6, 8, 12, 11, line)
    local.rect(cx - 5, 9, 10, 9, dark)
    local.rect(cx - 4, 9, 8, 8, main)
    local.rect(cx - 5, 4, 10, 7, line)
    local.rect(cx - 4, 5, 8, 5, main)
    local.set(cx - 2, 6, bone)
    local.set(cx + 2, 6, bone)
    if side == "p0":
        local.line(cx - 4, 5, cx - 8, 1, bone)
        local.line(cx + 4, 5, cx + 8, 1, bone)
    else:
        local.line(cx - 4, 5, cx - 6, 1, bone)
        local.line(cx - 6, 1, cx - 9, 3, bone)
        local.line(cx + 4, 5, cx + 6, 1, bone)
        local.line(cx + 6, 1, cx + 9, 3, bone)
    if pose == "throw":
        local.line(cx + 5, 11, cx + 9 + lean, 8, bone)
        local.set(cx + 11 + lean, 7, accent)
    else:
        local.line(cx + 5, 12, cx + 8, 15, bone)
        local.line(cx - 5, 12, cx - 8, 15, bone)
    local.line(cx - 3, 18, cx - 5, 22, line)
    local.line(cx + 3, 18, cx + 5, 22, line)
    if pose == "hit":
        local.rect(cx - 7, 7, 14, 12, p["hit"])
        local.rect(cx - 4, 9, 8, 8, main)
    c.blit(local, x, y, scale)


def draw_player_attitude(
    c: Canvas,
    p: dict[str, tuple[int, int, int, int]],
    x: int,
    y: int,
    side: str,
    pose: str,
    scale: int = 4,
) -> None:
    """Attitude poses: forward-lean throw, weight-back taunt, kill stomp.

    Same silhouette grammar as draw_player but with exaggerated body
    geometry to show the gore-revival posture pole. Doom/Duke posture
    inside HLD-disciplined silhouette."""
    local = Canvas(28, 28, TRANSPARENT)
    main = p["p0"] if side == "p0" else p["p1"]
    dark = p["p0_dark"] if side == "p0" else p["p1_dark"]
    line = p["bg"]
    bone = p["bone"]
    accent = p["trail"] if side == "p0" else p["recall"]

    cx = 14
    if pose == "lean_throw":
        # Forward-leaning throw: torso pitched ~30deg into the throw,
        # weight thrown onto the lead foot, rear leg trailing. Body
        # geometry is asymmetric on purpose — Duke posture, not idle.
        # Torso (skewed forward).
        local.rect(cx - 4, 10, 11, 9, line)
        local.rect(cx - 3, 11, 9, 7, dark)
        local.rect(cx - 2, 11, 7, 6, main)
        # Head pitched forward — silhouette leads from the brow.
        local.rect(cx - 2, 5, 9, 6, line)
        local.rect(cx - 1, 6, 7, 4, main)
        local.set(cx + 1, 7, bone)
        local.set(cx + 4, 7, bone)
        if side == "p0":
            local.line(cx - 1, 6, cx - 5, 2, bone)
            local.line(cx + 5, 6, cx + 9, 2, bone)
        else:
            local.line(cx - 1, 6, cx - 3, 2, bone)
            local.line(cx - 3, 2, cx - 6, 4, bone)
            local.line(cx + 5, 6, cx + 7, 2, bone)
            local.line(cx + 7, 2, cx + 10, 4, bone)
        # Throwing arm thrust forward past the body — fang already
        # leaving the hand.
        local.line(cx + 6, 12, cx + 13, 9, bone)
        local.line(cx + 13, 9, cx + 16, 7, bone)
        local.set(cx + 17, 6, accent)
        local.set(cx + 18, 5, p["hit"])
        # A short arc-streak at the release point.
        local.set(cx + 14, 7, accent)
        local.set(cx + 12, 8, accent)
        # Lead foot planted hard (a chunky weight pixel), rear leg
        # streaming behind.
        local.rect(cx + 4, 18, 4, 5, line)
        local.set(cx + 5, 22, dark)
        local.line(cx - 3, 18, cx - 7, 23, line)
        local.set(cx - 8, 22, line)
    elif pose == "kill_frame":
        # The proud carnage moment: hit-white halo + body remnant +
        # radial blood spray. Three-frame freeze in-engine; here we
        # render the apex frame.
        local.rect(cx - 6, 4, 13, 17, p["hit"])
        local.rect(cx - 4, 7, 9, 13, dark)
        local.rect(cx - 3, 8, 7, 11, main)
        local.set(cx - 1, 10, bone)
        local.set(cx + 2, 10, bone)
        # Radial blood spray — chunky pixels, not feathered.
        for dx, dy in [(-5, -2), (-6, 1), (-7, 4), (6, -1), (7, 2), (8, 5),
                        (-3, -5), (3, -5), (-4, 11), (4, 11), (-2, 13), (2, 13)]:
            local.set(cx + dx, 12 + dy, dark)
        for dx, dy in [(-6, 6), (6, 6), (0, -6)]:
            local.set(cx + dx, 12 + dy, main)
        # Bone shards — the cathedral pays its tithe.
        local.set(cx - 5, 0, bone)
        local.set(cx + 5, 1, bone)
        local.set(cx - 8, 8, bone)
    elif pose == "taunt":
        # Weight back, shoulders open, fang held low and ready. The Duke
        # posture: confident, not idle.
        local.rect(cx - 6, 8, 13, 11, line)
        local.rect(cx - 5, 9, 11, 9, dark)
        local.rect(cx - 4, 9, 9, 8, main)
        local.rect(cx - 5, 4, 11, 7, line)
        local.rect(cx - 4, 5, 9, 5, main)
        local.set(cx - 2, 6, bone)
        local.set(cx + 2, 6, bone)
        if side == "p0":
            local.line(cx - 4, 5, cx - 8, 1, bone)
            local.line(cx + 4, 5, cx + 8, 1, bone)
        else:
            local.line(cx - 4, 5, cx - 6, 1, bone)
            local.line(cx - 6, 1, cx - 9, 3, bone)
            local.line(cx + 4, 5, cx + 6, 1, bone)
            local.line(cx + 6, 1, cx + 9, 3, bone)
        # Fang held low, ready position.
        local.line(cx - 6, 14, cx - 9, 17, bone)
        local.set(cx - 10, 18, bone)
        # Stance wider than idle.
        local.line(cx - 4, 18, cx - 7, 23, line)
        local.line(cx + 4, 18, cx + 7, 23, line)
    c.blit(local, x, y, scale)


def draw_corpse_mark(
    c: Canvas,
    p: dict[str, tuple[int, int, int, int]],
    x: int,
    y: int,
    side: str,
    scale: int = 4,
) -> None:
    """The aftermath: a body-shaped stain that persists till round reset."""
    local = Canvas(28, 28, TRANSPARENT)
    dark = p["p0_dark"] if side == "p0" else p["p1_dark"]
    blood = p["p0"] if side == "p0" else p["p1"]
    bone = p["bone"]
    # Body outline as floor stain.
    for dx, dy in [(-4, 11), (-3, 11), (-2, 12), (-1, 13), (0, 13), (1, 13),
                    (2, 12), (3, 11), (4, 11), (-5, 13), (5, 13),
                    (-3, 14), (-2, 14), (2, 14), (3, 14), (-4, 15), (4, 15)]:
        local.set(14 + dx, dy, dark)
    # A single saturated drop — the wet center.
    local.set(14, 12, blood)
    local.set(15, 13, blood)
    # Splatter radiating outward from the body.
    for dx, dy in [(-7, 9), (-8, 12), (-6, 16), (7, 10), (9, 13), (6, 17),
                    (-9, 14), (10, 15), (0, 17), (-2, 18), (2, 18)]:
        local.set(14 + dx, dy, dark)
    # A bone fragment left behind.
    local.set(14, 8, bone)
    local.set(15, 8, bone)
    c.blit(local, x, y, scale)


def draw_marked_boomerang(
    c: Canvas,
    p: dict[str, tuple[int, int, int, int]],
    x: int,
    y: int,
    marks: int,
    scale: int = 4,
) -> None:
    """Bone-fang projectile with carved sigil, accumulating blood marks
    per kill landed this round. Cosmetic only — never feeds back to sim.

    Source sprite is 14x6 (matches the in-game boomerang scale roughly).
    `marks` ∈ {0, 1, 2, 3+} controls how much blood it has tasted."""
    local = Canvas(14, 6, TRANSPARENT)
    bone = p["bone"]
    bone_dim = p["wall_hi"]
    blood = p["p0_dark"]
    blood_p1 = p["p1_dark"]
    line = p["bg"]
    # Asymmetric fang body — leading edge wider, tapered tail.
    local.rect(0, 1, 12, 4, bone_dim)
    local.rect(1, 1, 10, 3, bone)
    local.rect(2, 1, 6, 1, p["hit"])  # gleam along the spine
    local.set(11, 1, bone_dim)
    local.set(12, 2, bone_dim)
    local.set(13, 2, line)
    # Outline — the silhouette is what reads at 12px.
    local.line(0, 0, 11, 0, line)
    local.line(0, 4, 11, 4, line)
    local.set(0, 1, line)
    local.set(0, 3, line)
    # Carved sigil — a tiny X near the throwing edge.
    local.set(4, 2, line)
    local.set(5, 3, line)
    local.set(5, 2, bone)
    local.set(4, 3, bone)
    # Accumulated blood marks. P0 marks (warm) and P1 marks (cool) alternate
    # so the boomerang shows "the round had two-sided violence".
    if marks >= 1:
        local.set(2, 2, blood)
        local.set(3, 1, blood)
    if marks >= 2:
        local.set(7, 3, blood_p1)
        local.set(8, 2, blood_p1)
        local.set(9, 3, blood_p1)
    if marks >= 3:
        local.set(6, 1, blood)
        local.set(10, 2, blood)
        local.set(8, 1, blood_p1)
    c.blit(local, x, y, scale)


def draw_projectile_suite(c: Canvas, p: dict[str, tuple[int, int, int, int]], x: int, y: int) -> None:
    # Projectile
    for off in range(0, 36, 8):
        c.rect(x + off, y + 20 - off // 4, 12, 8, p["bone"])
        c.rect(x + off + 8, y + 24 - off // 4, 8, 6, p["wall_hi"])
        c.rect(x + off + 2, y + 21 - off // 4, 5, 3, p["hit"])
    # Trails
    for i in range(8):
        c.rect(x + 80 + i * 12, y + 18 + i % 2 * 4, 8, 8, p["trail"])
        if i % 2 == 0:
            c.rect(x + 84 + i * 12, y + 30, 5, 5, p["recall"])
    # Hit bursts
    for r in [10, 18, 28]:
        cx = x + 230 + r
        cy = y + 32
        for dx, dy in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (-1, 1)]:
            c.line(cx, cy, cx + dx * r, cy + dy * r, p["hit"])


def draw_hud(c: Canvas, p: dict[str, tuple[int, int, int, int]], x: int, y: int) -> None:
    for i in range(5):
        c.rect(x + i * 18, y, 12, 8, p["bg"])
        c.rect(x + i * 18 + 2, y + 2, 8, 4, p["p0"])
        c.rect(x + i * 18, y + 18, 12, 8, p["bg"])
        c.rect(x + i * 18 + 2, y + 20, 8, 4, p["p1"])
    for i, v in enumerate([3, 2, 1]):
        draw_digit(c, x + 125 + i * 38, y - 8, v, p["hit"], p["trail"])
    c.frame(x + 270, y - 10, 96, 54, p["wall_hi"])
    c.rect(x + 282, y + 8, 24, 24, p["line"])
    c.frame(x + 318, y + 8, 32, 24, p["hit"])


DIGITS = {
    1: ["010", "110", "010", "010", "111"],
    2: ["111", "001", "111", "100", "111"],
    3: ["111", "001", "111", "001", "111"],
}


def draw_digit(
    c: Canvas,
    x: int,
    y: int,
    digit: int,
    color: tuple[int, int, int, int],
    accent: tuple[int, int, int, int],
) -> None:
    for yy, row in enumerate(DIGITS[digit]):
        for xx, bit in enumerate(row):
            if bit == "1":
                c.rect(x + xx * 8, y + yy * 8, 6, 6, color)
                c.set(x + xx * 8 + 5, y + yy * 8 + 5, accent)


def palette_strip(c: Canvas, p: dict[str, tuple[int, int, int, int]], x: int, y: int) -> None:
    keys = ["bg", "floor", "floor2", "line", "wall", "wall_hi", "p0", "p0_dark",
            "p1", "p1_dark", "bone", "trail", "recall", "hit"]
    for i, key in enumerate(keys):
        c.rect(x + i * 28, y, 24, 24, p[key])


def board(slug: str) -> Canvas:
    p = palette(DIRECTIONS[slug])
    c = Canvas(720, 980, p["bg"])
    c.rect(20, 20, 680, 940, p["floor"])
    draw_arena(c, p, 46, 60)
    draw_player(c, p, 372, 92, "p0", "idle")
    draw_player(c, p, 510, 92, "p1", "idle")
    draw_player(c, p, 372, 226, "p0", "throw")
    draw_player(c, p, 510, 226, "p1", "dash")
    draw_player(c, p, 372, 360, "p0", "hit")
    draw_player(c, p, 510, 360, "p1", "death")
    draw_projectile_suite(c, p, 70, 510)
    draw_hud(c, p, 70, 665)
    palette_strip(c, p, 70, 820)
    c.frame(20, 20, 680, 940, p["wall_hi"])
    return c


def synthesis_showcase() -> Canvas:
    """The chosen direction in *contact*: HLD composes the room, gore-revival
    cleans it. Bone Cathedral with the violence remembered.

    The other four direction boards show palettes at rest. This board
    shows the synthesis in motion — persistent floor stains from prior
    kills, attitude in the silhouette, the kill-frame freeze, and the
    boomerang accumulating blood marks across rounds."""
    p = palette(DIRECTIONS["bone_cathedral"])
    c = Canvas(720, 980, p["bg"])
    c.rect(20, 20, 680, 940, p["floor"])
    # Arena: same composition discipline, but stained with the round so far.
    draw_arena_marked(c, p, 46, 60)
    # Pose grid — the two modes side by side.
    # Left column = composition mode (HLD-quiet).
    # Right column = contact mode (gore-revival proud).
    # Vertical divider tracks the seam between the two modes.
    for yy in range(70, 470, 3):
        c.set(488, yy, p["wall_hi"])
    # Mode tabs above each column — small chunky bars.
    c.rect(380, 64, 96, 4, p["wall_hi"])
    c.rect(496, 64, 96, 4, p["p0"])
    draw_player(c, p, 372, 80, "p0", "idle")
    draw_player_attitude(c, p, 504, 76, "p0", "lean_throw")
    draw_player_attitude(c, p, 372, 214, "p1", "taunt")
    draw_player_attitude(c, p, 504, 210, "p1", "kill_frame")
    draw_player(c, p, 372, 348, "p0", "death")
    draw_corpse_mark(c, p, 504, 348, "p0")
    # Boomerang accumulation row — clean / round 1 / round 2 / round 3.
    # Each boomerang is 56x24 at scale=4. Spacing ~150px gives clear
    # separation while the labels stay implicit (kill count = mark count).
    draw_marked_boomerang(c, p, 80, 502, 0)
    draw_marked_boomerang(c, p, 230, 502, 1)
    draw_marked_boomerang(c, p, 380, 502, 2)
    draw_marked_boomerang(c, p, 530, 502, 3)
    # Tiny tally beneath each boomerang — chunky pip count.
    for i, count in enumerate([0, 1, 2, 3]):
        x0 = 80 + i * 150
        for k in range(3):
            color = p["p0"] if k < count else p["bg"]
            c.rect(x0 + 8 + k * 14, 540, 8, 4, color)
            if k >= count:
                c.frame(x0 + 8 + k * 14, 540, 8, 4, p["line"])
    # Trail + hit-burst suite (carries forward from the rest board).
    draw_projectile_suite(c, p, 70, 580)
    # HUD — same instrument-panel rules.
    draw_hud(c, p, 70, 685)
    # Palette strip — unchanged (synthesis lives in the rules, not the colors).
    palette_strip(c, p, 70, 820)
    c.frame(20, 20, 680, 940, p["wall_hi"])
    return c


def overview(boards: list[tuple[str, Canvas]]) -> Canvas:
    c = Canvas(1480, 2040, rgba("#06070A"))
    positions = [(20, 20), (750, 20), (20, 1030), (750, 1030)]
    for (slug, b), (x, y) in zip(boards, positions):
        c.blit(b, x, y, 1)
        accent = palette(DIRECTIONS[slug])["wall_hi"]
        c.frame(x, y, b.width, b.height, accent)
    return c


def main() -> None:
    generated: list[tuple[str, Canvas]] = []
    for slug in DIRECTIONS:
        b = board(slug)
        generated.append((slug, b))
        path = f"{OUT_DIR}/{slug}.png"
        write_png(b, path)
        print(path)
    write_png(overview(generated), f"{OUT_DIR}/visual_target_overview.png")
    print(f"{OUT_DIR}/visual_target_overview.png")
    showcase = synthesis_showcase()
    showcase_path = f"{OUT_DIR}/synthesis_showcase.png"
    write_png(showcase, showcase_path)
    print(showcase_path)


if __name__ == "__main__":
    main()
