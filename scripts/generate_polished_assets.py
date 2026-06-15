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

import os
import struct
import zlib
from dataclasses import dataclass


ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))


# ---------------------------------------------------------------------------
# Palette — locked 16 colors from assets/palettes/two_top_16.gpl, plus a
# transparent slot. Keep hex values in sync with the .gpl.
# ---------------------------------------------------------------------------

PALETTE: dict[str, tuple[int, int, int, int]] = {
    "clear":           (0, 0, 0, 0),
    "void":            (11, 13, 18, 255),
    "deep_ash":        (23, 25, 34, 255),
    "bruise_shadow":   (43, 37, 51, 255),
    "charcoal_line":   (57, 52, 66, 255),
    "cold_stone":      (87, 90, 100, 255),
    "warm_bone_shade": (122, 101, 88, 255),
    "bone":            (203, 190, 148, 255),
    "hot_bone":        (255, 241, 194, 255),
    "blood_dark":      (110, 22, 50, 255),
    "p0_blood":        (210, 47, 69, 255),
    "ember":           (240, 106, 58, 255),
    "spark":           (255, 216, 102, 255),
    "deep_teal":       (13, 101, 114, 255),
    "p1_cyan":         (39, 199, 216, 255),
    "recall_blue":     (71, 108, 255, 255),
    "hit_white":       (248, 247, 232, 255),
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
# "The Cur" (P0) / "The Stag" (P1) share an animation skeleton; P1's distinct
# antlered silhouette lands in a later cycle (for now P1 = P0 recolored via
# keys_for("p1")).
#
# Rather than 41 hand-typed ASCII grids, the duelist is drawn procedurally
# from a parametric skeleton: filled body spans + committed top-left lighting
# + an auto-outline pass. Poses are parameters (lean / bob / arm / leg /
# flash / gore), so the 41-frame animation set is consistent frame-to-frame
# and tunable without redrawing. Output is the same char-grid the rest of the
# generator paints, so keys_for() still swaps P0<->P1 colors.
#
# Grid chars (consumed by paint()):
#   k void outline   l charcoal inner-line   M body   m body-shadow
#   b bone  B hot-bone  w warm-bone-shade   y spark(eyes)  h hit-white
#   o bruise(ground)   e accent(ember/recall-blue)
# ===========================================================================

PLAYER_PX = 32
_CX = 15  # silhouette centre column


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


# Body cells that participate in silhouette / shading / outline.
_BODY = {"M", "m", "h"}
_BONE = {"b", "B", "w"}
_SOLID = _BODY | _BONE | {"l", "y", "e", "o"}


def _shade(g: list[list[str]]) -> None:
    """Commit light from the top-left: the right ~third of every body span and
    its lowest row drop to shadow tone. Bone gets a hot-bone gleam on its
    upper-left, warm-shade on its lower-right."""
    # Body shadow: per row, shade the right portion of each contiguous M run.
    for y in range(PLAYER_PX):
        x = 0
        while x < PLAYER_PX:
            if g[y][x] == "M":
                x0 = x
                while x < PLAYER_PX and g[y][x] == "M":
                    x += 1
                x1 = x - 1
                run = x1 - x0 + 1
                shade_from = x1 - max(1, run // 3) + 1
                for xx in range(shade_from, x1 + 1):
                    g[y][xx] = "m"
            else:
                x += 1
    # Bone gleam / shade.
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
    """Wrap the silhouette in a 1px void outline on every empty 4-neighbour of
    a solid cell. Readability (gameplay priority #1) beats the broken-outline
    refinement, so the outline is closed."""
    adds = []
    for y in range(PLAYER_PX):
        for x in range(PLAYER_PX):
            if g[y][x] != ".":
                continue
            touch = False
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx, ny = x + dx, y + dy
                if 0 <= nx < PLAYER_PX and 0 <= ny < PLAYER_PX and g[ny][nx] in _SOLID:
                    touch = True
                    break
            if touch:
                adds.append((x, y))
    for x, y in adds:
        g[y][x] = "k"


# --- body parts ------------------------------------------------------------

def _horns(g: list[list[str]], dx: int, dy: int) -> None:
    """Two hooked bull horns rooted on the skull, curving up and out to a
    hot-bone tip. Wider spread reads more menacing at thumbnail size."""
    cx = _CX + dx
    # left horn
    _put(g, cx - 4, 4 + dy, "b")
    _put(g, cx - 4, 3 + dy, "b")
    _put(g, cx - 5, 2 + dy, "b")
    _put(g, cx - 5, 1 + dy, "B")
    _put(g, cx - 6, 1 + dy, "B")  # hooked tip
    # right horn
    _put(g, cx + 4, 4 + dy, "b")
    _put(g, cx + 4, 3 + dy, "b")
    _put(g, cx + 5, 2 + dy, "b")
    _put(g, cx + 5, 1 + dy, "B")
    _put(g, cx + 6, 1 + dy, "B")


def _head(g: list[list[str]], dx: int, dy: int) -> None:
    """Solid skull dome, sunk between the shoulders, with a heavy brow, two
    burning spark eyes set in void sockets, and a fanged snarl."""
    cx = _CX + dx
    # solid dome
    _span(g, 4 + dy, cx - 3, cx + 3, "M")
    _span(g, 5 + dy, cx - 4, cx + 4, "M")
    _span(g, 6 + dy, cx - 4, cx + 4, "M")
    _span(g, 7 + dy, cx - 4, cx + 4, "M")
    _span(g, 8 + dy, cx - 4, cx + 4, "M")
    _span(g, 9 + dy, cx - 3, cx + 3, "M")  # jaw
    # eyes: void sockets with burning spark pupils
    _put(g, cx - 3, 7 + dy, "k")
    _put(g, cx + 3, 7 + dy, "k")
    _put(g, cx - 2, 7 + dy, "y")
    _put(g, cx + 2, 7 + dy, "y")
    _put(g, cx - 2, 6 + dy, "e")  # eye glow bleed
    _put(g, cx + 2, 6 + dy, "e")
    # fanged snarl
    _span(g, 9 + dy, cx - 2, cx + 2, "l")
    _put(g, cx - 1, 9 + dy, "B")
    _put(g, cx + 1, 9 + dy, "B")


def _torso(g: list[list[str]], dx: int, dy: int, hunch: int = 0) -> None:
    """Hunched shoulders tapering to a waist, then a ragged cloak that always
    reaches down to the legs (filled to a fixed hem row so the torso never
    floats free of the legs when the upper body bobs)."""
    cx = _CX + dx
    sy = 11 + dy + hunch
    # shoulders (widest, hunched up)
    _span(g, sy, cx - 5, cx + 5, "M")
    _span(g, sy + 1, cx - 5, cx + 5, "M")
    _put(g, cx, sy + 3, "b")  # bone amulet
    # fill the body from the shoulders down to the cloak hem at a fixed row,
    # tapering at the waist then flaring for the cloak.
    for y in range(sy + 2, 22):
        if y < sy + 5:
            w = 4
        elif y < 19:
            w = 3
        else:
            w = min(6, 5 + (y - 19))
        _span(g, y, cx - w, cx + w, "M")
    # jagged cloak hem (deterministic notch pattern), rows 22-23
    for x in range(cx - 6, cx + 7):
        if (x - cx) % 2 == 0:
            _put(g, x, 22, "M")
            _put(g, x, 23, "m")
        else:
            _put(g, x, 22, "m")


def _arm(g: list[list[str]], side: int, dx: int, dy: int, pose: str) -> None:
    """side -1 left / +1 right. pose: rest|back|fwd|up|reach. Arms are 2px so
    they read as limbs, not twigs, and end in a bone claw."""
    cx = _CX + dx
    sy = 12 + dy
    sx = cx + side * 5
    if pose == "rest":
        for i in range(5):
            _put(g, sx, sy + i, "M")
            _put(g, sx + side, sy + i, "M")
        _put(g, sx, sy + 5, "b")
    elif pose == "back":
        # cocked back over the shoulder (throw anticipation)
        _put(g, sx, sy, "M")
        _put(g, sx - side, sy - 1, "M")
        _put(g, sx - side, sy, "M")
        _put(g, sx - side * 2, sy - 2, "M")
        _put(g, sx - side * 3, sy - 2, "b")
        _put(g, sx - side * 3, sy - 3, "b")
    elif pose == "fwd":
        # thrust forward, 2px thick
        for i in range(6):
            _put(g, sx + side * i, sy + 1, "M")
            _put(g, sx + side * i, sy + 2, "M")
        _put(g, sx + side * 6, sy + 1, "b")
        _put(g, sx + side * 6, sy + 2, "b")
    elif pose == "up":
        for i in range(6):
            _put(g, sx, sy - i, "M")
            _put(g, sx + side, sy - i, "M")
        _put(g, sx, sy - 6, "b")
    elif pose == "reach":
        for i in range(4):
            _put(g, sx + side * i, sy - i, "M")
            _put(g, sx + side * i, sy - i + 1, "M")
        _put(g, sx + side * 4, sy - 4, "b")


def _legs(g: list[list[str]], dx: int, dy: int, phase: str) -> None:
    """phase: stand|strideR|strideL|air|kneel|splay."""
    cx = _CX + dx
    ty = 24 + dy
    if phase == "stand":
        for i in range(5):
            _span(g, ty + i, cx - 3, cx - 1, "M")
            _span(g, ty + i, cx + 1, cx + 3, "M")
        _span(g, ty + 5, cx - 4, cx - 1, "b")  # clawed feet
        _span(g, ty + 5, cx + 1, cx + 4, "b")
    elif phase == "strideR":
        for i in range(5):
            _span(g, ty + i, cx + 1 + i // 2, cx + 3 + i // 2, "M")  # right fwd
            _span(g, ty + i, cx - 4 + (4 - i) // 2, cx - 2 + (4 - i) // 2, "M")  # left back
        _span(g, ty + 5, cx + 3, cx + 6, "b")
        _span(g, ty + 5, cx - 6, cx - 3, "b")
    elif phase == "strideL":
        for i in range(5):
            _span(g, ty + i, cx - 3 - i // 2, cx - 1 - i // 2, "M")
            _span(g, ty + i, cx + 2 - (4 - i) // 2, cx + 4 - (4 - i) // 2, "M")
        _span(g, ty + 5, cx - 6, cx - 3, "b")
        _span(g, ty + 5, cx + 3, cx + 6, "b")
    elif phase == "air":
        # both legs tucked up, body airborne
        for i in range(3):
            _span(g, ty + i, cx - 3, cx - 1, "M")
            _span(g, ty + i, cx + 1, cx + 3, "M")
        _span(g, ty + 3, cx - 4, cx - 2, "b")
        _span(g, ty + 3, cx + 2, cx + 4, "b")
    elif phase == "kneel":
        # right leg folded under, left planted
        for i in range(4):
            _span(g, ty + i, cx + 1, cx + 3, "M")
        _span(g, ty + 4, cx + 1, cx + 4, "b")
        _span(g, ty + 2, cx - 4, cx - 1, "M")
        _span(g, ty + 3, cx - 5, cx - 2, "b")
    elif phase == "splay":
        for i in range(5):
            _span(g, ty + i, cx - 5 + i // 2, cx - 3 + i // 2, "M")
            _span(g, ty + i, cx + 3 - i // 2, cx + 5 - i // 2, "M")
        _span(g, ty + 5, cx - 7, cx - 4, "b")
        _span(g, ty + 5, cx + 4, cx + 7, "b")


def _ground(g: list[list[str]], dx: int) -> None:
    cx = _CX + dx
    _span(g, 30, cx - 4, cx + 4, "o")
    _span(g, 31, cx - 3, cx + 3, "o")


def _draw_duelist(
    *,
    lean: int = 0,
    bob: int = 0,
    hunch: int = 0,
    arm_l: str = "rest",
    arm_r: str = "rest",
    leg: str = "stand",
    ground: bool = True,
) -> list[list[str]]:
    g = _blank()
    if ground:
        _ground(g, lean)
    _legs(g, lean, 0, leg)  # legs stay planted (no bob)
    _torso(g, lean, bob, hunch)
    _arm(g, -1, lean, bob, arm_l)
    _arm(g, +1, lean, bob, arm_r)
    _head(g, lean, bob)
    _horns(g, lean, bob)
    _shade(g)
    _outline(g)
    return g


def _flash(g: list[list[str]]) -> list[list[str]]:
    """White-hot hit silhouette: every body/bone cell -> hit-white, outline kept."""
    out = [row[:] for row in g]
    for y in range(PLAYER_PX):
        for x in range(PLAYER_PX):
            c = out[y][x]
            if c in _BODY or c in _BONE or c in ("l", "y"):
                out[y][x] = "h"
    return out


def _spark_burst(g: list[list[str]], x: int, y: int) -> None:
    """A small spark/ember pop at a hand (perfect-catch flair)."""
    _put(g, x, y, "B")
    _put(g, x - 1, y, "y")
    _put(g, x + 1, y, "y")
    _put(g, x, y - 1, "y")
    _put(g, x, y + 1, "e")


def _gore_chunks(g: list[list[str]], stage: int) -> None:
    """Deterministic gore chunks flung outward, growing with the death stage."""
    # (dx, dy, char) fixed offsets relative to centre — no RNG (sim-safe ethos).
    chunks = [
        (-6, 12, "M"), (6, 11, "m"), (-8, 16, "m"), (8, 15, "M"),
        (-4, 8, "M"), (5, 7, "m"), (-9, 20, "m"), (9, 19, "M"),
        (-2, 6, "y"), (3, 5, "e"), (-7, 23, "m"), (7, 22, "M"),
    ]
    cx = _CX
    for i in range(min(len(chunks), stage * 3)):
        dx, dy, ch = chunks[i]
        spread = stage  # chunks drift further out each stage
        _put(g, cx + dx + (1 if dx > 0 else -1) * spread, dy, ch)


def _corpse_heap(lean: int = 0) -> list[list[str]]:
    """Low corpse mound — reads like the stain corpse-mark frame."""
    g = _blank()
    cx = _CX + lean
    _ground(g, lean)
    _span(g, 27, cx - 5, cx + 5, "M")
    _span(g, 28, cx - 6, cx + 6, "M")
    _span(g, 29, cx - 6, cx + 6, "m")
    _put(g, cx - 4, 26, "b")  # a horn jutting from the pile
    _put(g, cx - 5, 25, "B")
    _put(g, cx + 5, 27, "b")
    _shade(g)
    _outline(g)
    return g


def _player_frames() -> list[list[list[str]]]:
    """The 41-frame v2 sequence: IDLE6 RUN6 THROW8 DASH4 HIT4 CATCH3 DEATH10."""
    frames: list[list[list[str]]] = []

    # IDLE (6): gentle breath bob of the upper body; horns lag is implicit.
    for bob in (0, 0, -1, -1, 0, 1):
        frames.append(_draw_duelist(bob=bob, hunch=0))

    # RUN (6): forward lean, leg cycle, body lifts on the passing/air beats.
    run_cycle = [
        ("strideR", 0), ("air", -1), ("strideL", 0),
        ("strideR", -1), ("air", -1), ("strideL", 0),
    ]
    for leg, bob in run_cycle:
        frames.append(_draw_duelist(lean=1, bob=bob, leg=leg, arm_l="back", arm_r="fwd"))

    # THROW (8): 3 anticipation (coil back, lean back), 1 release smear,
    # 4 follow-through settling to rest.
    frames.append(_draw_duelist(lean=-1, arm_r="back", arm_l="rest"))      # ready
    frames.append(_draw_duelist(lean=-1, arm_r="back", hunch=-1))          # coil
    frames.append(_draw_duelist(lean=-2, arm_r="back", hunch=-1))          # wind
    rel = _draw_duelist(lean=1, arm_r="fwd")                                # release smear
    for i in range(6):  # arm smear streak
        _put(rel, _CX + 6 + i, 13, "e")
    frames.append(rel)
    frames.append(_draw_duelist(lean=1, arm_r="fwd"))                      # follow 1
    frames.append(_draw_duelist(lean=0, arm_r="fwd"))                      # follow 2
    frames.append(_draw_duelist(lean=0, arm_r="rest"))                     # recover
    frames.append(_draw_duelist(lean=0))                                   # settle

    # DASH (4): hard forward lunge, legs splayed, motion afterimage behind.
    for k in range(4):
        d = _draw_duelist(lean=2 + (k % 2), leg="splay", arm_l="back", arm_r="fwd", ground=False)
        # afterimage smear trailing back-left (darker body tone + accent)
        for t in (3, 5, 7):
            for y in range(12, 24):
                if d[y][max(0, _CX - 2)] in _BODY:
                    _put(d, max(0, _CX - t), y, "e" if t == 3 else "m")
        frames.append(d)

    # HIT (4): full white flash, then a recoiling stagger back to normal.
    base_hit = _draw_duelist(lean=0)
    frames.append(_flash(base_hit))                       # flash
    frames.append(_draw_duelist(lean=-2, hunch=-1))       # recoil
    frames.append(_draw_duelist(lean=-1))                 # settle 1
    frames.append(_draw_duelist(lean=0))                  # settle 2

    # CATCH (3): arm snaps up, spark pops at the hand, then lowers.
    c0 = _draw_duelist(arm_r="up")
    frames.append(c0)
    c1 = _draw_duelist(arm_r="up")
    _spark_burst(c1, _CX + 6, 6)
    frames.append(c1)
    frames.append(_draw_duelist(arm_r="reach"))

    # DEATH (10): stagger -> fold -> buckle -> gore burst -> disperse -> heap.
    frames.append(_draw_duelist(lean=-1, hunch=-1))                       # stagger
    frames.append(_draw_duelist(lean=-2, hunch=0))                        # stagger 2
    frames.append(_draw_duelist(lean=-1, hunch=1, arm_l="rest", arm_r="rest"))  # fold
    frames.append(_draw_duelist(lean=0, hunch=2, leg="kneel"))            # bow
    buckle = _draw_duelist(lean=0, hunch=3, leg="kneel")
    _gore_chunks(buckle, 1)
    frames.append(buckle)                                                 # buckle
    burst = _draw_duelist(lean=0, hunch=3, leg="kneel")
    _gore_chunks(burst, 2)
    frames.append(burst)                                                  # burst
    burst2 = _draw_duelist(lean=0, hunch=4, leg="kneel", ground=True)
    _gore_chunks(burst2, 3)
    frames.append(burst2)                                                 # burst 2
    disperse = _corpse_heap()
    _gore_chunks(disperse, 4)
    frames.append(disperse)                                               # disperse
    frames.append(_corpse_heap())                                         # heap
    frames.append(_corpse_heap())                                         # corpse mark

    assert len(frames) == 41, f"expected 41 frames, got {len(frames)}"
    return frames


_PLAYER_FRAME_CACHE: list[list[list[str]]] | None = None


def _player_frame_strs() -> list[str]:
    global _PLAYER_FRAME_CACHE
    if _PLAYER_FRAME_CACHE is None:
        _PLAYER_FRAME_CACHE = _player_frames()
    return [_grid_to_str(f) for f in _PLAYER_FRAME_CACHE]


def player_sheet(side: str) -> Canvas:
    """41-frame strip (32x32 cells): IDLE6 RUN6 THROW8 DASH4 HIT4 CATCH3 DEATH10.
    Per ART_DIRECTION.md v2. P0/P1 share geometry; keys_for(side) recolors."""
    frames = _player_frame_strs()
    canvas = Canvas(PLAYER_PX * 41, PLAYER_PX)
    for i, art in enumerate(frames):
        paint(canvas, i * PLAYER_PX, 0, art, side=side)
    return canvas


# Animation row layout for the review contact sheet.
_ANIM_ROWS = [
    ("idle", 0, 6),
    ("run", 6, 6),
    ("throw", 12, 8),
    ("dash", 20, 4),
    ("hit", 24, 4),
    ("catch", 28, 3),
    ("death", 31, 10),
]


def duelist_contact_sheet(side: str) -> Canvas:
    """Review sheet: each animation on its own row, scaled 5x on a void bg, so
    the 41-frame set can be eyeballed against the craft checklist."""
    frames = _PLAYER_FRAME_CACHE or _player_frames()
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
    c = Canvas(8 * 4, 8)
    for i, art in enumerate([EMBER_F0, EMBER_F1, EMBER_F2, EMBER_F3]):
        paint(c, i * 8, 0, art, side="p0")
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


def training_floor() -> Canvas:
    """Composed training arena: 10 tiles wide x 15 tiles tall = 160x240 px.

    Arrangement: walls around the perimeter, checker floor inside, spawn
    marks at the standard duel positions, duel-diamond geometry at center.
    """
    cols, rows = 10, 15
    c = Canvas(cols * 16, rows * 16)
    # Floor checker.
    for r in range(rows):
        for col in range(cols):
            primary = (col + r) % 2 == 0
            paint(c, col * 16, r * 16,
                  FLOOR_PRIMARY if primary else FLOOR_SECONDARY, side="p0")
    # Walls — top + bottom rows, left + right cols. Corners take precedence.
    for col in range(1, cols - 1):
        paint(c, col * 16, 0, WALL_EDGE_TOP, side="p0")
        paint(c, col * 16, (rows - 1) * 16, WALL_EDGE_BOTTOM, side="p0")
    for r in range(1, rows - 1):
        paint(c, 0, r * 16, WALL_EDGE_LEFT, side="p0")
        paint(c, (cols - 1) * 16, r * 16, WALL_EDGE_RIGHT, side="p0")
    paint(c, 0, 0, WALL_CORNER_TL, side="p0")
    paint(c, (cols - 1) * 16, 0, WALL_CORNER_TR, side="p0")
    paint(c, 0, (rows - 1) * 16, WALL_CORNER_BL, side="p0")
    paint(c, (cols - 1) * 16, (rows - 1) * 16, WALL_CORNER_BR, side="p0")
    # Spawn marks — opposite mid-arena positions.
    paint(c, 1 * 16, 7 * 16, SPAWN_MARK_P0, side="p0")
    paint(c, 8 * 16, 7 * 16, SPAWN_MARK_P1, side="p0")
    # Center duel motif — a thin charcoal-line vertical, plus a diamond
    # outlined in cold-stone for parallax.
    cx = c.width // 2
    cy = c.height // 2
    for y in range(20, c.height - 20):
        c.set(cx - 1, y, PALETTE["charcoal_line"])
    # Diamond from cardinal points.
    for r in range(28):
        c.set(cx - 1 - r, cy + r - 28, PALETTE["cold_stone"])
        c.set(cx - 1 - r, cy - r + 28, PALETTE["cold_stone"])
        c.set(cx - 1 + r, cy + r - 28, PALETTE["cold_stone"])
        c.set(cx - 1 + r, cy - r + 28, PALETTE["cold_stone"])
    return c


# ===========================================================================
# HUD — score pips, timer digits, countdown digits, match-over badge,
# touch ring states.
# ===========================================================================

PIP_FILLED = """
kkkkkkk.
kMMMMMk.
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


def score_pips() -> Canvas:
    """Two rows: P0 (top) + P1 (bottom). 5 pips per row, 8x8 each.
    Renders as filled-or-empty per round score; this sheet shows the
    template strip with all-filled and all-empty side by side."""
    c = Canvas(80, 16)
    for i in range(5):
        paint(c, i * 8, 0, PIP_FILLED, side="p0")
        paint(c, i * 8, 8, PIP_FILLED, side="p1")
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
# Main: write all assets to the canonical paths.
# ---------------------------------------------------------------------------


def main() -> None:
    outputs = [
        ("assets/sprites/players/duelist_a_sheet.png", player_sheet("p0")),
        ("assets/sprites/players/duelist_b_sheet.png", player_sheet("p1")),
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
        ("assets/arenas/tile_sheet.png", arena_tile_sheet()),
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
