#!/usr/bin/env python3
"""Generate polished Phase 15 pixel art for 2-Top.

Hand-designed sprites embodying the Bone Cathedral, blood-marked synthesis:
HLD discipline composes the silhouette; gore-revival fills the kill frame.
The arena remembers each kill until the round resets.

All art is defined as ASCII pixel grids that map characters to the
16-color palette in `assets/palettes/two_top_16.gpl`. Each glyph is one
pixel at source scale (24x24 for players, 12x12 for boomerang, etc).

Output paths overwrite the placeholders that `generate_placeholder_art.py`
used to write — same atlas layout, polished art.
"""

from __future__ import annotations

import math
import os
import struct
import zlib
from dataclasses import dataclass


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


# ---------------------------------------------------------------------------
# Palette — locked 16 colors from assets/palettes/two_top_16.gpl, plus a
# transparent slot. Keep hex values in sync with the .gpl.
# ---------------------------------------------------------------------------

# HLD-register evolution (2026-06-30): the five dead grey neutrals
# (void/deep_ash/bruise/charcoal/cold_stone) are re-cast as SATURATED indigo →
# violet → periwinkle darks, and the warm/cool accents punched up, so the world
# reads vibrant + lit like Hyper Light Drifter instead of grimdark grey. The
# character ramps keep their identity (Cur warm crimson, Stag cool cyan); only
# the hue/saturation of the darks changed, so every sprite re-tints in place.
PALETTE: dict[str, tuple[int, int, int, int]] = {
    "clear":           (0, 0, 0, 0),
    "void":            (16, 14, 34, 255),     # deep indigo-black
    "deep_ash":        (32, 28, 66, 255),     # dark indigo (the base dark)
    "bruise_shadow":   (66, 36, 92, 255),     # rich dark violet
    "charcoal_line":   (94, 64, 132, 255),    # mid violet
    "cold_stone":      (104, 126, 168, 255),  # dusty periwinkle-blue
    "warm_bone_shade": (130, 96, 102, 255),   # muted rose-brown
    "bone":            (210, 196, 156, 255),  # bone
    "hot_bone":        (255, 243, 202, 255),  # pale gold-white
    "blood_dark":      (122, 28, 66, 255),    # deep crimson-magenta
    "p0_blood":        (226, 52, 84, 255),    # vivid Cur red
    "ember":           (245, 112, 60, 255),   # ember orange
    "spark":           (255, 220, 112, 255),  # spark gold
    "deep_teal":       (16, 118, 132, 255),   # teal
    "p1_cyan":         (52, 212, 226, 255),   # vivid Stag cyan
    "recall_blue":     (86, 120, 255, 255),   # recall blue
    "hit_white":       (250, 248, 240, 255),  # near-white
}


# ---------------------------------------------------------------------------
# Canvas + PNG writer (no external libs).
# ---------------------------------------------------------------------------


@dataclass
class Canvas:
    width: int
    height: int
    fill: tuple[int, int, int, int] = PALETTE["clear"]

    def __post_init__(self) -> None:
        self.pixels = [self.fill for _ in range(self.width * self.height)]

    def set(self, x: int, y: int, color: tuple[int, int, int, int]) -> None:
        if 0 <= x < self.width and 0 <= y < self.height:
            if color[3] == 0:
                return
            self.pixels[y * self.width + x] = color

    def force(self, x: int, y: int, color: tuple[int, int, int, int]) -> None:
        """Set a pixel even if the source color is transparent (clears it)."""
        if 0 <= x < self.width and 0 <= y < self.height:
            self.pixels[y * self.width + x] = color

    def rect(self, x: int, y: int, w: int, h: int, color: tuple[int, int, int, int]) -> None:
        for yy in range(y, y + h):
            for xx in range(x, x + w):
                self.set(xx, yy, color)

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
                        self.force(dx + x * scale + sx, dy + y * scale + sy, color)


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


# ---------------------------------------------------------------------------
# ASCII grid painter.
#
# Each character maps to a palette key. Spaces and dots are transparent. The
# grid renders top-down, left-to-right. `paint(canvas, ox, oy, art, side)`
# substitutes side-relative keys (M/m/e) so the same silhouette can render as
# P0 or P1 without duplicating geometry.
# ---------------------------------------------------------------------------

BASE_KEYS = {
    ".": "clear",
    " ": "clear",
    "k": "void",
    "l": "charcoal_line",
    "o": "bruise_shadow",
    "d": "deep_ash",
    "S": "cold_stone",
    "w": "warm_bone_shade",
    "b": "bone",
    "B": "hot_bone",
    "h": "hit_white",
    "y": "spark",
    # Side-neutral fallbacks (overridden by keys_for).
    "M": "p0_blood",
    "m": "blood_dark",
    "L": "ember",          # body LIGHT step (hue-shift warm); p1 -> p1_cyan
    "G": "ember",          # top-left corner GLEAM; p1 -> hit_white (crisp pop)
    "D": "bruise_shadow",  # body DEEP shadow (hue-shift cool), both sides
    "e": "ember",
    "C": "p1_cyan",
    "c": "deep_teal",
    "r": "recall_blue",
}


def keys_for(side: str) -> dict[str, tuple[int, int, int, int]]:
    """Return key->color map. M/m/e flip with side so generic poses can paint
    either character. C/c/r stay fixed (used when a pose explicitly references
    the *opposite* side, e.g. a P0 hit by P1 still shows P1's blue marks)."""
    keys = {char: PALETTE[name] for char, name in BASE_KEYS.items()}
    if side == "p1":
        keys["M"] = PALETTE["p1_cyan"]
        keys["m"] = PALETTE["deep_teal"]
        # The Stag is a cool character: the bright cyan IS its light, so the
        # LIGHT step holds at p1_cyan and the hue-shift lives in the cooling
        # shadows (deep_teal -> bruise_shadow) rather than a warm highlight.
        keys["L"] = PALETTE["p1_cyan"]
        keys["G"] = PALETTE["hit_white"]  # sparse cool gleam — the Stag's pop
        keys["e"] = PALETTE["recall_blue"]
    return keys


def paint(canvas: Canvas, ox: int, oy: int, art: str, side: str = "p0") -> None:
    keys = keys_for(side)
    rows = art.strip("\n").split("\n")
    for y, row in enumerate(rows):
        for x, char in enumerate(row):
            color = keys.get(char)
            if color is not None and color[3] > 0:
                canvas.set(ox + x, oy + y, color)


# ===========================================================================
# Player sprites (32x32 source) — ART_DIRECTION.md v2.
#
# "The Cur" (P0) / "The Stag" (P1) share the 41-frame animation skeleton but
# diverge in BODY build (see the Build profiles below): the Cur is a broad,
# low, forward brute; the Stag is a tall, narrow, upright herald. They read
# apart as solid-black silhouettes — body shape, not just crown/color — per
# audit finding D-VQ-01 and the v2 design panel. keys_for("p1") then recolors
# red -> cyan on top of the already-distinct Stag shape.
#
# The duelist is a broad, hunched demon-brute drawn procedurally from a
# parametric skeleton so the 41-frame set stays anatomically consistent and
# tunable. Form (pecs / abs / flank / cloak folds) is placed as explicit
# shadow; a thin auto rim-shade + auto outline finish the silhouette. Output
# is the same char-grid the rest of the generator paints, so keys_for() still
# swaps P0<->P1 colors.
#
# Grid chars (consumed by paint()):
#   k void outline   l charcoal inner-line   M body   m body-shadow
#   b bone  B hot-bone  w warm-bone-shade   y spark(eyes)  h hit-white
#   o bruise(ground)   e accent(ember/recall-blue)
# ===========================================================================

PLAYER_PX = 48
_CX = 24  # silhouette centre column


def _blank() -> list[list[str]]:
    return [["." for _ in range(PLAYER_PX)] for _ in range(PLAYER_PX)]


def _put(g: list[list[str]], x: int, y: int, ch: str) -> None:
    if 0 <= x < PLAYER_PX and 0 <= y < PLAYER_PX:
        g[y][x] = ch


def _span(g: list[list[str]], y: int, x0: int, x1: int, ch: str) -> None:
    if x0 > x1:
        x0, x1 = x1, x0
    for x in range(x0, x1 + 1):
        _put(g, x, y, ch)


def _grid_to_str(g: list[list[str]]) -> str:
    return "\n".join("".join(row) for row in g)


_BODY = {"M", "m", "D", "L", "G", "h"}
_BONE = {"b", "B", "w"}
_SOLID = _BODY | _BONE | {"l", "y", "e", "o"}

# Style presets (Pillar 4 — divergent-select). `_shade`/`_outline` read the
# module-global STYLE so the same geometry renders in different treatments; the
# operator picks one and it becomes the default. Knobs:
#   light:   flat | rim | gleam   (no body light / L lit rim / + G corner gleam)
#   shadow:  soft | deep          (m shadow rim / + D deep-shadow corners)
#   outline: void | shade         (k black outline / D bruise selout)
#   dither:  bool                 (ordered dither on interior faces)
_DEFAULT_STYLE = {"light": "gleam", "shadow": "deep", "outline": "void", "dither": False}
STYLE = dict(_DEFAULT_STYLE)


def _shade(g: list[list[str]]) -> None:
    """Directional 4-tone body shading + bone gleam, light committed top-left.
    Each mid-body (`M`) pixel on the lit (top/left) silhouette edge takes the
    LIGHT ramp step `L`; the shadowed (bottom/right) edge takes shadow `m`, and
    convex bottom-right corners the DEEP shadow `D`; the interior keeps `M`.
    keys_for() makes that a hue-shifted ramp (P0: bruise->blood-dark->blood->
    ember; P1: cool-shadow shift). This is committed-light-direction shading,
    not pillow shading. Hand-placed `m` form (pecs/abs/flank) is preserved
    because only `M` pixels are touched; eyes stay `y` (spark), the hottest
    pixel. Edges are detected against empty ('.'), so promotions never cascade."""

    def empty(x: int, y: int) -> bool:
        return not (0 <= x < PLAYER_PX and 0 <= y < PLAYER_PX) or g[y][x] == "."

    light, shadow, dither = STYLE["light"], STYLE["shadow"], STYLE["dither"]
    for y in range(PLAYER_PX):
        for x in range(PLAYER_PX):
            if g[y][x] != "M":
                continue
            top, left = empty(x, y - 1), empty(x - 1, y)
            bot, right = empty(x, y + 1), empty(x + 1, y)
            if light != "flat" and (top or left):
                g[y][x] = "G" if (light == "gleam" and top and left) else "L"
            elif bot or right:
                g[y][x] = "D" if (shadow == "deep" and bot and right) else "m"
            elif dither and (x + y) % 2 == 0:
                g[y][x] = "m"          # ordered dither on interior faces
    for y in range(PLAYER_PX):
        for x in range(PLAYER_PX):
            if g[y][x] == "b":
                up_left_open = (
                    y > 0 and x > 0 and g[y - 1][x] not in _BONE and g[y][x - 1] not in _BONE
                )
                down_right_solid = (
                    y + 1 < PLAYER_PX and x + 1 < PLAYER_PX
                    and (g[y + 1][x] in _BONE or g[y][x + 1] in _BONE)
                )
                if up_left_open:
                    g[y][x] = "B"
                elif not down_right_solid:
                    g[y][x] = "w"


def _outline(g: list[list[str]]) -> None:
    """Wrap the silhouette in a closed 1px void outline (gameplay readability
    is priority #1)."""
    adds = []
    for y in range(PLAYER_PX):
        for x in range(PLAYER_PX):
            if g[y][x] != ".":
                continue
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx, ny = x + dx, y + dy
                if 0 <= nx < PLAYER_PX and 0 <= ny < PLAYER_PX and g[ny][nx] in _SOLID:
                    adds.append((x, y))
                    break
    oc = "k" if STYLE["outline"] == "void" else "D"
    for x, y in adds:
        g[y][x] = oc


# --- cloaked-drifter rig ---------------------------------------------------
#
# Hyper Light Drifter move (locked 2026-06-22): the duelists are HOODED
# DRIFTERS, not faces. A hood + high collar hides the face entirely; the only
# feature is a single glowing eye-slit. This sidesteps the unsolved problem of
# a charming 48px face and buys a dramatic, readable silhouette for free. The
# flat `M` cloak + the existing `_shade` (committed top-left light -> L/G, deep
# bottom-right shadow -> m/D) reproduce HLD's bold blocking + bloom rim. Gore
# lives in the FLOOR (kill stains), never on the cloak — Duke crust without
# fighting HLD's elegance (docs/STYLE_BIBLE.md).
#
# The Cur (P0) is a BROAD round-hooded brute, warm ember eye-glow, bone
# pauldron (asymmetry). The Stag (P1) is a TALL pointed-hooded herald, cool
# cyan/white eye-glow, a long trailing scarf (asymmetry). They read apart as
# solid-black shapes before color registers (audit D-VQ-01).


@dataclass(frozen=True)
class Build:
    """Per-duelist cloak profile. One swap re-proportions all 41 frames and the
    two read apart as solid-black silhouettes (audit D-VQ-01 + v2 panel)."""

    hood: str       # "round" (broad dome, Cur) | "peak" (tall point, Stag)
    sh: int         # shoulder/mantle half-width (Cur broad, Stag narrow)
    body: int       # cloak half-width below the shoulders
    flare: int      # hem widening per 4 rows (Cur drapes out, Stag stays a column)
    cape: str       # "none" (Cur) | "scarf" (Stag's long trailing tail)
    pauldron: bool  # bone shoulder plate (the Cur's status + asymmetry)
    hot: str        # hottest eye pixel: "y" spark (Cur warm) | "h" hit-white (Stag cool)


CUR = Build(hood="round", sh=12, body=9, flare=1, cape="none", pauldron=True, hot="y")
STAG = Build(hood="peak", sh=6, body=6, flare=0, cape="scarf", pauldron=False, hot="h")


# --- body parts ------------------------------------------------------------

def _rect(g: list[list[str]], x: int, y: int, w: int, h: int, ch: str) -> None:
    for yy in range(y, y + h):
        _span(g, yy, x, x + w - 1, ch)


def _hood(g: list[list[str]], cx: int, dy: int, b: Build) -> None:
    """The hood: a solid cloak shape with a dark face cavity carved into it.
    `round` = broad dome (Cur); `peak` = tall forward-leaning point (Stag)."""
    if b.hood == "round":
        rows = [(6, 5), (7, 7), (8, 8), (9, 9), (10, 10), (11, 10),
                (12, 10), (13, 10), (14, 9), (15, 9), (16, 8)]
        for y, half in rows:
            _span(g, y + dy, cx - half, cx + half, "M")
        # carved face cavity (brim row 11 overhangs the top)
        for y, half in [(12, 5), (13, 6), (14, 6), (15, 5)]:
            _span(g, y + dy, cx - half, cx + half, "k")
    else:  # peak — tall narrow hood with a forward-flopping cloth tip
        rows = [(5, 3), (6, 4), (7, 4), (8, 5), (9, 5), (10, 5),
                (11, 6), (12, 6), (13, 6), (14, 6), (15, 5), (16, 5)]
        for y, half in rows:
            _span(g, y + dy, cx - half, cx + half, "M")
        # the tip flops forward-left — reads as heavy cloth, not a rigid cone
        for (x, y) in [(cx - 3, 4), (cx - 4, 4), (cx - 4, 3), (cx - 5, 3), (cx - 5, 2)]:
            _put(g, x, y + dy, "M")
        # carved face cavity (brim row 11 overhangs the top)
        for y, half in [(12, 3), (13, 4), (14, 4), (15, 3)]:
            _span(g, y + dy, cx - half, cx + half, "k")


def _eyes(g: list[list[str]], cx: int, dy: int, b: Build) -> None:
    """The single tell under the hood: a glowing eye-slit. Outer `e` (ember on
    the Cur, recall-blue on the Stag via keys_for) + a hot core (`y` spark /
    `h` hit-white). Always the hottest pixels on the body."""
    ox = 0
    _span(g, 14 + dy, cx + ox - 4, cx + ox - 2, "e")
    _span(g, 14 + dy, cx + ox + 2, cx + ox + 4, "e")
    _put(g, cx + ox - 3, 14 + dy, b.hot)
    _put(g, cx + ox + 3, 14 + dy, b.hot)


def _mantle(g: list[list[str]], cx: int, dy: int, b: Build,
            hunch: int, stride: str) -> None:
    """Shoulders flaring to the mantle, then the cloak draping to a torn hem,
    then boots peeking under it. Width per build; hem is planted (not bobbed)
    so breath/lean move the upper body over fixed feet."""
    sy = 17 + dy + hunch
    # shoulder mantle: widen to the full shoulder, then start the body
    _span(g, sy, cx - (b.sh - 4), cx + (b.sh - 4), "M")
    _span(g, sy + 1, cx - (b.sh - 1), cx + (b.sh - 1), "M")
    _span(g, sy + 2, cx - b.sh, cx + b.sh, "M")        # widest point
    _span(g, sy + 3, cx - (b.sh - 1), cx + (b.sh - 1), "M")
    # cloak body from below the shoulders down to a planted hem
    hem = 41
    top = sy + 4
    half_max = b.body
    for i, y in enumerate(range(top, hem + 1)):
        half = min(b.sh + 1, b.body + (i * b.flare) // 4)
        half_max = max(half_max, half)
        _span(g, y, cx - half, cx + half, "M")
    # central fold seam (vertical read) + a couple of fold shadows
    for y in range(top, hem):
        _put(g, cx, y, "m")
    _put(g, cx - min(4, b.body - 1), top + 4, "m")
    _put(g, cx + min(3, b.body - 2), top + 6, "m")
    # jagged torn hem (Duke crust)
    for x in range(cx - half_max, cx + half_max + 1):
        d = x - cx
        if d % 3 == 0:
            _put(g, x, hem + 1, "M")
        if d % 5 == 0:
            _put(g, x, hem + 2, "m")
    # boots peeking under the hem; stride shifts the weight foot
    sh_l = -1 if stride in ("strideL", "splay") else (1 if stride == "strideR" else 0)
    if stride != "air":
        _rect(g, cx - 6 + sh_l, hem + 1, 4, 3, "l")
        _rect(g, cx + 3 + sh_l, hem + 1, 4, 3, "l")


def _scarf(g: list[list[str]], cx: int, dy: int, sway: int) -> None:
    """The Stag's long trailing scarf, streaming off the left shoulder past the
    hem — an asymmetric silhouette tell that reads opposite the Cur's pauldron.
    `sway` streams it further out under motion (run/dash)."""
    sx = cx - 7
    for i, y in enumerate(range(18 + dy, 44)):
        off = i // 3 + sway
        _put(g, sx - off, y, "M")
        _put(g, sx - off - 1, y, "m")
        if i % 2 == 0:
            _put(g, sx - off + 1, y, "L")    # lit inner edge
    _put(g, sx - (8 + sway), 43, "m")
    _put(g, sx - (8 + sway), 44, "m")


def _pauldron(g: list[list[str]], cx: int, dy: int) -> None:
    """A clean angular bone plate over the left shoulder — asymmetry + status,
    not a blob. Gleam top-left, warm shade bottom-right (the _shade bone pass
    refines it further)."""
    sy = 18 + dy
    # a flatter plate capping the shoulder slope (wider than tall = armor, not a horn)
    _span(g, sy, cx - 12, cx - 9, "b")
    _span(g, sy + 1, cx - 13, cx - 9, "b")
    _span(g, sy + 2, cx - 12, cx - 10, "b")
    _put(g, cx - 12, sy, "B")        # gleam
    _put(g, cx - 13, sy + 1, "w")    # shade edge
    _put(g, cx - 10, sy + 2, "w")


def _sleeve(g: list[list[str]], cx: int, dy: int, b: Build, arm: str) -> None:
    """A cloaked sleeve emerging from the right of the body for the throw/catch
    gestures (the cloak hides the arm otherwise). pose: back|fwd|up|reach. Ends
    in a small pale hand."""
    sx = cx + (b.sh - 3)
    sy = 20 + dy
    if arm == "back":      # cocked up-and-back (throw anticipation)
        for i in range(5):
            _put(g, sx + i // 2, sy - i, "M")
            _put(g, sx + i // 2 + 1, sy - i, "M")
        _put(g, sx + 3, sy - 5, "h")
    elif arm == "fwd":     # thrust forward (release / follow-through)
        for i in range(8):
            _put(g, sx + i, sy + 1, "M")
            _put(g, sx + i, sy + 2, "M")
        _put(g, sx + 8, sy + 1, "h")
        _put(g, sx + 8, sy + 2, "L")
    elif arm == "up":      # raised to catch
        for i in range(8):
            _put(g, sx, sy - i, "M")
            _put(g, sx + 1, sy - i, "M")
        _put(g, sx, sy - 8, "h")
    elif arm == "reach":   # lowering after the catch
        for i in range(5):
            _put(g, sx + i, sy - i, "M")
            _put(g, sx + i, sy - i + 1, "M")
        _put(g, sx + 5, sy - 5, "h")


def _ground(g: list[list[str]], dx: int) -> None:
    cx = _CX + dx
    _span(g, 45, cx - 8, cx + 8, "o")
    _span(g, 46, cx - 5, cx + 5, "o")


# ===========================================================================
# 3/4 TOP-DOWN RIG (locked 2026-06-30). The duelists are now drawn turned ~35deg
# to face right and seen from ~25deg above, so they STAND IN the perspective
# stage instead of reading as flat front-on standees. The turn lives in the
# silhouette (leading right shoulder forward, far left receding, face-slit angled
# to the facing side), a lit hood-crown reads from the top, the cloak hem pools on
# the floor, and a cast shadow plants the weight. The renderer mirrors the sheet
# for left-facing. Driven by the SAME lean/bob/hunch/arm/leg params so the 41-frame
# contract and both builds (Cur broad / Stag tall) flow through unchanged. The
# front-facing _hood/_mantle/_eyes/_scarf/_pauldron/_sleeve above are retired
# (kept for git history; unreferenced).
# ===========================================================================


def _tq_shadow(g: list[list[str]], cx: int) -> None:
    """Cast-shadow ellipse on the floor — light upper-left, shadow lower-right."""
    for (yy, x0, x1) in [(43, cx - 7, cx + 8), (44, cx - 9, cx + 10), (45, cx - 6, cx + 7)]:
        _span(g, yy, x0, x1, "o")


# --- cloak runes: the personal mark -----------------------------------------
# Eight quiet glyphs worn on the cloak's chest — the install-id's demon made
# visibly YOURS without touching anything readability owns: painted only
# over pixels that are already cloth ('M'/'m'), so the silhouette, the team
# hood, and the eye heat are untouchable by construction. Rune 0 is the
# unmarked classic. Drawn in fold-shadow 'm' with a single lit 'L' pixel;
# at table distance it reads as cloth detail, up close it reads as a mark.
_RUNE_GLYPHS: "dict[int, list[str]]" = {
    1: ["..#..", ".#.#.", "..*..", ".#.#.", "..#.."],  # the eye
    2: ["#...#", ".#.#.", "..*..", ".#.#.", "#...#"],  # the cross
    3: ["..#..", "..#..", ".#*#.", "..#..", "..#.."],  # the anchor
    4: [".###.", "#...#", "#.*.#", "#...#", ".###."],  # the ring
    5: ["#....", ".#...", "..*..", "...#.", "....#"],  # the slash
    6: ["..#..", ".#.#.", "#.*.#", ".#.#.", "..#.."],  # the fang
    7: [".#.#.", ".#.#.", "..*..", ".#.#.", ".#.#."],  # the gate
}

# The rune player_sheet is currently building — a module global rather than
# a parameter threaded through 41 frames x 3 views x 3 compositors. The
# generator is single-threaded and deterministic; player_sheet sets it and
# restores it around each build.
_ACTIVE_RUNE = 0


def _paint_rune(g: list[list[str]], cx: int, sy: int) -> None:
    glyph = _RUNE_GLYPHS.get(_ACTIVE_RUNE)
    if glyph is None:
        return
    top = sy + 6  # chest rows, below the mantle, above the hem folds
    for ry, row in enumerate(glyph):
        for rx, ch in enumerate(row):
            if ch == ".":
                continue
            x, y = cx - 2 + rx, top + ry
            if 0 <= y < len(g) and 0 <= x < len(g[0]) and g[y][x] in ("M", "m"):
                g[y][x] = "L" if ch == "*" else "m"


def _tq_figure(g: list[list[str]], cx: int, dy: int, b: Build,
               hunch: int, leg: str, arm: str, spaulder: bool) -> None:
    """Compose the turned cloaked drifter. Faces right; all geometry keys off the
    Build so Cur (round, broad) and Stag (peak, narrow + scarf) diverge."""
    turn = 2                       # leading-side extra reach = the 3/4 turn
    sh_near, sh_far = b.sh + turn - 1, b.sh - 2
    sy = 18 + dy + hunch
    hy = dy + (1 if hunch > 0 else 0)   # hood tips down a touch when hunched

    # --- cloak body: turned column, leading (right) edge fuller, pooled hem ---
    _span(g, sy - 2, cx - (sh_far - 4), cx + (sh_near - 4), "M")
    _span(g, sy - 1, cx - (sh_far - 1), cx + (sh_near - 1), "M")
    _span(g, sy,     cx - sh_far,       cx + sh_near,       "M")
    hem = 40
    top = sy + 1
    near = b.body + turn
    far = b.body
    for i, y in enumerate(range(top, hem + 1)):
        fl = (i * b.flare) // 4
        nr = min(sh_near + 1, near + fl)
        fr = min(sh_far + 1, far + fl)
        if y >= hem - 1:           # pool inward where the cloak meets the floor
            nr -= 1
            fr -= 1
        _span(g, y, cx - fr, cx + nr, "M")
    _span(g, hem + 1, cx - (far - 2), cx + (near - 2), "M")     # hem pool
    _span(g, hem + 2, cx - (far - 3), cx + (near - 3), "m")
    for y in range(top, hem):      # fold seam, shifted toward the leading side
        _put(g, cx + 2, y, "m")
    _put(g, cx - far + 2, top + 5, "m")
    _put(g, cx - far + 3, top + 9, "m")

    # --- boots under the pooled hem, planted in the shadow ---
    if leg != "air":
        shift = {"strideR": 1, "strideL": -1, "splay": 0}.get(leg, 0)
        _span(g, hem,     cx + 1 + shift, cx + 5 + shift, "l")   # near (leading) foot
        _span(g, hem + 1, cx + 1 + shift, cx + 5 + shift, "l")
        _span(g, hem,     cx - 5 + shift, cx - 2 + shift, "l")   # far foot, smaller
        _span(g, hem + 1, cx - 5 + shift, cx - 2 + shift, "l")

    # --- hood: rounded/peaked crown seen from above, turned right ---
    if b.hood == "round":
        crown = [(3, cx - 3, cx + 4), (4, cx - 5, cx + 6), (5, cx - 6, cx + 7),
                 (6, cx - 6, cx + 8), (7, cx - 6, cx + 8), (8, cx - 6, cx + 8),
                 (9, cx - 6, cx + 8), (10, cx - 6, cx + 8), (11, cx - 5, cx + 8),
                 (12, cx - 4, cx + 7), (13, cx - 3, cx + 7), (14, cx - 2, cx + 6)]
    else:  # peak — taller, narrower, with a forward (right) cloth tip
        crown = [(1, cx + 1, cx + 4), (2, cx, cx + 5), (3, cx - 1, cx + 5),
                 (4, cx - 2, cx + 6), (5, cx - 3, cx + 6), (6, cx - 3, cx + 7),
                 (7, cx - 3, cx + 7), (8, cx - 3, cx + 7), (9, cx - 3, cx + 7),
                 (10, cx - 3, cx + 7), (11, cx - 2, cx + 7), (12, cx - 1, cx + 6),
                 (13, cx, cx + 6), (14, cx + 1, cx + 5)]
    for (yy, x0, x1) in crown:
        _span(g, yy + hy, x0, x1, "M")
    _span(g, 15 + hy, cx - (sh_far - 2), cx + 3, "M")           # neck -> shoulders

    # face cavity, low and angled to the facing (right) side; brim overhang
    for (yy, x0, x1) in [(11, cx + 1, cx + 7), (12, cx + 1, cx + 7),
                         (13, cx + 2, cx + 7), (14, cx + 3, cx + 6)]:
        _span(g, yy + hy, x0, x1, "k")
    # glowing eye-slit deep in the cavity, tipped to the facing side
    _span(g, 12 + hy, cx + 3, cx + 6, "e")
    _put(g, cx + 4, 12 + hy, b.hot)
    _put(g, cx + 6, 13 + hy, "e")

    # --- shoulder asymmetry tell: pauldron (Cur) vs trailing scarf (Stag) ---
    if spaulder and b.pauldron:
        _span(g, sy - 2, cx + sh_near - 4, cx + sh_near, "b")
        _span(g, sy - 1, cx + sh_near - 4, cx + sh_near + 1, "b")
        _span(g, sy,     cx + sh_near - 3, cx + sh_near, "b")
        _put(g, cx + sh_near - 3, sy - 2, "B")
    if b.cape == "scarf":
        sway = 1 if leg in ("strideR", "strideL", "air", "splay") else 0
        sx = cx - sh_far
        for i, y in enumerate(range(sy, 43)):
            off = i // 3 + sway
            _put(g, sx - off, y, "M")
            _put(g, sx - off - 1, y, "m")
            if i % 2 == 0:
                _put(g, sx - off + 1, y, "L")

    # --- throwing sleeve on the LEADING (near/right) arm ---
    if arm not in ("rest", "none"):
        ax, ay = cx + sh_near - 1, sy + 2
        if arm == "back":          # cocked up-and-back (wind-up)
            for i in range(5):
                _put(g, ax + i // 2, ay - i, "M")
                _put(g, ax + 1 + i // 2, ay - i, "M")
            _put(g, ax + 3, ay - 5, "h")
        elif arm == "fwd":         # thrust forward (release/follow)
            for i in range(8):
                _put(g, ax + i, ay + 1, "M")
                _put(g, ax + i, ay + 2, "M")
            _put(g, ax + 8, ay + 1, "h")
            _put(g, ax + 8, ay + 2, "L")
        elif arm == "up":          # raised to catch
            for i in range(8):
                _put(g, ax, ay - i, "M")
                _put(g, ax + 1, ay - i, "M")
            _put(g, ax, ay - 8, "h")
        elif arm == "reach":       # lowering after the catch
            for i in range(5):
                _put(g, ax + i, ay - i, "M")
                _put(g, ax + i, ay - i + 1, "M")
            _put(g, ax + 5, ay - 5, "h")


def _tq_figure_back(g: list[list[str]], cx: int, dy: int, b: Build,
                    hunch: int, leg: str, arm: str, spaulder: bool) -> None:
    """Back view — character walking AWAY from the camera. Symmetric cloak,
    no face cavity, hood seen from behind. Scarf/pauldron flip to their
    back-view side."""
    sy = 18 + dy + hunch
    hy = dy + (1 if hunch > 0 else 0)

    # --- cloak body: symmetric, no turn ---
    _span(g, sy - 2, cx - (b.sh - 4), cx + (b.sh - 4), "M")
    _span(g, sy - 1, cx - (b.sh - 1), cx + (b.sh - 1), "M")
    _span(g, sy,     cx - b.sh,       cx + b.sh,       "M")
    hem = 40
    top = sy + 1
    for i, y in enumerate(range(top, hem + 1)):
        fl = (i * b.flare) // 4
        half = min(b.sh + 1, b.body + fl)
        if y >= hem - 1:
            half -= 1
        _span(g, y, cx - half, cx + half, "M")
    _span(g, hem + 1, cx - (b.body - 2), cx + (b.body - 2), "M")
    _span(g, hem + 2, cx - (b.body - 3), cx + (b.body - 3), "m")
    for y in range(top, hem):
        _put(g, cx, y, "m")
    _put(g, cx - min(4, b.body - 1), top + 4, "m")
    _put(g, cx + min(3, b.body - 2), top + 6, "m")

    # --- boots ---
    if leg != "air":
        shift = {"strideR": 1, "strideL": -1, "splay": 0}.get(leg, 0)
        _span(g, hem,     cx - 5 + shift, cx - 2 + shift, "l")
        _span(g, hem + 1, cx - 5 + shift, cx - 2 + shift, "l")
        _span(g, hem,     cx + 2 + shift, cx + 5 + shift, "l")
        _span(g, hem + 1, cx + 2 + shift, cx + 5 + shift, "l")

    # --- hood: seen from behind, smooth dome/peak, NO face cavity ---
    if b.hood == "round":
        crown = [(4, cx - 4, cx + 4), (5, cx - 6, cx + 6), (6, cx - 7, cx + 7),
                 (7, cx - 7, cx + 7), (8, cx - 7, cx + 7), (9, cx - 7, cx + 7),
                 (10, cx - 7, cx + 7), (11, cx - 6, cx + 6), (12, cx - 5, cx + 5),
                 (13, cx - 4, cx + 4), (14, cx - 3, cx + 3)]
    else:
        crown = [(1, cx - 1, cx + 1), (2, cx - 2, cx + 2), (3, cx - 3, cx + 3),
                 (4, cx - 4, cx + 4), (5, cx - 4, cx + 4), (6, cx - 5, cx + 5),
                 (7, cx - 5, cx + 5), (8, cx - 5, cx + 5), (9, cx - 5, cx + 5),
                 (10, cx - 5, cx + 5), (11, cx - 4, cx + 4), (12, cx - 3, cx + 3),
                 (13, cx - 2, cx + 2), (14, cx - 2, cx + 2)]
    for (yy, x0, x1) in crown:
        _span(g, yy + hy, x0, x1, "M")
    _span(g, 15 + hy, cx - (b.sh - 2), cx + (b.sh - 2), "M")

    # --- back-view asymmetry: pauldron on RIGHT from behind, scarf on LEFT ---
    if spaulder and b.pauldron:
        _span(g, sy - 2, cx + b.sh - 4, cx + b.sh, "b")
        _span(g, sy - 1, cx + b.sh - 4, cx + b.sh + 1, "b")
        _span(g, sy,     cx + b.sh - 3, cx + b.sh, "b")
        _put(g, cx + b.sh - 3, sy - 2, "B")
    if b.cape == "scarf":
        sway = 1 if leg in ("strideR", "strideL", "air", "splay") else 0
        sx = cx - b.sh
        for i, y in enumerate(range(sy, 43)):
            off = i // 3 + sway
            _put(g, sx - off, y, "M")
            _put(g, sx - off - 1, y, "m")
            if i % 2 == 0:
                _put(g, sx - off + 1, y, "L")

    # --- sleeve: visible behind/above on the right side ---
    if arm not in ("rest", "none"):
        ax, ay = cx + b.sh - 1, sy + 2
        if arm == "back":
            for i in range(5):
                _put(g, ax - 1 + i // 3, ay - i, "M")
                _put(g, ax + i // 3, ay - i, "M")
            _put(g, ax, ay - 5, "h")
        elif arm == "fwd":
            for i in range(6):
                _put(g, ax + 1, ay - i, "M")
                _put(g, ax + 2, ay - i, "M")
            _put(g, ax + 1, ay - 6, "h")
        elif arm == "up":
            for i in range(8):
                _put(g, ax, ay - i, "M")
                _put(g, ax + 1, ay - i, "M")
            _put(g, ax, ay - 8, "h")
        elif arm == "reach":
            for i in range(5):
                _put(g, ax + i // 2, ay - i, "M")
                _put(g, ax + 1 + i // 2, ay - i, "M")
            _put(g, ax + 3, ay - 5, "h")


def _tq_figure_front(g: list[list[str]], cx: int, dy: int, b: Build,
                     hunch: int, leg: str, arm: str, spaulder: bool) -> None:
    """Front view — character walking TOWARD the camera. Face cavity visible
    from the front (wider), eyes centered. Scarf/pauldron on their view-side."""
    sy = 18 + dy + hunch
    hy = dy + (1 if hunch > 0 else 0)

    # --- cloak body: symmetric, slight spread showing the opening ---
    _span(g, sy - 2, cx - (b.sh - 4), cx + (b.sh - 4), "M")
    _span(g, sy - 1, cx - (b.sh - 1), cx + (b.sh - 1), "M")
    _span(g, sy,     cx - b.sh,       cx + b.sh,       "M")
    hem = 40
    top = sy + 1
    for i, y in enumerate(range(top, hem + 1)):
        fl = (i * b.flare) // 4
        half = min(b.sh + 1, b.body + fl)
        if y >= hem - 1:
            half -= 1
        _span(g, y, cx - half, cx + half, "M")
    _span(g, hem + 1, cx - (b.body - 2), cx + (b.body - 2), "M")
    _span(g, hem + 2, cx - (b.body - 3), cx + (b.body - 3), "m")
    for y in range(top, hem):
        _put(g, cx, y, "m")
    _put(g, cx - min(4, b.body - 1), top + 3, "m")
    _put(g, cx + min(3, b.body - 2), top + 5, "m")

    # --- boots ---
    if leg != "air":
        shift = {"strideR": 1, "strideL": -1, "splay": 0}.get(leg, 0)
        _span(g, hem,     cx - 5 + shift, cx - 2 + shift, "l")
        _span(g, hem + 1, cx - 5 + shift, cx - 2 + shift, "l")
        _span(g, hem,     cx + 2 + shift, cx + 5 + shift, "l")
        _span(g, hem + 1, cx + 2 + shift, cx + 5 + shift, "l")

    # --- hood: front-facing, WIDER face cavity, brim overhanging ---
    if b.hood == "round":
        crown = [(4, cx - 5, cx + 5), (5, cx - 7, cx + 7), (6, cx - 8, cx + 8),
                 (7, cx - 8, cx + 8), (8, cx - 8, cx + 8), (9, cx - 8, cx + 8),
                 (10, cx - 8, cx + 8), (11, cx - 7, cx + 7), (12, cx - 6, cx + 6),
                 (13, cx - 5, cx + 5), (14, cx - 4, cx + 4)]
        for (yy, x0, x1) in crown:
            _span(g, yy + hy, x0, x1, "M")
        # wider front face cavity
        for (yy, x0, x1) in [(10, cx - 6, cx + 6), (11, cx - 6, cx + 6),
                              (12, cx - 5, cx + 5), (13, cx - 4, cx + 4)]:
            _span(g, yy + hy, x0, x1, "k")
    else:
        crown = [(1, cx - 1, cx + 1), (2, cx - 2, cx + 2), (3, cx - 3, cx + 3),
                 (4, cx - 4, cx + 4), (5, cx - 5, cx + 5), (6, cx - 5, cx + 5),
                 (7, cx - 6, cx + 6), (8, cx - 6, cx + 6), (9, cx - 6, cx + 6),
                 (10, cx - 6, cx + 6), (11, cx - 5, cx + 5), (12, cx - 4, cx + 4),
                 (13, cx - 3, cx + 3), (14, cx - 2, cx + 2)]
        for (yy, x0, x1) in crown:
            _span(g, yy + hy, x0, x1, "M")
        for (yy, x0, x1) in [(10, cx - 4, cx + 4), (11, cx - 4, cx + 4),
                              (12, cx - 3, cx + 3), (13, cx - 2, cx + 2)]:
            _span(g, yy + hy, x0, x1, "k")
    _span(g, 15 + hy, cx - (b.sh - 2), cx + (b.sh - 2), "M")

    # front-facing eyes: centered, both visible, full glow
    _span(g, 11 + hy, cx - 4, cx - 2, "e")
    _span(g, 11 + hy, cx + 2, cx + 4, "e")
    _put(g, cx - 3, 11 + hy, b.hot)
    _put(g, cx + 3, 11 + hy, b.hot)

    # --- front-view asymmetry: pauldron on LEFT (viewer's left), scarf on RIGHT ---
    if spaulder and b.pauldron:
        _span(g, sy - 2, cx - b.sh, cx - b.sh + 4, "b")
        _span(g, sy - 1, cx - b.sh - 1, cx - b.sh + 4, "b")
        _span(g, sy,     cx - b.sh, cx - b.sh + 3, "b")
        _put(g, cx - b.sh + 3, sy - 2, "B")
    if b.cape == "scarf":
        sway = 1 if leg in ("strideR", "strideL", "air", "splay") else 0
        sx = cx + b.sh
        for i, y in enumerate(range(sy, 43)):
            off = i // 3 + sway
            _put(g, sx + off, y, "M")
            _put(g, sx + off + 1, y, "m")
            if i % 2 == 0:
                _put(g, sx + off - 1, y, "L")

    # --- sleeve: extends toward camera (downward on the leading side) ---
    if arm not in ("rest", "none"):
        ax, ay = cx + b.sh - 1, sy + 2
        if arm == "back":
            for i in range(5):
                _put(g, ax - 1 + i // 3, ay - i, "M")
                _put(g, ax + i // 3, ay - i, "M")
            _put(g, ax, ay - 5, "h")
        elif arm == "fwd":
            for i in range(6):
                _put(g, ax, ay + i, "M")
                _put(g, ax + 1, ay + i, "M")
            _put(g, ax, ay + 6, "h")
            _put(g, ax + 1, ay + 6, "L")
        elif arm == "up":
            for i in range(8):
                _put(g, ax, ay - i, "M")
                _put(g, ax + 1, ay - i, "M")
            _put(g, ax, ay - 8, "h")
        elif arm == "reach":
            for i in range(4):
                _put(g, ax, ay + i, "M")
                _put(g, ax + 1, ay + i, "M")
            _put(g, ax, ay + 4, "h")


def _draw_duelist(
    *,
    lean: int = 0,
    bob: int = 0,
    hunch: int = 0,
    arm_l: str = "rest",       # accepted for signature-compat; unused (cloak)
    arm_r: str = "none",
    leg: str = "stand",
    ground: bool = True,
    spaulder: bool = True,
    headgear: str = "horns",
) -> list[list[str]]:
    """Compose one 3/4 cloaked-drifter frame. `headgear` selects the build
    ('antlers' -> Stag, else Cur); `lean` shifts the turn, `bob` breathes,
    `hunch` folds forward, `arm_r` drives the throwing sleeve, `leg` the stride."""
    b = STAG if headgear == "antlers" else CUR
    cx = _CX + lean
    g = _blank()
    if ground:
        _tq_shadow(g, cx)
    _tq_figure(g, cx, bob, b, hunch, leg, arm_r, spaulder)
    _paint_rune(g, cx, 18 + bob + hunch)
    _shade(g)
    _outline(g)
    return g


def _draw_duelist_back(
    *,
    lean: int = 0,
    bob: int = 0,
    hunch: int = 0,
    arm_l: str = "rest",
    arm_r: str = "none",
    leg: str = "stand",
    ground: bool = True,
    spaulder: bool = True,
    headgear: str = "horns",
) -> list[list[str]]:
    b = STAG if headgear == "antlers" else CUR
    cx = _CX + lean
    g = _blank()
    if ground:
        _tq_shadow(g, cx)
    _tq_figure_back(g, cx, bob, b, hunch, leg, arm_r, spaulder)
    _paint_rune(g, cx, 18 + bob + hunch)
    _shade(g)
    _outline(g)
    return g


def _draw_duelist_front(
    *,
    lean: int = 0,
    bob: int = 0,
    hunch: int = 0,
    arm_l: str = "rest",
    arm_r: str = "none",
    leg: str = "stand",
    ground: bool = True,
    spaulder: bool = True,
    headgear: str = "horns",
) -> list[list[str]]:
    b = STAG if headgear == "antlers" else CUR
    cx = _CX + lean
    g = _blank()
    if ground:
        _tq_shadow(g, cx)
    _tq_figure_front(g, cx, bob, b, hunch, leg, arm_r, spaulder)
    _paint_rune(g, cx, 18 + bob + hunch)
    _shade(g)
    _outline(g)
    return g


def _flash(g: list[list[str]]) -> list[list[str]]:
    out = [row[:] for row in g]
    for y in range(PLAYER_PX):
        for x in range(PLAYER_PX):
            c = out[y][x]
            if c in _BODY or c in _BONE or c in ("l", "y"):
                out[y][x] = "h"
    return out


def _spark_burst(g: list[list[str]], x: int, y: int) -> None:
    _put(g, x, y, "B")
    _put(g, x - 1, y, "y")
    _put(g, x + 1, y, "y")
    _put(g, x, y - 1, "y")
    _put(g, x, y + 1, "e")
    _put(g, x - 1, y - 1, "e")


def _gore_chunks(g: list[list[str]], stage: int) -> None:
    """Comedic-gore burst on death — flung cloak scraps + ember sparks. Lands in
    stages so the death anim escalates. (The persistent kill stain lives in the
    arena floor, not here.)"""
    chunks = [
        (-9, 18, "M"), (9, 16, "m"), (-12, 24, "m"), (12, 22, "M"),
        (-6, 14, "M"), (7, 12, "m"), (-13, 30, "m"), (13, 28, "M"),
        (-4, 12, "y"), (5, 10, "e"), (-10, 32, "m"), (10, 31, "M"),
    ]
    cx = _CX
    for i in range(min(len(chunks), stage * 3)):
        dx, dy, ch = chunks[i]
        spread = stage
        _put(g, cx + dx + (1 if dx > 0 else -1) * spread, dy, ch)


def _cloak_heap(lean: int = 0) -> list[list[str]]:
    """The corpse: a crumpled cloak mound with the hood fallen forward and a
    last dim eye-glow guttering out."""
    g = _blank()
    cx = _CX + lean
    _ground(g, lean)
    _span(g, 38, cx - 6, cx + 6, "M")
    _span(g, 39, cx - 9, cx + 9, "M")
    _span(g, 40, cx - 11, cx + 11, "M")
    _span(g, 41, cx - 11, cx + 11, "M")
    _span(g, 42, cx - 10, cx + 10, "m")
    # fallen hood lump slumped forward-left
    _span(g, 36, cx - 5, cx + 1, "M")
    _span(g, 37, cx - 7, cx + 3, "M")
    _put(g, cx - 3, 38, "e")    # guttering eye-glow
    _put(g, cx - 2, 38, "e")
    _shade(g)
    _outline(g)
    return g


def _player_frames(headgear: str = "horns") -> list[list[list[str]]]:
    """The 41-frame v2 sequence: IDLE6 RUN6 THROW8 DASH4 HIT4 CATCH3 DEATH10.
    `headgear` ('horns' -> Cur / 'antlers' -> Stag) selects the cloak build so
    P0 and P1 read apart before color registers."""
    def D(**kw):
        return _draw_duelist(headgear=headgear, **kw)

    frames: list[list[list[str]]] = []

    # IDLE (6): a fuller hooded breath (exaggerated) — the upper cloak swells
    # and settles with more travel so even standing still has life.
    for bob in (0, -1, -2, -2, -1, 1):
        frames.append(D(bob=bob))

    # RUN (6): a harder forward drive + a bouncier air phase (exaggerated).
    run_cycle = [
        ("strideR", 0), ("air", -2), ("strideL", 0),
        ("strideR", -1), ("air", -2), ("strideL", 0),
    ]
    for leg, bob in run_cycle:
        frames.append(D(lean=2, bob=bob, leg=leg))

    # THROW (8): a BIG coiled wind-up (lean hard back, sleeve cocked) snapping to
    # a hard follow-through — exaggerated anticipation + overshoot for punch.
    frames.append(D(lean=-2, arm_r="back"))
    frames.append(D(lean=-2, arm_r="back", hunch=-1))
    frames.append(D(lean=-3, arm_r="back", hunch=-2))   # deepest coil
    rel = D(lean=2, arm_r="fwd")                          # hard snap forward
    for i in range(11):                      # longer ember release smear
        _put(rel, min(PLAYER_PX - 1, _CX + 13 + i), 21, "e" if i < 7 else "y")
    frames.append(rel)
    frames.append(D(lean=2, arm_r="fwd"))
    frames.append(D(lean=1, arm_r="fwd"))
    frames.append(D(lean=0, arm_r="reach"))
    frames.append(D(lean=0))

    # DASH (4): a harder lunge (exaggerated lean), scarf stretched back, a
    # longer motion afterimage streaking off the trailing edge.
    for k in range(4):
        d = D(lean=3 + (k % 2), leg="splay", arm_r="fwd", ground=False)
        for t in (3, 6, 9, 12):
            for y in range(18, 36):
                xx = max(0, _CX - 6)
                if d[y][xx] in _BODY:
                    _put(d, max(0, _CX - 5 - t), y, "e" if t == 3 else "m")
        frames.append(d)

    # HIT (4): white flash, then a BIG recoiling stagger (exaggerated) that
    # eases back to neutral.
    frames.append(_flash(D()))
    frames.append(D(lean=-3, hunch=-2))
    frames.append(D(lean=-1, hunch=-1))
    frames.append(D(lean=0))

    # CATCH (3): sleeve snaps up, spark pops, lowers.
    frames.append(D(arm_r="up"))
    c1 = D(arm_r="up")
    _spark_burst(c1, _CX + (CUR.sh - 3) if headgear != "antlers" else _CX + (STAG.sh - 3), 11)
    frames.append(c1)
    frames.append(D(arm_r="reach"))

    # DEATH (10): stagger -> fold -> buckle -> gore burst -> cloak heap.
    frames.append(D(lean=-1, hunch=-1))
    frames.append(D(lean=-2))
    frames.append(D(lean=-1, hunch=1))
    frames.append(D(lean=0, hunch=2))
    buckle = D(lean=0, hunch=3)
    _gore_chunks(buckle, 1)
    frames.append(buckle)
    burst = D(lean=0, hunch=4)
    _gore_chunks(burst, 2)
    frames.append(burst)
    burst2 = _cloak_heap()
    _gore_chunks(burst2, 3)
    frames.append(burst2)
    disperse = _cloak_heap()
    _gore_chunks(disperse, 4)
    frames.append(disperse)
    frames.append(_cloak_heap())
    frames.append(_cloak_heap())

    # CHARGE (4): a coiled throw wind-up — planted, leaned hard back, sleeve
    # cocked — with a charge spark that SWELLS at the cocked hand as the throw
    # builds. Loops while THROW is held; releasing fires the THROW anim. This is
    # the "coiled potential energy" read the charge mechanic needs.
    for k in range(4):
        ch = D(lean=-2, arm_r="back", hunch=-1, leg="stand")
        hx, hy = max(0, _CX - 9), 20
        _put(ch, hx, hy, ["e", "y", "y", "B"][k])   # swelling core
        _put(ch, hx, hy - 1, "e")
        if k >= 1:
            _put(ch, hx - 1, hy, "y")
        if k >= 2:
            _put(ch, hx + 1, hy, "y")
            _put(ch, hx, hy + 1, "e")
        if k >= 3:
            _put(ch, hx - 1, hy - 1, "B")
            _put(ch, hx + 1, hy - 1, "y")
            _put(ch, hx, hy - 2, "y")
        frames.append(ch)

    assert len(frames) == 45, f"expected 45 frames, got {len(frames)}"
    return frames


def _player_frames_back(headgear: str = "horns") -> list[list[list[str]]]:
    """45-frame back-view sequence — same anim layout as the side sheet."""
    def D(**kw):
        return _draw_duelist_back(headgear=headgear, **kw)

    frames: list[list[list[str]]] = []

    for bob in (0, -1, -2, -2, -1, 1):
        frames.append(D(bob=bob))

    run_cycle = [
        ("strideR", 0), ("air", -2), ("strideL", 0),
        ("strideR", -1), ("air", -2), ("strideL", 0),
    ]
    for leg, bob in run_cycle:
        frames.append(D(lean=0, bob=bob, leg=leg))

    frames.append(D(lean=0, arm_r="back"))
    frames.append(D(lean=0, arm_r="back", hunch=-1))
    frames.append(D(lean=0, arm_r="back", hunch=-2))
    frames.append(D(lean=0, arm_r="fwd"))
    frames.append(D(lean=0, arm_r="fwd"))
    frames.append(D(lean=0, arm_r="fwd"))
    frames.append(D(lean=0, arm_r="reach"))
    frames.append(D(lean=0))

    for k in range(4):
        frames.append(D(lean=0, leg="splay", arm_r="fwd", ground=False))

    frames.append(_flash(D()))
    frames.append(D(lean=0, hunch=-2))
    frames.append(D(lean=0, hunch=-1))
    frames.append(D(lean=0))

    frames.append(D(arm_r="up"))
    frames.append(D(arm_r="up"))
    frames.append(D(arm_r="reach"))

    frames.append(D(lean=0, hunch=-1))
    frames.append(D(lean=0))
    frames.append(D(lean=0, hunch=1))
    frames.append(D(lean=0, hunch=2))
    buckle = D(lean=0, hunch=3)
    _gore_chunks(buckle, 1)
    frames.append(buckle)
    burst = D(lean=0, hunch=4)
    _gore_chunks(burst, 2)
    frames.append(burst)
    burst2 = _cloak_heap()
    _gore_chunks(burst2, 3)
    frames.append(burst2)
    disperse = _cloak_heap()
    _gore_chunks(disperse, 4)
    frames.append(disperse)
    frames.append(_cloak_heap())
    frames.append(_cloak_heap())

    for k in range(4):
        ch = D(lean=0, arm_r="back", hunch=-1, leg="stand")
        hx, hy = max(0, _CX + 8), 20
        _put(ch, hx, hy, ["e", "y", "y", "B"][k])
        _put(ch, hx, hy - 1, "e")
        if k >= 1:
            _put(ch, hx - 1, hy, "y")
        if k >= 2:
            _put(ch, hx + 1, hy, "y")
            _put(ch, hx, hy + 1, "e")
        if k >= 3:
            _put(ch, hx - 1, hy - 1, "B")
            _put(ch, hx + 1, hy - 1, "y")
            _put(ch, hx, hy - 2, "y")
        frames.append(ch)

    assert len(frames) == 45, f"expected 45 back frames, got {len(frames)}"
    return frames


def _player_frames_front(headgear: str = "horns") -> list[list[list[str]]]:
    """45-frame front-view sequence — same anim layout as the side sheet."""
    def D(**kw):
        return _draw_duelist_front(headgear=headgear, **kw)

    frames: list[list[list[str]]] = []

    for bob in (0, -1, -2, -2, -1, 1):
        frames.append(D(bob=bob))

    run_cycle = [
        ("strideR", 0), ("air", -2), ("strideL", 0),
        ("strideR", -1), ("air", -2), ("strideL", 0),
    ]
    for leg, bob in run_cycle:
        frames.append(D(lean=0, bob=bob, leg=leg))

    frames.append(D(lean=0, arm_r="back"))
    frames.append(D(lean=0, arm_r="back", hunch=-1))
    frames.append(D(lean=0, arm_r="back", hunch=-2))
    frames.append(D(lean=0, arm_r="fwd"))
    frames.append(D(lean=0, arm_r="fwd"))
    frames.append(D(lean=0, arm_r="fwd"))
    frames.append(D(lean=0, arm_r="reach"))
    frames.append(D(lean=0))

    for k in range(4):
        frames.append(D(lean=0, leg="splay", arm_r="fwd", ground=False))

    frames.append(_flash(D()))
    frames.append(D(lean=0, hunch=-2))
    frames.append(D(lean=0, hunch=-1))
    frames.append(D(lean=0))

    frames.append(D(arm_r="up"))
    frames.append(D(arm_r="up"))
    frames.append(D(arm_r="reach"))

    frames.append(D(lean=0, hunch=-1))
    frames.append(D(lean=0))
    frames.append(D(lean=0, hunch=1))
    frames.append(D(lean=0, hunch=2))
    buckle = D(lean=0, hunch=3)
    _gore_chunks(buckle, 1)
    frames.append(buckle)
    burst = D(lean=0, hunch=4)
    _gore_chunks(burst, 2)
    frames.append(burst)
    burst2 = _cloak_heap()
    _gore_chunks(burst2, 3)
    frames.append(burst2)
    disperse = _cloak_heap()
    _gore_chunks(disperse, 4)
    frames.append(disperse)
    frames.append(_cloak_heap())
    frames.append(_cloak_heap())

    for k in range(4):
        ch = D(lean=0, arm_r="back", hunch=-1, leg="stand")
        hx, hy = max(0, _CX + 8), 20
        _put(ch, hx, hy, ["e", "y", "y", "B"][k])
        _put(ch, hx, hy - 1, "e")
        if k >= 1:
            _put(ch, hx - 1, hy, "y")
        if k >= 2:
            _put(ch, hx + 1, hy, "y")
            _put(ch, hx, hy + 1, "e")
        if k >= 3:
            _put(ch, hx - 1, hy - 1, "B")
            _put(ch, hx + 1, hy - 1, "y")
            _put(ch, hx, hy - 2, "y")
        frames.append(ch)

    assert len(frames) == 45, f"expected 45 front frames, got {len(frames)}"
    return frames


_PLAYER_FRAME_CACHE: dict[str, list[list[list[str]]]] = {}


def _frames_for(side: str, direction: str = "side") -> list[list[list[str]]]:
    headgear = "antlers" if side == "p1" else "horns"
    key = f"{headgear}_{direction}_r{_ACTIVE_RUNE}"
    if key not in _PLAYER_FRAME_CACHE:
        if direction == "back":
            _PLAYER_FRAME_CACHE[key] = _player_frames_back(headgear)
        elif direction == "front":
            _PLAYER_FRAME_CACHE[key] = _player_frames_front(headgear)
        else:
            _PLAYER_FRAME_CACHE[key] = _player_frames(headgear)
    return _PLAYER_FRAME_CACHE[key]


def player_sheet(side: str, rune: int = 0) -> Canvas:
    """3-row × 45-column atlas (48×48 cells): row 0 = side, row 1 = back,
    row 2 = front. Each row: IDLE6 RUN6 THROW8 DASH4 HIT4 CATCH3 DEATH10 CHARGE4.
    The engine selects the row from movement direction. `rune` picks the
    cloak mark (0 = unmarked; see `_RUNE_GLYPHS`) — the install-id's demon."""
    global _ACTIVE_RUNE
    _ACTIVE_RUNE = rune
    canvas = Canvas(PLAYER_PX * 45, PLAYER_PX * 3)
    for row_idx, direction in enumerate(("side", "back", "front")):
        frames = _frames_for(side, direction)
        for i, art in enumerate(frames):
            paint(canvas, i * PLAYER_PX, row_idx * PLAYER_PX, _grid_to_str(art), side=side)
    _ACTIVE_RUNE = 0
    return canvas


_ANIM_ROWS = [
    ("idle", 0, 6),
    ("run", 6, 6),
    ("throw", 12, 8),
    ("dash", 20, 4),
    ("hit", 24, 4),
    ("catch", 28, 3),
    ("death", 31, 10),
    ("charge", 41, 4),
]


def duelist_contact_sheet(side: str) -> Canvas:
    """Review sheet: each animation on its own row, scaled 5x on a void bg."""
    frames = _frames_for(side)
    scale = 5
    cell = PLAYER_PX * scale
    cols = max(n for _, _, n in _ANIM_ROWS)
    rows = len(_ANIM_ROWS)
    c = Canvas(cols * cell, rows * cell, PALETTE["deep_ash"])
    for r, (_, start, count) in enumerate(_ANIM_ROWS):
        for i in range(count):
            tile = Canvas(PLAYER_PX, PLAYER_PX)
            paint(tile, 0, 0, _grid_to_str(frames[start + i]), side=side)
            c.blit(tile, i * cell, r * cell, scale)
    return c



# ===========================================================================
# Boomerang — the visual protagonist. 12x12 source.
#
# Asymmetric bone-fang. The leading edge is wider than the tail; a carved
# sigil sits on the spine; a hot-bone gleam runs along the upper edge. Per-
# round blood marks accumulate as cosmetic flecks (sim-safe).
# ===========================================================================

BOOM_CLEAN = """
............
.kkkkk......
kbBBBbk.....
kBBhBBwk....
.kBBwwwbk...
..kwwwwbk...
...kwwwwbk..
....kwwwbk..
.....kwwbk..
......kbk...
.......k....
............
"""

# Variants: blood marks land progressively as kills accumulate.
BOOM_1MARK = """
............
.kkkkk......
kbBBBbk.....
kBmhBBwk....
.kBmwwwbk...
..kwwwwbk...
...kwwwwbk..
....kwwwbk..
.....kwwbk..
......kbk...
.......k....
............
"""

BOOM_2MARK = """
............
.kkkkk......
kbBBBbk.....
kBmhBBwk....
.kBmwwwbk...
..kwwwwbk...
...kwwccbk..
....kwccbk..
.....kwwbk..
......kbk...
.......k....
............
"""

BOOM_3MARK = """
............
.kkkkk......
kbmmmbk.....
kmmhBBwk....
.kBmwwwbk...
..kmwwccbk..
...kmwccbk..
....kwccbk..
.....kwwbk..
......kbk...
.......k....
............
"""

# The carved sigil uses 'X' and 'x' as outline glyphs; remap them to void.
BOOM_KEYS_EXTRA = {"X": PALETTE["void"], "x": PALETTE["bone"]}


def paint_boomerang(canvas: Canvas, ox: int, oy: int, art: str) -> None:
    """Special painter for the boomerang sigil glyphs."""
    keys = keys_for("p0")
    keys.update(BOOM_KEYS_EXTRA)
    rows = art.strip("\n").split("\n")
    for y, row in enumerate(rows):
        for x, char in enumerate(row):
            color = keys.get(char)
            if color is not None and color[3] > 0:
                canvas.set(ox + x, oy + y, color)


def boomerang(marks: int = 0) -> Canvas:
    art = [BOOM_CLEAN, BOOM_1MARK, BOOM_2MARK, BOOM_3MARK][min(marks, 3)]
    c = Canvas(12, 12)
    paint_boomerang(c, 0, 0, art)
    return c


def boomerang_sheet() -> Canvas:
    """4-variant strip: 0 / 1 / 2 / 3+ kills."""
    c = Canvas(48, 12)
    for i, art in enumerate([BOOM_CLEAN, BOOM_1MARK, BOOM_2MARK, BOOM_3MARK]):
        paint_boomerang(c, i * 12, 0, art)
    return c


# ===========================================================================
# Trail — flying (3 frames) + returning (3 frames). 12x12 each, 6-frame
# strip total.
# ===========================================================================

# Flying: spark/ember dots, broken pattern, dissipating.
TRAIL_FLY_F0 = """
............
............
............
............
.....yy.....
....yeey....
....eMee....
.....ee.....
.....y......
............
............
............
"""

TRAIL_FLY_F1 = """
............
............
............
............
.....y......
....yyy.....
....yey.....
.....e......
............
............
............
............
"""

TRAIL_FLY_F2 = """
............
............
............
............
.....y......
.....y......
............
............
............
............
............
............
"""

# Returning: recall-blue ticks mixed with bone, distinct shape.
TRAIL_RTN_F0 = """
............
............
............
....bbbb....
...rrbbrr...
..rrrCCrrr..
...rrCCrr...
....rrrr....
.....rr.....
............
............
............
"""

TRAIL_RTN_F1 = """
............
............
............
............
....r.r.....
...r.r.r....
....r.r.....
.....r......
............
............
............
............
"""

TRAIL_RTN_F2 = """
............
............
............
............
............
.....r......
....r.r.....
.....r......
............
............
............
............
"""


def trail_sheet() -> Canvas:
    """6-frame strip: 3 flying + 3 returning, 12x12 each."""
    c = Canvas(72, 12)
    for i, art in enumerate([TRAIL_FLY_F0, TRAIL_FLY_F1, TRAIL_FLY_F2,
                              TRAIL_RTN_F0, TRAIL_RTN_F1, TRAIL_RTN_F2]):
        paint_boomerang(c, i * 12, 0, art)
    return c


# ===========================================================================
# Hit burst — 4 frames, 24x24 each. Hard radial spokes, hit-white first,
# then spark accent. Capped at ~12 frames total when paired with the death
# burst (kill frame budget per `VISUAL_TARGET_PACK.md` § Kill Frame).
# ===========================================================================

HIT_F0 = """
........................
........................
........................
........................
........................
.........h..h...........
..........hh............
..........hh............
.........hhhh...........
.......hhhhhhhh.........
......hhhhhhhhhh........
......hhhhhhhhhh........
......hhhhhhhhhh........
.......hhhhhhhh.........
.........hhhh...........
..........hh............
..........hh............
.........h..h...........
........................
........................
........................
........................
........................
........................
"""

HIT_F1 = """
........................
........................
........h.....h.........
........h.....h.........
........h.....h.........
........y.....y.........
.........h...h..........
.........y...y..........
.....hhh..hhh..hhh......
.....yhh..hhh..hhy......
.h....hhhhhhhhhhh....h..
.hyy..hhhhhhhhhhh..yyh..
.h....hhhhhhhhhhh....h..
.....yhh..hhh..hhy......
.....hhh..hhh..hhh......
.........y...y..........
.........h...h..........
........y.....y.........
........h.....h.........
........h.....h.........
........h.....h.........
........................
........................
........................
"""

HIT_F2 = """
........................
.......y.......y........
.......y.......y........
......y.........y.......
......y.........y.......
.....y...........y......
.....y..h.....h..y......
....y...h.....h...y.....
....y..............y....
...y..hh.......hh..y....
y...hhhh.......hhhh...y.
y...hhhh.......hhhh...y.
y...hhhh.......hhhh...y.
...y..hh.......hh..y....
....y..............y....
....y...h.....h...y.....
.....y..h.....h..y......
.....y...........y......
......y.........y.......
......y.........y.......
.......y.......y........
.......y.......y........
........................
........................
"""

HIT_F3 = """
y....y....y....y....y...
.y...y....y....y...y....
..y..y....y....y..y.....
...y.y....y....y.y......
....yy....y....yy.......
.....y....y....y........
......y..yyy..y.........
.......y.....y..........
........y...y...........
.........y.y............
y.........y..........y..
.y.......yyy........y...
y.........y..........y..
.........y.y............
........y...y...........
.......y.....y..........
......y..yyy..y.........
.....y....y....y........
....yy....y....yy.......
...y.y....y....y.y......
..y..y....y....y..y.....
.y...y....y....y...y....
y....y....y....y....y...
........................
"""


def hit_burst_sheet() -> Canvas:
    c = Canvas(24 * 4, 24)
    for i, art in enumerate([HIT_F0, HIT_F1, HIT_F2, HIT_F3]):
        paint(c, i * 24, 0, art, side="p0")
    return c


# ===========================================================================
# Death burst — 6 frames, 24x24 each.
#
# Frame 0: white silhouette flash (tail end of the hit-flash).
# Frame 1: chunky radial blood spray, hard pixels.
# Frame 2: bone shards begin scattering outward.
# Frame 3: shards reach apex, body remnant darkening.
# Frame 4: shards falling; floor stain begins committing.
# Frame 5: corpse mark fully committed (this frame is what persists).
#
# Color codes: M = victim main, m = victim dark, b = bone, h = hit-white,
# y = spark. The atlas uses P0 colors; the renderer recolors via a shader
# uniform per kill.
# ===========================================================================

DEATH_F0 = """
........................
........................
........................
.....h..........h.......
....hh..........hh......
....hhhhhhhhhhhhhh......
....hMMMMMMMMMMMMh......
....hMMMMMMMMMMMMh......
....hMMhMMMMMMhMMh......
....hMMMMMMMMMMMMh......
...hhMMMMMMMMMMMMhh.....
...hMMMMMMMMMMMMMMh.....
...hMMMMMMMMMMMMMMh.....
...hMMMMMMMMMMMMMMh.....
....hMMMMMMMMMMMMh......
....hhMMMMMMMMMMhh......
.....hhMMMMMMMMhh.......
......hhMMMMMMhh........
.......hhMMMMhh.........
........hhMMhh..........
.........hhhh...........
..........hh............
........................
........................
"""

DEATH_F1 = """
........................
.....m..............m...
......m...m..mm....m....
.....m.....mm....m......
....m..mm..hh.mm....m...
.m..m..hMMMMMMh.m..m....
....mMMMMMMMMMMMm.......
.m.mhMMMMMMMMMMMMm......
....hMMMmMMMmMMmMh.m....
m..hMMMmMMMMMMMmMMh.....
.mhmMMMmMMMMMMMMMMh..m..
..hMMMMMMMMMMMMMMMMh.m..
.mhMMMMMMMMMMMMMMMMh....
m.hMMMMMMMMMMMMMMMMm....
..hhMMMMMMMMMMMMMMhh.m..
m.mhMMMMMMMMMMMMMMh.m...
....hhmMMMMMMMMMmhh.....
m....mhhMMMMMMMmh..m....
......mmhhMMmmh..m......
....m..mmmmmm.m.....m...
.m...m..m..m......m.....
....m...m.m...m...m.m...
......m...m...........m.
......m.................
"""

DEATH_F2 = """
........................
.b....m...........m..b..
....b....b..m..b........
.....b...m..m...b...m...
.m..b..b..mm..b....b....
......b..mMMMm..b...m...
.b....mMMMMMMMm.....b...
......mMMMMMMMm....b....
m..b..mMMMmMMmMm....m..b
.....bmMMMMMMmMm..b.....
m..bmmMMMMMMMMMm.....m..
....mmMMMMMMMMMMb.b.....
.b.mmMMMMMMMMMMMm.......
m..mmMMMMMMMMMMMm....b..
.b.mmMMMMMMMMMmmm....m..
m...mmMMMMMMMmm.b...b...
.b...mmmMMMMmm......m...
......mmmMMmm..b....b...
m..b...mmmmm.....m......
.b....mm.....m..b...m...
......b..b....b...b.....
.m...b..m...b...b...m..b
b..m......m..b....b.....
m...b....m..b...m...m..b
"""

DEATH_F3 = """
.b........b.................
b..............b........b
.b...m......m..b...m..b...
b...b..mm.....b...b...m.
.....b.b..mmm.b.b.....b.
.b.....bmmmmm......b....
b...m..bmmmmmm....m..b..
.b.....bmmmmmm....m..b..
b....b.bmmmmmmm.....b...
.b....bmmmmmmmm....m...b
b...m..bmmmmmmmm.....b..
.b....mbmmmmmmmm.....m..
b..b..mbmmmmmmmm......b.
.b....mmbmmmmmmm.....b..
b...m..mmmmmmmmm....m...
.b....b.mmmmmmm......b..
b..m...b.mmmmmm....b....
.b...b...mmmmmm....m...b
b..b...b..mmmmm.....b...
.b....b...mmmmm....m..b.
b..m......mmmmm......b..
.b....b...mmmmm....m..b.
b..m...b..mmmm......b...
.b....b....mmm.....m..b.
"""

DEATH_F4 = """
........................
b...........b...........
.....b........b.........
b........b.....b........
.....b.....b.........b..
b.......mmmm......b.....
.....bmmMMMMmm.....b....
....mmMMMMMMMMm.........
b..mmmMMMmMMmMmm....m..b
..mmmMMMMMMMMMmm....b...
.mmMMMMMMMMMMMMmm....b..
.mmMMMMMMMMMMMMMm.....b.
.mmMMMMMMMMMMMMMm....b..
.mmMMMMMMMMMMMMMm.....b.
b.mmMMMMMMMMMMMmm.b.....
..mmmmMMMMMMMmmmm....b..
b...mmmMMMMMmmm.....b...
......mmmmmmm....b...b..
.b....mmmmm....b...m..b.
b...mmmmmmm.....b...b...
b...mmmmmmm....b...m..b.
.....mmmmmm....b...b..b.
b...mmmmmm......b...b...
.b...mmmmm.....b...m..b.
"""

DEATH_F5 = """
........................
........................
........................
........................
........................
........................
.........mm.............
........mmmm............
........mmmmm...........
.......mmmMmmm..........
.......mmMMMmm..........
......mmmMMmmmm.........
......mmMMMMMmm.........
......mmmMmmmmm.........
.......mmmmmmm..........
.......mmmmmm...........
........mmmm............
.........mm.............
........................
........................
........................
........................
........................
........................
"""


def death_burst_sheet() -> Canvas:
    c = Canvas(24 * 6, 24)
    for i, art in enumerate([DEATH_F0, DEATH_F1, DEATH_F2, DEATH_F3, DEATH_F4, DEATH_F5]):
        paint(c, i * 24, 0, art, side="p0")
    return c


# ===========================================================================
# Recall pulse — 4 frames, 16x16 each. Inward blue ticks, never a wash.
# ===========================================================================

RECALL_F0 = """
................
................
......rrrr......
.....rrrrrr.....
....rr....rr....
....r......r....
...r........r...
...r........r...
...r........r...
...r........r...
....r......r....
....rr....rr....
.....rrrrrr.....
......rrrr......
................
................
"""

RECALL_F1 = """
................
................
................
.......rr.......
......rrrr......
......r..r......
.....r....r.....
.....r....r.....
.....r....r.....
.....r....r.....
......r..r......
......rrrr......
.......rr.......
................
................
................
"""

RECALL_F2 = """
................
................
................
................
.......rr.......
.......rr.......
......r..r......
......r..r......
......r..r......
......r..r......
.......rr.......
.......rr.......
................
................
................
................
"""

RECALL_F3 = """
................
................
................
................
................
................
.......rr.......
.......rr.......
.......rr.......
.......rr.......
................
................
................
................
................
................
"""


def recall_pulse_sheet() -> Canvas:
    c = Canvas(16 * 4, 16)
    for i, art in enumerate([RECALL_F0, RECALL_F1, RECALL_F2, RECALL_F3]):
        paint(c, i * 16, 0, art, side="p0")
    return c


# ===========================================================================
# Ambient embers — 4 frames, 8x8 each. Isolated 1-3px chunks, dark enough
# never to compete with the boomerang trail.
# ===========================================================================

EMBER_F0 = """
........
........
....e...
...eee..
...emm..
....m...
........
........
"""

EMBER_F1 = """
........
....e...
...eee..
....e...
...m....
........
........
........
"""

EMBER_F2 = """
....e...
...eee..
....e...
........
........
........
........
........
"""

EMBER_F3 = """
....e...
....e...
........
........
........
........
........
........
"""


def ember_sheet() -> Canvas:
    """3 rows x 4 frames @ 8x8: row 0 the ember mote (Anchor / the Pit),
    row 1 cold dust (Crossing / Vigil / Gallery), row 2 the grove spore
    (Reliquary / Forest). Rows 1-2 are exact recolors of row 0 so the
    drift/flicker animation reads identically in every register."""
    c = Canvas(8 * 4, 8 * 3)
    for i, art in enumerate([EMBER_F0, EMBER_F1, EMBER_F2, EMBER_F3]):
        paint(c, i * 8, 0, art, side="p0")
    cold = {PALETTE["ember"]: PALETTE["cold_stone"], PALETTE["spark"]: PALETTE["bone"]}
    spore = {PALETTE["ember"]: PALETTE["deep_teal"], PALETTE["spark"]: PALETTE["bone"]}
    w = 8 * 4
    for y in range(8):
        for x in range(w):
            px = c.pixels[y * w + x]
            c.pixels[(y + 8) * w + x] = cold.get(px, px)
            c.pixels[(y + 16) * w + x] = spore.get(px, px)
    return c


# ===========================================================================
# Floor stains — three intensities per side + corpse mark. 16x16 source.
# Render-only state; persists till round reset; never feeds sim.
# ===========================================================================

STAIN_SMALL = """
................
................
................
.......m........
......mmm.......
.....mmMmm......
......mmm.......
.......m........
................
................
................
................
................
................
................
................
"""

STAIN_MED = """
................
................
.......m........
.....mmmm.......
....mmMMm.......
....mmmMmm......
.....mMMmm......
......mmm.......
.....m..mm......
.......m........
................
................
................
................
................
................
"""

STAIN_HEAVY = """
................
......m.........
.....mm.m.......
....mmMmm.......
...mmMMmmm......
...mmMMMmmm.....
...mmMMmmmm.m...
....mmMmmmm.....
.....mmMmm......
......mmm.m.....
.....m..mm......
....m...m.......
.................
.....m..........
................
................
"""

CORPSE_MARK = """
................
................
....mmm..mmm....
...mmmmmmmmm....
..mmmMMmMMMmm...
..mmMMMMMmMmm...
...mmMMmMMMmm...
...mmmMMMMmm....
....mmMMMmm.....
.....mmmmm......
......m.m.......
......m.m.......
.....m...m......
....m.....m.....
...m.......m....
................
"""


def render_stain(art: str, side: str) -> Canvas:
    c = Canvas(16, 16)
    paint(c, 0, 0, art, side=side)
    return c


def stain_sheet(side: str) -> Canvas:
    """3-stain strip + corpse mark = 4 cells, 16x16 each."""
    c = Canvas(16 * 4, 16)
    for i, art in enumerate([STAIN_SMALL, STAIN_MED, STAIN_HEAVY, CORPSE_MARK]):
        paint(c, i * 16, 0, art, side=side)
    return c


# ===========================================================================
# Arena tiles — 16x16 source. Floor primary/secondary make the checker;
# wall edges + corners frame the arena; spawn marks anchor each player;
# duel-diamond corners mark center geometry.
# ===========================================================================

FLOOR_PRIMARY = """
oooooooooooooooo
oooooooooooooooo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oodddddddddddddo
oooooooooooooooo
oooooooooooooooo
"""

FLOOR_SECONDARY = """
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
ooooddddddddoooo
oooodddooodddooo
oooooddooooooooo
oooooddooodooooo
oooooooooddooooo
oooodoooooooooooo
oooooddooodooooo
ooooddddooooooooo
oooodddoodddoooo
ooooddddddddoooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
"""

WALL_EDGE_TOP = """
kkkkkkkkkkkkkkkk
kkkkkkkkkkkkkkkk
wwwwwwwwwwwwwwww
wwbbwwbbwwbbwwbb
wwbbwwbbwwbbwwbb
wwwwwwwwwwwwwwww
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
"""

WALL_EDGE_BOTTOM = """
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
oooooooooooooooo
wwwwwwwwwwwwwwww
wwbbwwbbwwbbwwbb
wwbbwwbbwwbbwwbb
wwwwwwwwwwwwwwww
kkkkkkkkkkkkkkkk
kkkkkkkkkkkkkkkk
"""

WALL_EDGE_LEFT = """
kkwwwwoooooooooo
kkwwwwoooooooooo
kkwbbwoooooooooo
kkwbbwoooooooooo
kkwwwwoooooooooo
kkwwwwoooooooooo
kkwbbwoooooooooo
kkwbbwoooooooooo
kkwwwwoooooooooo
kkwwwwoooooooooo
kkwbbwoooooooooo
kkwbbwoooooooooo
kkwwwwoooooooooo
kkwwwwoooooooooo
kkwbbwoooooooooo
kkwbbwoooooooooo
"""

WALL_EDGE_RIGHT = """
ooooooooooowwwkk
ooooooooooowwwkk
ooooooooooowbwkk
ooooooooooowbwkk
ooooooooooowwwkk
ooooooooooowwwkk
ooooooooooowbwkk
ooooooooooowbwkk
ooooooooooowwwkk
ooooooooooowwwkk
ooooooooooowbwkk
ooooooooooowbwkk
ooooooooooowwwkk
ooooooooooowwwkk
ooooooooooowbwkk
ooooooooooowbwkk
"""

WALL_CORNER_TL = """
kkkkkkkkkkkkkkkk
kkkkkkkkkkkkkkkk
kkkkwwwwwwwwwwww
kkkkwbbwwbbwwbbw
kkkkwbbwwbbwwbbw
kkkkwwwwwwwwwwww
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
"""

WALL_CORNER_TR = """
kkkkkkkkkkkkkkkk
kkkkkkkkkkkkkkkk
wwwwwwwwwwwwkkkk
wbbwwbbwwbbwkkkk
wbbwwbbwwbbwkkkk
wwwwwwwwwwwwkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
"""

WALL_CORNER_BL = """
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkw...........
kkkkwwwwwwwwwwww
kkkkwbbwwbbwwbbw
kkkkwbbwwbbwwbbw
kkkkwwwwwwwwwwww
kkkkkkkkkkkkkkkk
kkkkkkkkkkkkkkkk
"""

WALL_CORNER_BR = """
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
...........wkkkk
wwwwwwwwwwwwkkkk
wbbwwbbwwbbwkkkk
wbbwwbbwwbbwkkkk
wwwwwwwwwwwwkkkk
kkkkkkkkkkkkkkkk
kkkkkkkkkkkkkkkk
"""

SPAWN_MARK_P0 = """
................
................
................
....llllllll....
...lmmmmmmmml...
..lmmMMMMMMmml..
..lmMMMMMMMMml..
..lmMMmmmmMMml..
..lmMMmmmmMMml..
..lmMMMMMMMMml..
..lmMMMMMMMMml..
..lmmMMMMMMmml..
...lmmmmmmmml...
....llllllll....
................
................
"""

SPAWN_MARK_P1 = """
................
................
................
....llllllll....
...lccccccccl...
..lccCCCCCCccl..
..lcCCCCCCCCcl..
..lcCCccccCCcl..
..lcCCccccCCcl..
..lcCCCCCCCCcl..
..lcCCCCCCCCcl..
..lccCCCCCCccl..
...lccccccccl...
....llllllll....
................
................
"""

DUEL_DIAMOND_CORNER = """
ldddddddddddddo
ldddddddddddddo
ldddddddddddddo
lddddddddddddo.
lddddddddlddddo
lddddddlllddddo
lddddlllllddddo
lddlllllllddddo
lllllldlllddddo
ldddddlllddddo.
lddddddlllddddo
ldddddddlddddo.
lddddddddddddo.
ldddddddddddoo.
ldddddddddddoo.
ooooooooooooooo
"""


def _tile(art: str) -> Canvas:
    c = Canvas(16, 16)
    paint(c, 0, 0, art, side="p0")
    return c


def arena_tile_sheet() -> Canvas:
    """Single sheet with all 12 arena tiles (3 rows x 4 cols)."""
    tiles = [
        FLOOR_PRIMARY, FLOOR_SECONDARY, SPAWN_MARK_P0, SPAWN_MARK_P1,
        WALL_EDGE_TOP, WALL_EDGE_BOTTOM, WALL_EDGE_LEFT, WALL_EDGE_RIGHT,
        WALL_CORNER_TL, WALL_CORNER_TR, WALL_CORNER_BL, WALL_CORNER_BR,
    ]
    c = Canvas(64, 48)
    for i, art in enumerate(tiles):
        col = i % 4
        row = i // 4
        paint(c, col * 16, row * 16, art, side="p0")
    return c


_PALETTE_RGB: "list[tuple[int, int, int]] | None" = None


def _nearest_palette(rgba: tuple[int, int, int, int]) -> tuple[int, int, int, int]:
    """Snap an RGBA tuple to the nearest of the 16 locked colors (alpha kept).
    Keeps procedural blends (e.g. the floor vignette) inside the palette lock —
    a smooth gradient isn't pixel art anyway; stepping to palette reads cleaner.
    Enforced by scripts/check_palette.py."""
    global _PALETTE_RGB
    if _PALETTE_RGB is None:
        _PALETTE_RGB = [v[:3] for v in PALETTE.values() if v[3] > 0]
    r, g, b, a = rgba
    nr, ng, nb = min(
        _PALETTE_RGB, key=lambda c: (c[0] - r) ** 2 + (c[1] - g) ** 2 + (c[2] - b) ** 2
    )
    return (nr, ng, nb, a)


# Hue-shifted ramps within the locked 16 (dark -> light). `_ramp_shade()` reads
# a finished sprite, finds each material's rim, and steps the lit (top/left) rim
# one toward LIGHT and the shadow (bottom/right) rim one toward DARK — the same
# committed-light hue-shift the duelists get, applied to any flat asset so the
# whole set reads as one hand. Thin/isolated pixels (<=1 solid neighbor) are
# left alone so 1px linework isn't smeared. Colors not in a ramp are untouched.
_RAMPS_NAMED = [
    ["blood_dark", "p0_blood", "ember", "spark"],   # heat / red
    ["deep_teal", "p1_cyan", "hit_white"],          # cool / cyan
    ["warm_bone_shade", "bone", "hot_bone"],        # bone
    ["deep_ash", "charcoal_line", "cold_stone"],    # stone
]
_RAMP_INDEX: "dict[tuple[int, int, int], tuple[int, int]] | None" = None


def _ramp_shade(c: Canvas) -> None:
    global _RAMP_INDEX
    if _RAMP_INDEX is None:
        _RAMP_INDEX = {}
        for ri, names in enumerate(_RAMPS_NAMED):
            for i, n in enumerate(names):
                _RAMP_INDEX[PALETTE[n][:3]] = (ri, i)
    W, H, px = c.width, c.height, c.pixels
    nbr = ((1, 0), (-1, 0), (0, 1), (0, -1))

    def solid(x: int, y: int) -> bool:
        return 0 <= x < W and 0 <= y < H and px[y * W + x][3] > 0

    out = px[:]
    for y in range(H):
        for x in range(W):
            r, g, b, a = px[y * W + x]
            if a == 0 or (r, g, b) not in _RAMP_INDEX:
                continue
            if sum(solid(x + dx, y + dy) for dx, dy in nbr) <= 1:
                continue  # thin/isolated — leave it
            ri, i = _RAMP_INDEX[(r, g, b)]
            ramp = _RAMPS_NAMED[ri]
            top = not solid(x, y - 1)
            left = not solid(x - 1, y)
            bot = not solid(x, y + 1)
            right = not solid(x + 1, y)
            if top or left:
                ni = min(i + 1, len(ramp) - 1)
            elif bot or right:
                ni = max(i - 1, 0)
            else:
                continue
            if ni != i:
                nc = PALETTE[ramp[ni]]
                out[y * W + x] = (nc[0], nc[1], nc[2], a)
    c.pixels = out


# Per-arena hue registers. The base atmosphere + grout shifts into a
# distinct dark register per arena; the compositions then paint their
# features on top. (In the retint era this swap WAS the whole visual
# difference between arena floors.)
_ARENA_FLOOR_SWAPS = {
    "anchor": {},
    # Crossing — colder: warm-bone accents + wall band shift to cold stone.
    "crossing": {"warm_bone_shade": "cold_stone"},
    # Reliquary — warm-dead: deep-ash field shifts to bruise shadow, warm-bone
    # accents glow deep teal (sealed-temple light).
    "reliquary": {"deep_ash": "bruise_shadow", "warm_bone_shade": "deep_teal"},
    # The Pit — the walled box reads bruised and closed-in: the field drops a
    # register and the old-blood details run hotter (the ring remembers).
    "pit": {"deep_ash": "bruise_shadow", "blood_dark": "ember"},
    # The Vigil — the no-storm arena is the coldest room in the cathedral:
    # patient teal light, stone accents.
    "vigil": {"warm_bone_shade": "cold_stone", "blood_dark": "deep_teal"},
    # The Gallery — corridors of charcoal: the grout lines dominate and the
    # accents go bone-pale (a museum of angles).
    "gallery": {"blood_dark": "bruise_shadow"},
    # The Forest — the grove floor runs mossy: old blood veins read as
    # deep-teal undergrowth beneath the bone trees.
    "forest": {"blood_dark": "deep_teal"},
}


# ---- Shared floor machinery ------------------------------------------------
# Extracted from the original training_floor so seven real compositions can
# share the atmosphere without seven copies of the dither loop. The floor
# discipline is unchanged: tier-6 quiet, features in muted registers, the
# duelists and the fang stay the brightest things on the table.

_FLOOR_W, _FLOOR_H = 320, 480
_FLOOR_BAYER = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]


def _floor_dhash(x: int, y: int) -> int:
    return ((x * 73856093) ^ (y * 19349663)) & 0xFF


def _world_px(x_cm: float) -> int:
    """World x (cm, arena half-width 500) -> floor-canvas x."""
    return round((x_cm + 500.0) * 0.32)


def _world_py(y_cm: float) -> int:
    """World y (cm) -> floor-canvas y. Texture row 0 is the FAR (+y) court
    edge (see the app's crumble_arena_floor strip mapping)."""
    return round((750.0 - y_cm) * 0.32)


def _floor_atmosphere(c: Canvas, pools: "list[tuple[int, int, float, float, tuple]]") -> None:
    """The dithered void->ash->bruise gradient with soft light pools and
    sparse dark grain. `pools` = [(cx, cy, radius, gain, color)] — the lit
    spots that make the floor read as a surface under light."""
    W, H = _FLOOR_W, _FLOOR_H
    void = PALETTE["void"]
    ash = PALETTE["deep_ash"]
    bruise = PALETTE["bruise_shadow"]
    for y in range(H):
        t = y / H
        if t < 0.55:
            a, b, f = void, ash, t / 0.55
        else:
            a, b, f = ash, bruise, (t - 0.55) / 0.45
        for x in range(W):
            col = [a[i] + (b[i] - a[i]) * f for i in range(3)]
            for (px, py, rad, gain, tone) in pools:
                d = math.hypot(x - px, y - py) / rad
                if d < 1.0:
                    g = (1.0 - d) * gain
                    col = [col[i] + (tone[i] - col[i]) * g for i in range(3)]
            if _floor_dhash(x // 4, y // 4) < 9:
                col = [col[i] * 0.84 for i in range(3)]
            thr = (_FLOOR_BAYER[y & 3][x & 3] - 7.5) * 1.9
            c.pixels[y * W + x] = _nearest_palette(
                (int(col[0] + thr), int(col[1] + thr), int(col[2] + thr), 255)
            )


def _floor_grout(c: Canvas, pitch_x: int = 32, pitch_y: int = 32) -> None:
    bruise = PALETTE["bruise_shadow"]
    for gx in range(0, _FLOOR_W, pitch_x):
        for y in range(_FLOOR_H):
            c.set(gx, y, bruise)
    for gy in range(0, _FLOOR_H, pitch_y):
        for x in range(_FLOOR_W):
            c.set(x, gy, bruise)


def _floor_register(c: Canvas, arena: str) -> None:
    """Shift the base into the arena's dark hue register (the old retint,
    now applied to the base only — features paint their own colors on top)."""
    swap = {PALETTE[k]: PALETTE[v] for k, v in _ARENA_FLOOR_SWAPS[arena].items()}
    if swap:
        c.pixels = [swap.get(px, px) for px in c.pixels]


def _floor_edge_void(c: Canvas) -> None:
    """Ledge over the void: drop face, edge vignette, lit lip — the
    open-island read (safe ground simply ENDS)."""
    W, H = _FLOOR_W, _FLOOR_H
    void = PALETTE["void"]
    wbs = PALETTE["warm_bone_shade"]
    bone = PALETTE["bone"]
    drop = 10
    c.rect(0, 0, W, drop, void)
    c.rect(0, H - drop, W, drop, void)
    c.rect(0, 0, drop, H, void)
    c.rect(W - drop, 0, drop, H, void)
    for y in range(H):
        for x in range(W):
            edge = min(x, W - 1 - x, y, H - 1 - y)
            if edge < 30:
                px = c.pixels[y * W + x]
                f = (30 - edge) / 30 * 0.8
                blended = tuple(round(px[i] * (1 - f) + void[i] * f) for i in range(4))
                c.pixels[y * W + x] = _nearest_palette(blended)
    for x in range(drop, W - drop):
        c.set(x, drop, wbs)
        c.set(x, H - drop - 1, bone)
    for y in range(drop, H - drop):
        c.set(drop, y, wbs)
        c.set(W - drop - 1, y, wbs)


def _floor_edge_walled(c: Canvas) -> None:
    """The Pit's rim: the island edge is a BUILT wall, not a drop. Nothing
    falls out of this arena — the boundary ricochets fangs and contains
    duelists — so the floor must not lie with a void lip. Stone courses,
    charcoal seams, and ember gouges where fangs have struck the ring."""
    W, H = _FLOOR_W, _FLOOR_H
    cold = PALETTE["cold_stone"]
    char = PALETTE["charcoal_line"]
    bruise = PALETTE["bruise_shadow"]
    ember = PALETTE["ember"]
    band = 10
    for y in range(H):
        for x in range(W):
            edge = min(x, W - 1 - x, y, H - 1 - y)
            if edge < band:
                # Stone courses in the dark register — a wall, not a frame.
                course = (edge // 3) & 1
                col = bruise if course == 0 else char
                # Seams between blocks along the run of the wall.
                run = x if edge in (y, H - 1 - y) else y
                if run % 16 < 1:
                    col = PALETTE["void"]
                # Sparse worn highlights on the outermost course only.
                if edge < 3 and _floor_dhash(x, y) < 28:
                    col = cold
                # Ember gouges — sparse strike memory on the inner course.
                if edge >= band - 3 and _floor_dhash(x, y) < 5:
                    col = ember
                c.pixels[y * W + x] = col
            elif edge < band + 8:
                # Contact shadow where the wall meets the floor.
                if _floor_dhash(x, y) < 120:
                    px = c.pixels[y * W + x]
                    blended = tuple(
                        round(px[i] * 0.72 + PALETTE["void"][i] * 0.28) for i in range(4)
                    )
                    c.pixels[y * W + x] = _nearest_palette(blended)


def _floor_ring(c: Canvas, cx: int, cy: int, r: float, color, step: int = 5,
                broken: bool = False) -> None:
    for a in range(0, 360, step):
        if broken and _floor_dhash(a, int(r)) < 90:
            continue
        c.set(round(cx + r * math.cos(math.radians(a))),
              round(cy + r * math.sin(math.radians(a))), color)


def _floor_broken_line(c: Canvas, x0: int, y0: int, x1: int, y1: int, color,
                       keep: int = 150) -> None:
    """A dithered line — Bresenham with hash-skipped pixels, for marks that
    must read as grown or worn (roots, wear) rather than drawn."""
    dx, dy = abs(x1 - x0), -abs(y1 - y0)
    sx, sy = (1 if x0 < x1 else -1), (1 if y0 < y1 else -1)
    err = dx + dy
    while True:
        if _floor_dhash(x0, y0) < keep:
            c.set(x0, y0, color)
        if x0 == x1 and y0 == y1:
            break
        e2 = 2 * err
        if e2 >= dy:
            err += dy
            x0 += sx
        if e2 <= dx:
            err += dx
            y0 += sy


def _floor_smudge(c: Canvas, sx: int, sy: int, rad: int, color) -> None:
    """The dried-blood ellipse from the original composition."""
    for a in range(0, 360, 18):
        for rr in range(rad):
            if (a + rr) % 3:
                continue
            c.set(round(sx + rr * math.cos(math.radians(a))),
                  round(sy + rr * 0.7 * math.sin(math.radians(a))), color)


def _floor_pad(c: Canvas, cx: int, cy: int, hw: int, hh: int, color) -> None:
    """Hollow footprint pad under a piece of cover — the floor remembers
    what stands on it."""
    for x in range(cx - hw, cx + hw + 1):
        c.set(x, cy - hh, color)
        c.set(x, cy + hh, color)
    for y in range(cy - hh, cy + hh + 1):
        c.set(cx - hw, y, color)
        c.set(cx + hw, y, color)


def _floor_scorch(c: Canvas, cx: int, cy: int, r: int) -> None:
    """Ember-and-char burn halo under a pyre."""
    char = PALETTE["charcoal_line"]
    ember = PALETTE["ember"]
    for a in range(0, 360, 6):
        for rr in range(r - 3, r):
            if _floor_dhash(a, rr) < 110:
                c.set(round(cx + rr * math.cos(math.radians(a))),
                      round(cy + rr * 0.8 * math.sin(math.radians(a))),
                      ember if _floor_dhash(rr, a) < 30 else char)


def _floor_spawn_marks(c: Canvas) -> None:
    """Rings on the true spawn points (0, ±300) — the depth-duel seats."""
    wbs = PALETTE["warm_bone_shade"]
    for y_cm in (300, -300):
        sx, sy = _world_px(0), _world_py(y_cm)
        for a in range(0, 360, 24):
            c.set(round(sx + 9 * math.cos(math.radians(a))),
                  round(sy + 9 * math.sin(math.radians(a))), wbs)


# ---- The seven compositions ------------------------------------------------


def _anchor_features(c: Canvas) -> None:
    """The first table: the occult duel stage. Central sigil, crate pads,
    the pyre's burn memory, old blood."""
    W, H = _FLOOR_W, _FLOOR_H
    char = PALETTE["charcoal_line"]
    wbs = PALETTE["warm_bone_shade"]
    teal = PALETTE["deep_teal"]
    bruise = PALETTE["bruise_shadow"]
    for (x0, y0, x1, y1) in [(48, 60, 78, 128), (250, 392, 226, 452), (150, 300, 168, 358)]:
        c.line(x0, y0, x1, y1, char)
    for (sx, sy, rad) in [(72, 150, 10), (250, 330, 12)]:
        _floor_smudge(c, sx, sy, rad, bruise)
    cx, cy = W // 2, H // 2
    _floor_ring(c, cx, cy, 42, teal)
    _floor_ring(c, cx, cy, 30, wbs)
    for r in range(36):
        for (sx, sy) in [(cx - r, cy - 36 + r), (cx + r, cy - 36 + r),
                         (cx - r, cy + 36 - r), (cx + r, cy + 36 - r)]:
            c.set(sx, sy, wbs)
    c.line(cx - 16, cy, cx + 16, cy, wbs)
    c.line(cx, cy - 16, cx, cy + 16, wbs)
    _floor_scorch(c, cx, cy, 14)
    for x_cm in (-280, 280):
        for y_cm in (300, -300):
            _floor_pad(c, _world_px(x_cm), _world_py(y_cm), 13, 13, char)
    _floor_spawn_marks(c)


def _crossing_features(c: Canvas) -> None:
    """The moat arena: a cracked rim where the floor ends at the chasm,
    blood run over the edge, the worn crossing lane, sigil echoes."""
    char = PALETTE["charcoal_line"]
    blood = PALETTE["blood_dark"]
    teal = PALETTE["deep_teal"]
    cold = PALETTE["cold_stone"]
    y_top = _world_py(60)   # far rim of the moat
    y_bot = _world_py(-60)  # near rim
    # Cracked rim lines just outside the moat band.
    for x in range(12, _FLOOR_W - 12):
        if _floor_dhash(x, 1) < 200:
            c.set(x, y_top - 1, char)
        if _floor_dhash(x, 2) < 200:
            c.set(x, y_bot + 1, char)
    # Blood that ran over the edge — short drips hanging into the band.
    for x in range(16, _FLOOR_W - 16, 7):
        if _floor_dhash(x, 3) < 70:
            drip = 2 + (_floor_dhash(x, 4) % 4)
            for d in range(drip):
                c.set(x, y_top + d, blood)
                c.set(_FLOOR_W - x, y_bot - d, blood)
    # The crossing lane: wear dither in an hourglass between the seats.
    for y_cm in range(-260, 261, 2):
        y = _world_py(y_cm)
        half = 6 + round(abs(y_cm) / 260.0 * 10)
        for x in range(_world_px(0) - half, _world_px(0) + half):
            if _floor_dhash(x, y) < 26:
                c.set(x, y, cold)
    # Sigil echoes under the altars (SIM_VERSION 13 seats: off the duel axis).
    for (x_cm, y_cm) in ((-230, -150), (230, 150)):
        sx, sy = _world_px(x_cm), _world_py(y_cm)
        _floor_ring(c, sx, sy, 10, teal)
        _floor_ring(c, sx, sy, 14, teal, step=15)
    # Pillar pads.
    for x_cm in (-300, 300):
        for y_cm in (210, -210):
            _floor_pad(c, _world_px(x_cm), _world_py(y_cm), 11, 24, char)
    _floor_spawn_marks(c)


def _reliquary_features(c: Canvas) -> None:
    """The sealed temple: niche arches along the far and near walls, teal
    door thresholds on the diagonal, the chain that links the two pyres."""
    char = PALETTE["charcoal_line"]
    wbs = PALETTE["warm_bone_shade"]
    teal = PALETTE["deep_teal"]
    bruise = PALETTE["bruise_shadow"]
    # Reliquary niches: quiet arch outlines along the short walls.
    for x in range(40, _FLOOR_W - 39, 40):
        for y in (16, _FLOOR_H - 17):
            for a in range(0, 181, 20):
                c.set(round(x + 6 * math.cos(math.radians(a))),
                      round(y + 4 * math.sin(math.radians(a))) if y < 100
                      else round(y - 4 * math.sin(math.radians(a))), wbs)
    # Door thresholds (350,-550) / (-350,550): double teal frames + ticks.
    for (x_cm, y_cm) in ((350, -550), (-350, 550)):
        dx, dy = _world_px(x_cm), _world_py(y_cm)
        _floor_pad(c, dx, dy, 12, 12, teal)
        _floor_pad(c, dx, dy, 15, 15, bruise)
        for (tx, ty) in ((-18, 0), (18, 0), (0, -18), (0, 18)):
            c.set(dx + tx, dy + ty, teal)
    # The chain: a dashed line linking the two chained pyres at (±200, 0).
    py = _world_py(0)
    for x in range(_world_px(-200) + 10, _world_px(200) - 9):
        if x % 8 < 4:
            c.set(x, py, char)
    for x_cm in (-200, 200):
        _floor_scorch(c, _world_px(x_cm), py, 12)
    # Bar pads.
    for (x_cm, y_cm, hw, hh) in ((0, 180, 21, 10), (0, -180, 21, 10),
                                 (-330, 0, 10, 21), (330, 0, 10, 21)):
        _floor_pad(c, _world_px(x_cm), _world_py(y_cm), hw, hh, char)
    c.line(60, 84, 84, 130, char)
    _floor_smudge(c, 236, 120, 9, bruise)
    _floor_spawn_marks(c)


def _pit_features(c: Canvas) -> None:
    """The fight ring: concentric wear from years of circling, radial
    gouges, heavy blood history, ember flecks near the centre."""
    W, H = _FLOOR_W, _FLOOR_H
    char = PALETTE["charcoal_line"]
    blood = PALETTE["blood_dark"]
    ember = PALETTE["ember"]
    bruise = PALETTE["bruise_shadow"]
    cx, cy = W // 2, H // 2
    for r in (40, 80, 120):
        _floor_ring(c, cx, cy, r, char, step=3, broken=True)
    # Radial gouges — short strike lines the ring remembers.
    for i in range(10):
        a = i * 36 + (_floor_dhash(i, 7) % 18)
        r0 = 30 + (_floor_dhash(i, 11) % 70)
        x0 = round(cx + r0 * math.cos(math.radians(a)))
        y0 = round(cy + r0 * math.sin(math.radians(a)))
        x1 = round(cx + (r0 + 14) * math.cos(math.radians(a)))
        y1 = round(cy + (r0 + 14) * math.sin(math.radians(a)))
        c.line(x0, y0, x1, y1, char)
    for (sx, sy, rad) in [(128, 208, 14), (196, 276, 13), (110, 300, 8), (222, 176, 9)]:
        _floor_smudge(c, sx, sy, rad, blood)
    for y in range(cy - 120, cy + 120):
        for x in range(cx - 120, cx + 120):
            if math.hypot(x - cx, y - cy) < 118 and _floor_dhash(x, y) < 3:
                c.set(x, y, ember if _floor_dhash(y, x) < 100 else bruise)
    for x_cm in (-160, 160):
        _floor_pad(c, _world_px(x_cm), _world_py(0), 13, 13, char)
    _floor_spawn_marks(c)


def _vigil_features(c: Canvas) -> None:
    """The patient room: pristine long flagstones, the wide vigil circle
    binding the two pyres, candle specks. No cracks — nothing has ever
    been allowed to break here."""
    W, H = _FLOOR_W, _FLOOR_H
    teal = PALETTE["deep_teal"]
    bone = PALETTE["bone"]
    cx, cy = W // 2, H // 2
    _floor_ring(c, cx, cy, 110, teal, step=4, broken=True)
    _floor_ring(c, cx, cy, 116, teal, step=9, broken=True)
    for x_cm in (-220, 220):
        px, py = _world_px(x_cm), _world_py(0)
        _floor_scorch(c, px, py, 12)
        for i in range(14):
            ox = (_floor_dhash(i, x_cm) % 33) - 16
            oy = (_floor_dhash(x_cm, i) % 33) - 16
            if abs(ox) + abs(oy) > 8:
                c.set(px + ox, py + oy, bone)
    _floor_spawn_marks(c)


def _gallery_features(c: Canvas) -> None:
    """The museum of angles: parquet checker, corridor runners along the
    rails, plinth pads with a bone tick — exhibits, labelled."""
    W = _FLOOR_W
    char = PALETTE["charcoal_line"]
    bone = PALETTE["bone"]
    bruise = PALETTE["bruise_shadow"]
    # Parquet: darken alternate 32px cells with sparse dither.
    for cyc in range(0, _FLOOR_H // 32 + 1):
        for cxc in range(0, W // 32 + 1):
            if (cxc + cyc) & 1:
                for y in range(cyc * 32 + 1, min((cyc + 1) * 32, _FLOOR_H)):
                    for x in range(cxc * 32 + 1, min((cxc + 1) * 32, W)):
                        if _floor_dhash(x, y) < 34:
                            c.set(x, y, bruise)
    # Corridor runners: bone-pale hairlines along the rail edges.
    for x_cm in (-240, 240):
        px = _world_px(x_cm)
        for y in range(_world_py(220), _world_py(-220)):
            if _floor_dhash(1, y) < 190:
                c.set(px - 13, y, bone)
                c.set(px + 13, y, bone)
    # Pads: rails, bars, corner plinths (with the bone exhibit tick).
    for (x_cm, y_cm, hw, hh, tick) in (
        (-240, 0, 9, 61, False), (240, 0, 9, 61, False),
        (0, 180, 38, 8, False), (0, -180, 38, 8, False),
        (-330, 480, 14, 14, True), (330, 480, 14, 14, True),
        (-330, -480, 14, 14, True), (330, -480, 14, 14, True),
    ):
        px, py = _world_px(x_cm), _world_py(y_cm)
        _floor_pad(c, px, py, hw, hh, char)
        if tick:
            c.line(px - 3, py + hh + 3, px + 3, py + hh + 3, bone)
    _floor_spawn_marks(c)


def _forest_features(c: Canvas) -> None:
    """The grove: root veins wandering out of each cluster, moss beds,
    leaf litter, a root flare under every tree."""
    W, H = _FLOOR_W, _FLOOR_H
    char = PALETTE["charcoal_line"]
    teal = PALETTE["deep_teal"]
    wbs = PALETTE["warm_bone_shade"]
    trees = [(-340, 500), (-220, 460), (-300, 360),
             (340, -500), (220, -460), (300, -360),
             (380, 120), (430, -10), (-380, -120), (-430, 10),
             (-90, 40), (90, -40)]
    # Root veins: two short broken wanderers out of each tree — grown, not
    # drawn; solid teal polylines read as lightning, not roots.
    for ti, (x_cm, y_cm) in enumerate(trees):
        px, py = _world_px(x_cm), _world_py(y_cm)
        for vi in range(2):
            a = _floor_dhash(ti, vi) * 360 // 256
            x, y = float(px), float(py)
            for seg in range(3):
                jitter = (_floor_dhash(ti * 7 + seg, vi) % 60) - 30
                aa = math.radians(a + jitter)
                nx = x + math.cos(aa) * (5 + seg * 2)
                ny = y + math.sin(aa) * (5 + seg * 2)
                _floor_broken_line(c, round(x), round(y), round(nx), round(ny), teal, keep=140)
                x, y = nx, ny
        # Root flare: four diagonal ticks at the trunk.
        for (dx, dy) in ((-1, -1), (1, -1), (-1, 1), (1, 1)):
            c.line(px + dx * 5, py + dy * 5, px + dx * 9, py + dy * 9, char)
    # Moss beds around the two big clusters + centre singles.
    for (x_cm, y_cm) in ((-287, 440), (287, -440), (0, 0)):
        mx, my = _world_px(x_cm), _world_py(y_cm)
        for y in range(my - 22, my + 22):
            for x in range(mx - 26, mx + 26):
                if math.hypot(x - mx, y - my) < 23 and _floor_dhash(x, y) < 30:
                    c.set(x, y, teal)
    # Leaf litter, field-wide and sparse.
    for y in range(14, H - 14, 3):
        for x in range(14, W - 14, 3):
            if _floor_dhash(x + 1, y + 2) < 2:
                c.set(x, y, wbs)
    _floor_spawn_marks(c)


_ARENA_FLOOR_FEATURES = {
    "anchor": _anchor_features,
    "crossing": _crossing_features,
    "reliquary": _reliquary_features,
    "pit": _pit_features,
    "vigil": _vigil_features,
    "gallery": _gallery_features,
    "forest": _forest_features,
}

# Atmosphere light pools per arena (canvas coords). Everyone else gets the
# single central stage pool; the Vigil's light lives on its two pyres and
# the Gallery runs dimmer, lit along the corridor crossings.
_ARENA_FLOOR_POOLS = {
    "vigil": [(90, 240, 130, 0.12, PALETTE["cold_stone"]),
              (230, 240, 130, 0.12, PALETTE["cold_stone"])],
    "gallery": [(160, 120, 120, 0.14, PALETTE["cold_stone"]),
                (160, 360, 120, 0.14, PALETTE["cold_stone"])],
    "pit": [(160, 240, 150, 0.26, PALETTE["cold_stone"])],
}
_DEFAULT_FLOOR_POOL = [(160, 240, 175, 0.22, PALETTE["cold_stone"])]

# Grout pitch per arena: the Vigil lays long patient flagstones, the
# Gallery a tight parquet grid.
_ARENA_FLOOR_GROUT = {"vigil": (40, 64), "gallery": (32, 32)}


def arena_floor(arena: str = "anchor") -> Canvas:
    """One real composition per arena (SIM_VERSION 13 batch — this replaced
    the seven-retints-of-one-floor era). Shared atmosphere, per-arena hue
    register (`_ARENA_FLOOR_SWAPS`), then the arena's own features: the
    floor now states each arena's rules (the Pit's walled rim, the moat's
    cracked lip, the Vigil's pyre light) instead of just recoloring them."""
    c = Canvas(_FLOOR_W, _FLOOR_H, PALETTE["deep_ash"])
    _floor_atmosphere(c, _ARENA_FLOOR_POOLS.get(arena, _DEFAULT_FLOOR_POOL))
    gx, gy = _ARENA_FLOOR_GROUT.get(arena, (32, 32))
    _floor_grout(c, gx, gy)
    _floor_register(c, arena)
    _ARENA_FLOOR_FEATURES[arena](c)
    if arena == "pit":
        _floor_edge_walled(c)
    else:
        _floor_edge_void(c)
    return c


def training_floor() -> Canvas:
    """The training slab is the Anchor stage — they were byte-identical in
    the retint era and staying aliased keeps it honest."""
    return arena_floor("anchor")


# ===========================================================================
# Bone pyre — Phase 16 arena cover. 3 cells @ 32x32: intact / cracked /
# shattered-rubble. Kept muted (warm-bone dominant, small ember eye glints)
# so the cover stays below players + boomerang in the readability hierarchy.
# ===========================================================================

def _pyre_cell(stage: int) -> Canvas:
    c = Canvas(32, 32)
    bone = PALETTE["bone"]
    wbs = PALETTE["warm_bone_shade"]
    void = PALETTE["void"]
    char = PALETTE["charcoal_line"]
    ember = PALETTE["ember"]
    dark = PALETTE["bruise_shadow"]
    cx = 16

    if stage < 2:
        # Base rubble.
        c.rect(cx - 9, 26, 18, 5, dark)
        c.rect(cx - 8, 25, 16, 2, wbs)
        # Stacked skulls (muted bone, right-side shade, void eye sockets).
        skulls = [(cx, 21, 5), (cx - 4, 16, 4), (cx + 4, 16, 4), (cx - 1, 10, 5)]
        for (sx, sy, r) in skulls:
            c.rect(sx - r, sy - r, 2 * r, 2 * r, wbs)
            c.rect(sx - r, sy - r, 2 * r - 1, r, bone)  # lit top-left
            for yy in range(sy - r, sy + r):
                c.set(sx + r - 1, yy, dark)
            c.set(sx - r + 1, sy, void)
            c.set(sx + r - 2, sy, void)
        # Top skull's burning eyes.
        c.set(cx - 2, 9, ember)
        c.set(cx + 1, 9, ember)
        if stage == 1:
            # Cracks spider across the stack.
            c.line(cx - 3, 11, cx + 2, 22, char)
            c.line(cx + 3, 13, cx - 1, 24, char)
            c.set(cx + 5, 14, char)
    else:
        # Collapsed rubble heap — dead, no ember.
        c.rect(cx - 10, 27, 20, 4, dark)
        c.rect(cx - 9, 25, 18, 2, wbs)
        for (sx, sy) in [(cx - 6, 24), (cx + 5, 24), (cx + 2, 22)]:
            c.rect(sx, sy, 3, 2, bone)
        c.rect(cx - 3, 22, 6, 4, wbs)
        c.set(cx - 2, 23, void)
        c.set(cx + 1, 23, void)
        c.line(cx - 3, 22, cx + 2, 26, char)
    _ramp_shade(c)  # directional bone form (kept muted — bone ramp tops at hot-bone only on rims)
    return c


def bone_pyre_sheet() -> Canvas:
    """3-cell strip: intact / cracked / shattered-rubble (32x32 each)."""
    c = Canvas(96, 32)
    for i in range(3):
        c.blit(_pyre_cell(i), i * 32, 0, 1)
    return c


# ===========================================================================
# Bone tree — the Forest's living cover. 3 cells @ 32x32: standing /
# burning / felled stump. A gnarled dead tree whose branches fork like
# antlers (the Cur/Stag motif grown into flora). Muted bone over dark so
# players + fangs stay the readable foreground; the burning cell carries
# ember tips that the render layer overdrives into the bloom.
# ===========================================================================

def _tree_cell(stage: int) -> Canvas:
    c = Canvas(32, 32)
    bone = PALETTE["bone"]
    wbs = PALETTE["warm_bone_shade"]
    void = PALETTE["void"]
    char = PALETTE["charcoal_line"]
    ember = PALETTE["ember"]
    dark = PALETTE["bruise_shadow"]
    cx = 16

    # Root flare — the tree stands ON the floor band.
    c.rect(cx - 8, 27, 16, 4, dark)
    c.rect(cx - 7, 26, 14, 2, wbs)

    if stage == 2:
        # Felled: a broken shin of trunk + splinters where the crown fell.
        c.rect(cx - 3, 20, 6, 7, wbs)
        c.rect(cx - 3, 20, 2, 7, bone)
        c.line(cx - 3, 20, cx + 2, 18, char)
        for (sx, sy) in [(cx - 7, 25), (cx + 4, 24), (cx + 6, 26)]:
            c.rect(sx, sy, 3, 2, wbs)
        c.set(cx, 22, void)
    else:
        trunk_dark = char if stage == 1 else wbs
        trunk_lit = wbs if stage == 1 else bone
        # Trunk, lit up its left edge.
        c.rect(cx - 2, 12, 5, 15, trunk_dark)
        c.rect(cx - 2, 12, 2, 15, trunk_lit)
        # Antler branches.
        c.line(cx, 12, cx - 7, 4, trunk_dark)
        c.line(cx - 7, 4, cx - 10, 6, trunk_dark)
        c.line(cx, 12, cx + 6, 3, trunk_dark)
        c.line(cx + 6, 3, cx + 10, 5, trunk_dark)
        c.line(cx - 1, 16, cx - 9, 12, trunk_dark)
        c.line(cx + 2, 15, cx + 10, 11, trunk_dark)
        # Knot hollow.
        c.set(cx, 19, void)
        c.set(cx + 1, 19, void)
        if stage == 1:
            # Alight: ember tips on every branch + a lick up the trunk.
            for (fx, fy) in [
                (cx - 10, 5), (cx + 10, 4), (cx - 9, 11),
                (cx + 10, 10), (cx - 7, 3), (cx + 6, 2),
            ]:
                c.set(fx, fy, ember)
                c.set(fx, fy + 1, ember)
            c.line(cx - 1, 26, cx - 1, 20, ember)
            c.set(cx + 1, 23, ember)
    _ramp_shade(c)
    return c


def bone_tree_sheet() -> Canvas:
    """3-cell strip: standing / burning / felled stump (32x32 each)."""
    c = Canvas(96, 32)
    for i in range(3):
        c.blit(_tree_cell(i), i * 32, 0, 1)
    return c


def chasm_strip() -> Canvas:
    """Vertical blood-chasm tile, 32x64 (tiles down the Crossing band). A
    dark pit with cracked bone ledges on the left/right edges, blood-dark
    veins, and a few ember glints from the depths so it reads as deadly."""
    c = Canvas(32, 64, PALETTE["void"])
    void = PALETTE["void"]
    deep = PALETTE["deep_ash"]
    blood = PALETTE["blood_dark"]
    ember = PALETTE["ember"]
    wbs = PALETTE["warm_bone_shade"]
    char = PALETTE["charcoal_line"]
    # depth shading toward the centre — a dark pit (deep ash), not a red band,
    # so the chasm reads as DEPTH between the duelists instead of a neon stripe.
    for x in range(4, 28):
        for y in range(0, 64):
            if (x + y) % 7 == 0:
                c.set(x, y, deep)
    # a few dim blood veins — the only saturated cue, sparse.
    for y in range(0, 64, 6):
        c.set(15 + (y // 5) % 3, y, blood)
    # two faint ember glints from the depths (tile-safe positions)
    for (ex, ey) in [(16, 22), (15, 50)]:
        c.set(ex, ey, ember)
    # cracked bone ledges hugging both rims
    for y in range(0, 64):
        c.set(2, y, wbs if y % 4 else char)
        c.set(3, y, char)
        c.set(29, y, wbs if y % 4 else char)
        c.set(28, y, char)
    return c


def altar_sigil_sheet() -> Canvas:
    """2-cell strip, 32x32 each: idle (dim teal rune) / lit (bright recall-
    blue, raises the bridge). Hit by a boomerang to trigger."""
    c = Canvas(64, 32)
    for cell, (ring, glow, core) in enumerate(
        [
            (PALETTE["deep_teal"], PALETTE["cold_stone"], PALETTE["warm_bone_shade"]),
            (PALETTE["recall_blue"], PALETTE["p1_cyan"], PALETTE["hit_white"]),
        ]
    ):
        ox = cell * 32
        cx, cy = 16, 16
        # outer ring
        for a in range(0, 360, 12):
            c.set(ox + cx + round(11 * math.cos(math.radians(a))),
                  cy + round(11 * math.sin(math.radians(a))), ring)
        # inscribed triangle (occult glyph)
        pts = [(cx, cy - 9), (cx - 8, cy + 6), (cx + 8, cy + 6)]
        for i in range(3):
            x0, y0 = pts[i]
            x1, y1 = pts[(i + 1) % 3]
            c.line(ox + x0, y0, ox + x1, y1, glow)
        # core
        c.rect(ox + cx - 1, cy - 1, 2, 2, core)
    return c


def sigil_door_sheet() -> Canvas:
    """2-cell strip, 32x32 each: active (glowing recall-blue portal rune) /
    cooldown (dimmed). A stone archway framing an occult teleport rune."""
    c = Canvas(64, 32)
    stone = PALETTE["cold_stone"]
    char = PALETTE["charcoal_line"]
    dark = PALETTE["bruise_shadow"]
    for cell, (rune, glow, core) in enumerate(
        [
            (PALETTE["recall_blue"], PALETTE["p1_cyan"], PALETTE["hit_white"]),
            (PALETTE["charcoal_line"], PALETTE["deep_teal"], PALETTE["cold_stone"]),
        ]
    ):
        ox = cell * 32
        # archway frame: two pillars + a lintel
        c.rect(ox + 4, 4, 3, 24, stone)
        c.rect(ox + 25, 4, 3, 24, stone)
        c.rect(ox + 4, 4, 24, 3, stone)
        for y in range(4, 28):
            c.set(ox + 6, y, char)
            c.set(ox + 25, y, char)
        # portal interior (dark)
        c.rect(ox + 8, 7, 16, 21, dark)
        # occult rune: nested chevrons + core
        cx = ox + 16
        for i, yy in enumerate((11, 15, 19)):
            c.line(cx - 5, yy, cx, yy - 3, rune)
            c.line(cx, yy - 3, cx + 5, yy, rune)
        c.set(cx, 23, glow)
        c.set(cx, 13, core)
    return c


def bone_bridge_tile() -> Canvas:
    """Vertical bone-plank bridge tile, 32x64 — the overlay drawn across the
    chasm while a sigil bridge is raised (makes the chasm safe)."""
    c = Canvas(32, 64)
    bone = PALETTE["bone"]
    wbs = PALETTE["warm_bone_shade"]
    char = PALETTE["charcoal_line"]
    # planks running across (horizontal slats every 6px)
    for y in range(0, 64):
        plank = (y % 6) != 0
        for x in range(3, 29):
            c.set(x, y, bone if plank else char)
        if plank:
            c.set(28, y, wbs)  # right shade
    # rope rails
    for y in range(0, 64):
        c.set(3, y, wbs)
        c.set(29, y, wbs)
    return c


# ===========================================================================
# HUD — score pips, timer digits, countdown digits, match-over badge,
# touch ring states.
# ===========================================================================

PIP_FILLED = """
kkkkkkk.
kBMMMMk.
kMmMmMk.
kMMMMMk.
kMmMmMk.
kMMMMMk.
kkkkkkk.
........
"""

PIP_EMPTY = """
kkkkkkk.
kdddddk.
kdSSSdk.
kdSSSdk.
kdSSSdk.
kdddddk.
kkkkkkk.
........
"""


def pickup_sheet() -> Canvas:
    """7-cell strip, 24x24 each — one floor pickup per PickupKind in the
    sim's `as_u8` order: Fire / Heavy / Bouncy / Curve / Multishot /
    Phantom / Swap. Each is a kind-colored glyph resting on a dark stone
    plinth so it reads as an item on the arena floor. Palette-true per
    ART_DIRECTION v2: Fire=Ember, Heavy=Cold Stone, Bouncy=Spark,
    Curve=Recall Blue, Multishot=Hot Bone, Phantom=Bruise Shadow,
    Swap=P1 Cyan."""
    cell = 24
    c = Canvas(cell * 7, cell)
    char = PALETTE["charcoal_line"]
    stone = PALETTE["cold_stone"]
    dark = PALETTE["bruise_shadow"]

    def disc(ox: int, cx: int, cy: int, r: int, color) -> None:
        for yy in range(-r, r + 1):
            for xx in range(-r, r + 1):
                if xx * xx + yy * yy <= r * r:
                    c.set(ox + cx + xx, cy + yy, color)

    # Dark stone plinth at the bottom of every cell grounds the item.
    for i in range(7):
        ox = i * cell
        c.rect(ox + 5, 21, 14, 1, char)
        c.rect(ox + 6, 20, 12, 1, stone)
        c.rect(ox + 8, 19, 8, 1, dark)

    # 0 — Fire: ember flame with a spark core.
    ox = 0
    ember, spark, bd = PALETTE["ember"], PALETTE["spark"], PALETTE["blood_dark"]
    disc(ox, 12, 12, 5, ember)
    c.rect(ox + 11, 4, 3, 8, ember)
    c.line(ox + 12, 4, ox + 9, 10, ember)
    c.line(ox + 12, 4, ox + 15, 10, ember)
    disc(ox, 12, 13, 2, spark)
    c.set(ox + 12, 15, bd)

    # 1 — Heavy: a cold-stone weight block, bone top, dark base.
    ox = cell
    bone = PALETTE["bone"]
    c.rect(ox + 5, 8, 14, 9, stone)
    c.rect(ox + 5, 8, 14, 2, bone)
    c.rect(ox + 5, 15, 14, 2, char)
    for x in range(4, 20):
        c.set(ox + x, 7, char)
        c.set(ox + x, 17, char)
    for y in range(8, 17):
        c.set(ox + 4, y, char)
        c.set(ox + 19, y, char)
    c.line(ox + 9, 12, ox + 14, 12, char)

    # 2 — Bouncy: a spark ball with motion arcs.
    ox = 2 * cell
    hb = PALETTE["hot_bone"]
    disc(ox, 11, 11, 5, PALETTE["spark"])
    disc(ox, 10, 9, 2, hb)
    for ax, ay in [(18, 6), (19, 9), (18, 12)]:
        c.set(ox + ax, ay, char)

    # 3 — Curve: a recall-blue banana arc with an arrowhead.
    ox = 3 * cell
    rb, cyn = PALETTE["recall_blue"], PALETTE["p1_cyan"]
    for a in range(205, 345, 7):
        x = 11 + round(8 * math.cos(math.radians(a)))
        y = 13 + round(8 * math.sin(math.radians(a)))
        c.set(ox + x, y, rb)
        c.set(ox + x, y - 1, cyn)
    c.line(ox + 17, 5, ox + 14, 6, rb)
    c.line(ox + 17, 5, ox + 17, 9, rb)

    # 4 — Multishot: three hot-bone fangs fanning up from a base.
    ox = 4 * cell
    bx, by = 12, 17
    for tx, ty in [(6, 5), (12, 3), (18, 5)]:
        c.line(ox + bx, by, ox + tx, ty, PALETTE["hot_bone"])
        c.set(ox + tx, ty, PALETTE["bone"])
        c.set(ox + tx, ty + 1, PALETTE["bone"])

    # 5 — Phantom: a faded spectre with hollow eyes.
    ox = 5 * cell
    br, hw = PALETTE["bruise_shadow"], PALETTE["hit_white"]
    disc(ox, 12, 10, 5, br)
    c.rect(ox + 7, 10, 11, 7, br)
    for x in range(7, 18, 2):
        c.set(ox + x, 17, br)
    for y in range(5, 17):
        c.set(ox + 6, y, stone)
        c.set(ox + 17, y, stone)
    c.set(ox + 10, 11, hw)
    c.set(ox + 14, 11, hw)

    # 6 — Swap: two opposing cyan arrows trading places around a teal core.
    ox = 6 * cell
    cyn2, teal = PALETTE["p1_cyan"], PALETTE["deep_teal"]
    disc(ox, 12, 11, 2, teal)
    # Upper arrow: left-to-right.
    c.line(ox + 6, 7, ox + 16, 7, cyn2)
    c.line(ox + 16, 7, ox + 13, 4, cyn2)
    c.line(ox + 16, 7, ox + 13, 10, cyn2)
    # Lower arrow: right-to-left.
    c.line(ox + 18, 15, ox + 8, 15, cyn2)
    c.line(ox + 8, 15, ox + 11, 12, cyn2)
    c.line(ox + 8, 15, ox + 11, 18, cyn2)

    _ramp_shade(c)  # directional hue-shift form, palette-locked
    return c


def score_pips() -> Canvas:
    """3-cell 8x8 atlas: [empty, filled-P0, filled-P1]. The in-match HUD draws
    five pips per player and indexes filled/empty by `MatchScore` (a filled pip
    carries a 1px hot-bone inner highlight so it reads even in the team hue)."""
    c = Canvas(24, 8)
    paint(c, 0, 0, PIP_EMPTY, side="p0")   # 0: empty (void outline, cold-stone core)
    paint(c, 8, 0, PIP_FILLED, side="p0")  # 1: filled P0 (blood)
    paint(c, 16, 0, PIP_FILLED, side="p1")  # 2: filled P1 (cyan)
    return c


# Timer digits 0-9, 5x7 each (compact fighting-game style).
TIMER_DIGITS = {
    "0": ["..hhh.", ".h...h", ".h..hh", ".h.h.h", ".hh..h", ".h...h", "..hhh."],
    "1": ["...h..", "..hh..", ".h.h..", "...h..", "...h..", "...h..", ".hhhhh"],
    "2": ["..hhh.", ".h...h", ".....h", "....h.", "..hh..", ".h....", ".hhhhh"],
    "3": ["..hhh.", ".h...h", "....h.", "..hh..", "....h.", ".h...h", "..hhh."],
    "4": [".h..h.", ".h..h.", ".h..h.", ".hhhhh", "....h.", "....h.", "....h."],
    "5": [".hhhhh", ".h....", ".hhhh.", "....h.", "....h.", ".h...h", "..hhh."],
    "6": ["...hh.", "..h...", ".h....", ".hhhh.", ".h...h", ".h...h", "..hhh."],
    "7": [".hhhhh", "....h.", "...h..", "...h..", "..h...", "..h...", ".h...."],
    "8": ["..hhh.", ".h...h", ".h...h", "..hhh.", ".h...h", ".h...h", "..hhh."],
    "9": ["..hhh.", ".h...h", ".h...h", "..hhhh", "....h.", "...h..", ".hh..."],
}


def timer_digits() -> Canvas:
    c = Canvas(6 * 10, 7)
    for idx, d in enumerate("0123456789"):
        for y, row in enumerate(TIMER_DIGITS[d]):
            for x, ch in enumerate(row):
                if ch == "h":
                    c.set(idx * 6 + x, y, PALETTE["hit_white"])
                    if x > 0 and y > 0 and TIMER_DIGITS[d][y - 1][x] == ".":
                        c.set(idx * 6 + x, y, PALETTE["hit_white"])
    return c


# Countdown digits — 16x16 each, hot/proud, used at round start.
COUNTDOWN_3 = """
.....hhhhhh.....
....hhhhhhhh....
...hh......hh...
...hh......hh...
...........hh...
..........hhh...
.......hhhhh....
.......hhhhh....
..........hhh...
...........hh...
...........hh...
...hh......hh...
...hh......hh...
....hhhhhhhh....
.....hhhhhh.....
................
"""

COUNTDOWN_2 = """
.....hhhhhh.....
....hhhhhhhh....
...hh......hh...
...hh......hh...
...........hh...
..........hh....
.........hh.....
........hh......
.......hh.......
......hh........
.....hh.........
....hh..........
...hh...........
...hhhhhhhhhhh..
...hhhhhhhhhhh..
................
"""

COUNTDOWN_1 = """
........hhh.....
.......hhhh.....
......hh.hh.....
.....hh..hh.....
....hh...hh.....
...hh....hh.....
.........hh.....
.........hh.....
.........hh.....
.........hh.....
.........hh.....
.........hh.....
.........hh.....
....hhhhhhhhhh..
....hhhhhhhhhh..
................
"""

COUNTDOWN_G = """
.....hhhhhhh....
....hhhhhhhhh...
...hh.......hh..
...hh...........
..hh............
..hh............
..hh............
..hh....hhhhh...
..hh....hhhhh...
..hh........hh..
..hh........hh..
...hh.......hh..
...hh.......hh..
....hhhhhhhhh...
.....hhhhhhh....
................
"""

COUNTDOWN_O = """
.....hhhhhh.....
....hhhhhhhh....
...hh......hh...
..hh........hh..
..hh........hh..
..hh........hh..
..hh........hh..
..hh........hh..
..hh........hh..
..hh........hh..
..hh........hh..
..hh........hh..
...hh......hh...
....hhhhhhhh....
.....hhhhhh.....
................
"""


def countdown_digits() -> Canvas:
    c = Canvas(16 * 5, 16)
    for i, art in enumerate([COUNTDOWN_3, COUNTDOWN_2, COUNTDOWN_1, COUNTDOWN_G, COUNTDOWN_O]):
        paint(c, i * 16, 0, art, side="p0")
        # Each glyph gets a spark accent on the trailing edge — restraint
        # rule: only one accent pixel per glyph.
        c.set(i * 16 + 13, 13, PALETTE["spark"])
    return c


# Match-over badge — large enough to read at typical UI scale. 64x32, big
# letters at 5x7 with hit-white fills, charcoal-line shadow underneath.

# 5x7 letter font, hand-drawn for the badge. Only the letters used appear.
BADGE_FONT_5X7 = {
    "M": ["h...h", "hh.hh", "h.h.h", "h.h.h", "h...h", "h...h", "h...h"],
    "A": [".hhh.", "h...h", "h...h", "hhhhh", "h...h", "h...h", "h...h"],
    "T": ["hhhhh", "..h..", "..h..", "..h..", "..h..", "..h..", "..h.."],
    "C": [".hhhh", "h....", "h....", "h....", "h....", "h....", ".hhhh"],
    "H": ["h...h", "h...h", "h...h", "hhhhh", "h...h", "h...h", "h...h"],
    "O": [".hhh.", "h...h", "h...h", "h...h", "h...h", "h...h", ".hhh."],
    "V": ["h...h", "h...h", "h...h", "h...h", "h...h", ".h.h.", "..h.."],
    "E": ["hhhhh", "h....", "h....", "hhhh.", "h....", "h....", "hhhhh"],
    "R": ["hhhh.", "h...h", "h...h", "hhhh.", "h.h..", "h..h.", "h...h"],
    " ": [".....", ".....", ".....", ".....", ".....", ".....", "....."],
}


def _draw_badge_text(c: Canvas, ox: int, oy: int, text: str,
                     fill: tuple[int, int, int, int],
                     shadow: tuple[int, int, int, int]) -> None:
    cursor = ox
    for char in text:
        glyph = BADGE_FONT_5X7.get(char, BADGE_FONT_5X7[" "])
        for gy, row in enumerate(glyph):
            for gx, bit in enumerate(row):
                if bit == "h":
                    c.set(cursor + gx, oy + gy + 1, shadow)  # 1px drop shadow
                    c.set(cursor + gx, oy + gy, fill)
        cursor += 6  # 5 glyph cols + 1 spacing


def match_over_badge() -> Canvas:
    c = Canvas(80, 32, PALETTE["clear"])
    # Frame.
    for x in range(80):
        c.set(x, 0, PALETTE["bone"])
        c.set(x, 31, PALETTE["bone"])
    for y in range(32):
        c.set(0, y, PALETTE["bone"])
        c.set(79, y, PALETTE["bone"])
    # Inset.
    for x in range(2, 78):
        for y in range(2, 30):
            c.set(x, y, PALETTE["void"])
    # Title text — two lines.
    _draw_badge_text(c, 13, 4, "MATCH",
                     PALETTE["hit_white"], PALETTE["charcoal_line"])
    _draw_badge_text(c, 16, 17, "OVER",
                     PALETTE["hit_white"], PALETTE["charcoal_line"])
    # Tiny spark flair, one corner.
    c.set(76, 3, PALETTE["spark"])
    c.set(3, 28, PALETTE["spark"])
    return c


VIRTUAL_STICK_IDLE = """
.....SSSSSSS.....
...SSSSSSSSSSS...
..SS.........SS..
.SS...........SS.
.S....SSSSS....S.
SS...S.....S...SS
S....S.....S....S
S...S...S...S...S
S....S.....S....S
SS...S.....S...SS
.S....SSSSS....S.
.SS...........SS.
..SS.........SS..
...SSSSSSSSSSS...
.....SSSSSSS.....
.................
"""

VIRTUAL_STICK_ACTIVE = """
.....bbbbbbb.....
...bbbbbbbbbbb...
..bb.........bb..
.bb...........bb.
.b....bbbbb....b.
bb...bMMMMMb...bb
b....bMMMMMb....b
b...bMMMbMMMb...b
b....bMMMMMb....b
bb...bMMMMMb...bb
.b....bbbbb....b.
.bb...........bb.
..bb.........bb..
...bbbbbbbbbbb...
.....bbbbbbb.....
.................
"""

THROW_RING_IDLE = """
.....SSSSSSS.....
...SS.......SS...
..S...........S..
.S.............S.
.S.............S.
S...............S
S...............S
S.......b.......S
S...............S
S...............S
.S.............S.
.S.............S.
..S...........S..
...SS.......SS...
.....SSSSSSS.....
.................
"""

THROW_RING_ACTIVE = """
.....BBBBBBB.....
...BB.......BB...
..B...........B..
.B....bBBBb....B.
.B...bBBBBBb...B.
B...bBBBhBBBb...B
B...bBBhBhBBb...B
B...BBBhhhBBB...B
B...bBBhBhBBb...B
B...bBBBhBBBb...B
.B...bBBBBBb...B.
.B....bBBBb....B.
..B...........B..
...BB.......BB...
.....BBBBBBB.....
.................
"""


def touch_controls_sheet() -> Canvas:
    """4-cell strip: virtual stick idle/active + throw ring idle/active."""
    c = Canvas(16 * 4, 16)
    for i, art in enumerate([VIRTUAL_STICK_IDLE, VIRTUAL_STICK_ACTIVE,
                              THROW_RING_IDLE, THROW_RING_ACTIVE]):
        paint(c, i * 16, 0, art, side="p0")
    return c


# ===========================================================================
# Phase 14 cycle 2b.2 — replay viewer scrub bar.
# Bone Cathedral aesthetic, HLD discipline (no gore-revival pole — this is
# a composition-mode UI surface with no contact). Designed at native source
# resolution; the viewer scales 4x onscreen for a 768x48 chunky-pixel band.
# ===========================================================================

# 192x12 source. Top + bottom 1px charcoal-line frame, deep-ash interior,
# warm-bone-shade tick marks every 16 px (one per "second-equivalent" at
# the canonical 1800-frame round), bone center pip every 64 px to anchor
# longer-range scrubbing. Renders identically empty in the asset; the
# viewer overlays the played-portion fill as a separate hot-bone Sprite.
SCRUB_BAR_TRACK_W = 192
SCRUB_BAR_TRACK_H = 12


def scrub_bar_track() -> Canvas:
    c = Canvas(SCRUB_BAR_TRACK_W, SCRUB_BAR_TRACK_H, PALETTE["clear"])
    # Charcoal-line frame: 1 px top + bottom + 1 px caps on the sides.
    c.rect(0, 0, SCRUB_BAR_TRACK_W, 1, PALETTE["charcoal_line"])
    c.rect(0, SCRUB_BAR_TRACK_H - 1, SCRUB_BAR_TRACK_W, 1, PALETTE["charcoal_line"])
    c.rect(0, 0, 1, SCRUB_BAR_TRACK_H, PALETTE["charcoal_line"])
    c.rect(SCRUB_BAR_TRACK_W - 1, 0, 1, SCRUB_BAR_TRACK_H, PALETTE["charcoal_line"])
    # Interior fill: deep ash so the empty-track reads as cold stone
    # rather than void (void would clash with the surrounding window).
    c.rect(1, 1, SCRUB_BAR_TRACK_W - 2, SCRUB_BAR_TRACK_H - 2, PALETTE["deep_ash"])
    # Tick marks every 16 px (warm bone shade — visible but recessive).
    # Skip the first/last 8 px so the ticks don't clash with the frame caps.
    for x in range(16, SCRUB_BAR_TRACK_W - 8, 16):
        c.set(x, 4, PALETTE["warm_bone_shade"])
        c.set(x, 7, PALETTE["warm_bone_shade"])
    # Major anchor pips every 64 px (bone — brighter, used as scrub
    # landmarks for ~25%-of-round seeks).
    for x in range(64, SCRUB_BAR_TRACK_W - 32, 64):
        c.rect(x - 1, 5, 3, 2, PALETTE["bone"])
    return c


# Slider handle — 8x16, stacked-fang silhouette pointing both ways
# (vertically symmetric so it reads as a "needle" indicator rather
# than a directional fang). Hot-bone core for visibility, charcoal-line
# stroke so it sits on top of the track without bleeding.
SCRUB_BAR_HANDLE = """
...BB...
..BBBB..
.BBhhBB.
BBhhhhBB
BBhBBhBB
.lBBBBl.
.lBBBBl.
.lBBBBl.
.lBBBBl.
.lBBBBl.
.lBBBBl.
BBhBBhBB
BBhhhhBB
.BBhhBB.
..BBBB..
...BB...
"""


def scrub_bar_handle() -> Canvas:
    c = Canvas(8, 16, PALETTE["clear"])
    paint(c, 0, 0, SCRUB_BAR_HANDLE, side="p0")
    return c


# Frame-step buttons — bone-fang chevron glyphs flanking the scrub bar.
# Two cells in one 32x16 strip: cell 0 = back (left-pointing), cell 1 =
# forward (right-pointing, mirrored). Charcoal-line stroke, hot-bone
# core. Sized to match the scrub-bar handle's vertical scale so they
# read as part of the same UI cluster.

FRAME_STEP_BACK = """
.......BB.......
......BBB.......
.....BBBl.......
....BBhBl.......
...BBhhBl.......
..BBhhhBl.......
.BBhhhhBl.......
BBhhhhhBl.......
BBhhhhhBl.......
.BBhhhhBl.......
..BBhhhBl.......
...BBhhBl.......
....BBhBl.......
.....BBBl.......
......BBl.......
.......B........
"""

FRAME_STEP_FWD = """
.......BB.......
.......BBB......
.......lBBB.....
.......lBhBB....
.......lBhhBB...
.......lBhhhBB..
.......lBhhhhBB.
.......lBhhhhhBB
.......lBhhhhhBB
.......lBhhhhBB.
.......lBhhhBB..
.......lBhhBB...
.......lBhBB....
.......lBBB.....
.......lBB......
........B.......
"""


def frame_step_buttons() -> Canvas:
    c = Canvas(16 * 2, 16, PALETTE["clear"])
    paint(c, 0, 0, FRAME_STEP_BACK, side="p0")
    paint(c, 16, 0, FRAME_STEP_FWD, side="p0")
    return c

# Speed pips — intentionally NOT an asset. The viewer renders each
# pip as its own Sprite entity (colored rectangle, no texture) so
# each pip can be individually click-tested + color-tinted by the
# active-speed selector. See `spawn_speed_pips` in replay_viewer.


# ===========================================================================
# Contact sheet — review board showing every polished asset at scale.
# ===========================================================================


def contact_sheet() -> Canvas:
    bg = PALETTE["void"]
    c = Canvas(1280, 720, bg)
    c.rect(8, 8, 1264, 704, PALETTE["deep_ash"])

    # Section: training arena (full composition).
    c.blit(training_floor(), 24, 24, 1)

    # Section: player sheets at 2x.
    c.blit(player_sheet("p0"), 200, 24, 2)
    c.blit(player_sheet("p1"), 200, 80, 2)

    # Section: boomerang variants at 4x.
    c.blit(boomerang_sheet(), 200, 156, 4)
    c.blit(trail_sheet(), 200, 220, 3)

    # Section: VFX at 2x.
    c.blit(hit_burst_sheet(), 200, 280, 2)
    c.blit(death_burst_sheet(), 200, 336, 2)
    c.blit(recall_pulse_sheet(), 200, 392, 2)
    c.blit(ember_sheet(), 480, 392, 2)

    # Section: stains.
    c.blit(stain_sheet("p0"), 200, 444, 2)
    c.blit(stain_sheet("p1"), 200, 484, 2)

    # Section: arena tiles.
    c.blit(arena_tile_sheet(), 600, 156, 2)

    # Section: HUD.
    c.blit(score_pips(), 24, 540, 3)
    c.blit(timer_digits(), 24, 596, 3)
    c.blit(countdown_digits(), 24, 624, 2)
    c.blit(match_over_badge(), 360, 624, 2)
    c.blit(touch_controls_sheet(), 600, 540, 2)

    return c


# ---------------------------------------------------------------------------
# 2.5D depth: the drop/cast shadow blob.
#
# The single cheapest "everything stands on the ground" cue (DESIGN_DIRECTION
# § depth). One void ellipse, ordered-dithered at the rim so it stays on the
# 16-color lock (void + transparent — no semi-alpha for the palette gate to
# reject). The render layer tints it to a soft alpha and stretches it to each
# actor's / block's footprint; the source is authored generously so the
# downscale stays crisp.
# ---------------------------------------------------------------------------


def shadow_blob() -> Canvas:
    w, h = 40, 22
    c = Canvas(w, h, PALETTE["clear"])
    cx, cy = (w - 1) / 2.0, (h - 1) / 2.0
    rx, ry = w / 2.0 - 1.0, h / 2.0 - 1.0
    void = PALETTE["void"]
    for y in range(h):
        for x in range(w):
            d = ((x - cx) / rx) ** 2 + ((y - cy) / ry) ** 2
            if d <= 0.72:
                c.set(x, y, void)  # solid core
            elif d <= 1.0 and (x + y) % 2 == 0:
                c.set(x, y, void)  # dithered feathered rim
    return c


# ---------------------------------------------------------------------------
# Charge/juice FX: the throw-charge ring, dash dust, and the screen vignette.
#
# All render-time cosmetics. The charge ring is scaled + brightened by the sim's
# ThrowCharge (rolled-back, read-only) — energy gathering under the duelist. The
# dust puff kicks up on a dash. The vignette is a screen-space UI overlay that
# frames the action and unifies the palette (the HLD cohesion cue). Every asset
# stays on the 16-color lock (dither, no semi-alpha) so the palette gate passes.
# ---------------------------------------------------------------------------


def charge_ring() -> Canvas:
    """A bright energy ring gathered under a charging throw. The render scales it
    DOWN + brightens it toward full charge (energy tightening inward), snapping
    to a hot flash at full. spark rim + ember inner glow + hot-bone cardinals."""
    n = 40
    c = Canvas(n, n, PALETTE["clear"])
    cx = cy = (n - 1) / 2.0
    r = n / 2.0 - 2.0
    spark, ember, hot = PALETTE["spark"], PALETTE["ember"], PALETTE["hot_bone"]
    for a in range(0, 360, 3):
        rad = math.radians(a)
        c.set(round(cx + r * math.cos(rad)), round(cy + r * math.sin(rad)), spark)
        c.set(round(cx + (r - 1) * math.cos(rad)), round(cy + (r - 1) * math.sin(rad)), ember)
    for a in (0, 90, 180, 270):  # bright cardinal ticks
        rad = math.radians(a)
        c.set(round(cx + r * math.cos(rad)), round(cy + r * math.sin(rad)), hot)
    return c


def dust_puff_sheet() -> Canvas:
    """4-cell ground-dust burst (14x14): a flat cloud that expands + fades from
    bone → cold-stone → deep-ash. Kicked up on a dash; the render also fades the
    alpha over the anim so it dissipates."""
    cell, n = 14, 4
    c = Canvas(cell * n, cell, PALETTE["clear"])
    cx = cy = cell / 2.0
    cols = [PALETTE["bone"], PALETTE["cold_stone"], PALETTE["cold_stone"], PALETTE["deep_ash"]]
    for f in range(n):
        r = 2.0 + f * 2.2
        ox = f * cell
        for a in range(0, 360, 30):
            rad = math.radians(a + f * 17)
            x = ox + round(cx + r * math.cos(rad))
            y = round(cy + r * 0.55 * math.sin(rad))  # flattened — ground dust
            c.set(x, y, cols[f])
    return c


def vignette() -> Canvas:
    """Screen-space vignette (stretched to fill as a UI overlay): a transparent
    centre fading through an ordered-dithered void toward the edges. Frames the
    action and unifies the palette. Authored at 128px; the pixel dither keeps the
    fullscreen stretch soft + palette-legal (void + clear only)."""
    n = 128
    c = Canvas(n, n, PALETTE["clear"])
    cx = cy = (n - 1) / 2.0
    maxr = (n / 2.0) * 1.18
    void = PALETTE["void"]
    bayer = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]
    for y in range(n):
        for x in range(n):
            d = math.hypot(x - cx, y - cy) / maxr
            if d > 0.60:
                t = (d - 0.60) / 0.40  # 0 at the fade start, 1 at the corner
                if t * 16 > bayer[y & 3][x & 3]:
                    c.set(x, y, void)
    return c


# ---------------------------------------------------------------------------
# Main: write all assets to the canonical paths.
# ---------------------------------------------------------------------------


def main() -> None:
    outputs = [
        ("assets/sprites/players/duelist_a_sheet.png", player_sheet("p0")),
        ("assets/sprites/players/duelist_b_sheet.png", player_sheet("p1")),
        *[
            (f"assets/sprites/players/duelist_a_v{n}.png", player_sheet("p0", n))
            for n in range(1, 8)
        ],
        *[
            (f"assets/sprites/players/duelist_b_v{n}.png", player_sheet("p1", n))
            for n in range(1, 8)
        ],
        ("assets/sprites/projectiles/bone_fang.png", boomerang(0)),
        ("assets/sprites/projectiles/bone_fang_marked_sheet.png", boomerang_sheet()),
        ("assets/sprites/projectiles/bone_fang_trail_sheet.png", trail_sheet()),
        ("assets/sprites/particles/hit_burst_sheet.png", hit_burst_sheet()),
        ("assets/sprites/particles/death_burst_sheet.png", death_burst_sheet()),
        ("assets/sprites/particles/recall_pulse_sheet.png", recall_pulse_sheet()),
        ("assets/sprites/particles/ambient_ember_sheet.png", ember_sheet()),
        ("assets/sprites/stains/p0_stain_sheet.png", stain_sheet("p0")),
        ("assets/sprites/stains/p1_stain_sheet.png", stain_sheet("p1")),
        ("assets/arenas/training_floor.png", training_floor()),
        ("assets/arenas/anchor_floor.png", arena_floor("anchor")),
        ("assets/arenas/crossing_floor.png", arena_floor("crossing")),
        ("assets/arenas/reliquary_floor.png", arena_floor("reliquary")),
        ("assets/arenas/pit_floor.png", arena_floor("pit")),
        ("assets/arenas/vigil_floor.png", arena_floor("vigil")),
        ("assets/arenas/gallery_floor.png", arena_floor("gallery")),
        ("assets/arenas/forest_floor.png", arena_floor("forest")),
        ("assets/arenas/tile_sheet.png", arena_tile_sheet()),
        ("assets/sprites/arena/bone_pyre_sheet.png", bone_pyre_sheet()),
        ("assets/sprites/arena/bone_tree_sheet.png", bone_tree_sheet()),
        ("assets/sprites/arena/chasm_strip.png", chasm_strip()),
        ("assets/sprites/arena/altar_sigil_sheet.png", altar_sigil_sheet()),
        ("assets/sprites/arena/bone_bridge_tile.png", bone_bridge_tile()),
        ("assets/sprites/arena/sigil_door_sheet.png", sigil_door_sheet()),
        ("assets/sprites/pickups/pickup_sheet.png", pickup_sheet()),
        ("assets/sprites/fx/shadow_blob.png", shadow_blob()),
        ("assets/sprites/fx/charge_ring.png", charge_ring()),
        ("assets/sprites/fx/dust_puff_sheet.png", dust_puff_sheet()),
        ("assets/sprites/fx/vignette.png", vignette()),
        ("assets/hud/score_pips.png", score_pips()),
        ("assets/hud/timer_digits.png", timer_digits()),
        ("assets/hud/countdown_digits.png", countdown_digits()),
        ("assets/hud/match_over_badge.png", match_over_badge()),
        ("assets/hud/touch_controls.png", touch_controls_sheet()),
        ("assets/hud/scrub_bar_track.png", scrub_bar_track()),
        ("assets/hud/scrub_bar_handle.png", scrub_bar_handle()),
        ("assets/hud/frame_step_buttons.png", frame_step_buttons()),
        ("assets/concepts/phase15_contact_sheet.png", contact_sheet()),
        ("assets/concepts/contact_sheet_v2.png", duelist_contact_sheet("p0")),
        ("assets/concepts/contact_sheet_v2_p1.png", duelist_contact_sheet("p1")),
    ]
    for rel_path, canvas in outputs:
        write_png(canvas, rel_path)
        print(rel_path)


if __name__ == "__main__":
    main()
