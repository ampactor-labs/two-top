# Assets

Phase 15 polished pixel art for 2-Top. Hand-designed sprites embodying
the **Bone Cathedral, blood-marked** synthesis from
`../VISUAL_TARGET_PACK.md` (gore-pixel revival × HLD slickness, applied
per-mode).

All assets are deterministic — defined as ASCII pixel grids in
`scripts/generate_polished_assets.py` and reproducible across platforms
with no external libs. Edit the grid, regenerate, the PNG snaps to
match.

Regenerate:

```sh
python3 scripts/generate_polished_assets.py
```

## Palette

- `palettes/two_top_16.gpl` — locked 16-color GIMP-importable palette.

## Player Sheets

All player frames are 24 by 24 px. Sheet dimensions are 528 by 24 px:
22 frames in one row.

Frame ranges:

| Range | Animation |
| --- | --- |
| 0-3   | idle (subtle bob) |
| 4-9   | throw (wind-up → cock → release → fang flying → recovery → settle) |
| 10-11 | dash (forward smear with side-accent trail) |
| 12-15 | hit (white silhouette flash → return) |
| 16-21 | death (stagger → bow → buckle → disperse → corpse mark) |

Files:

- `sprites/players/duelist_a_sheet.png` — P0, compact horned silhouette,
  red body.
- `sprites/players/duelist_b_sheet.png` — P1, antlered wider silhouette,
  cyan body.

## Projectile

- `sprites/projectiles/bone_fang.png` — 12 by 12 px clean boomerang
  (carved sigil along the spine, asymmetric fang shape).
- `sprites/projectiles/bone_fang_marked_sheet.png` — 4-cell strip,
  12 by 12 each: clean / 1-mark / 2-mark / 3-mark. Per-round blood
  accumulation (cosmetic, never feeds sim).
- `sprites/projectiles/bone_fang_trail_sheet.png` — 6 frames, 12 by 12
  each: 3 flying frames (spark/ember dots) + 3 returning frames
  (recall-blue ticks mixed with bone).

## Particles

- `sprites/particles/hit_burst_sheet.png` — 4 frames, 24 by 24 each.
  Hit-white core → radial spokes → spark accents.
- `sprites/particles/death_burst_sheet.png` — 6 frames, 24 by 24 each.
  Flash → spray → shards → chunks → corpse mark commit.
- `sprites/particles/recall_pulse_sheet.png` — 4 frames, 16 by 16 each.
  Inward recall-blue ticks; never a full-screen wash.
- `sprites/particles/ambient_ember_sheet.png` — 4 frames, 8 by 8 each.
  Drifting 1-3 px chunks, dim enough not to compete with the boomerang
  trail.

## Stains (Render-only, round-scoped)

- `sprites/stains/p0_stain_sheet.png` — 4-cell strip, 16 by 16 each:
  small / medium / heavy / corpse mark. P0 victim stains.
- `sprites/stains/p1_stain_sheet.png` — same layout, P1 victim stains.

Stains paint the floor at kill positions and persist until round reset.
The arena remembers each round's violence; this is the gore-revival
pole's resting state in composition mode.

## Arena and Tiles

- `arenas/training_floor.png` — full 160 by 240 px composed training
  arena (walls, checker floor, spawn marks, duel diamond).
- `arenas/tile_sheet.png` — 12-tile sheet (3 rows by 4 cols, 16 by 16
  each): floor primary/secondary, spawn marks (P0/P1), wall edges
  (top/bottom/left/right), wall corners (TL/TR/BL/BR).

## HUD

- `hud/score_pips.png` — 80 by 16 px; 5 pips per row, P0 (top) + P1
  (bottom).
- `hud/timer_digits.png` — 60 by 7 px; digits 0-9, hit-white.
- `hud/countdown_digits.png` — 80 by 16 px; 3 / 2 / 1 / G / O glyphs
  with restrained spark accents.
- `hud/match_over_badge.png` — 80 by 32 px; bone frame, hit-white
  letters, drop shadow, single spark flair per corner.
- `hud/touch_controls.png` — 64 by 16 px; 4-cell strip with virtual
  stick idle/active and throw ring idle/active.
- `hud/scrub_bar_track.png` — 192 by 12 px; replay-viewer scrub bar
  background (deep-ash interior, charcoal-line frame, warm-bone-shade
  ticks every 16 px, bone anchor pips every 64 px).
- `hud/scrub_bar_handle.png` — 8 by 16 px; bone-fang vertical needle
  used as the current-frame slider handle on the scrub bar.

## Contact Sheet

- `concepts/phase15_contact_sheet.png` — 1280 by 720 review board with
  every polished asset visible at scale. The acceptance gate.

## Visual Target Pack

- `concepts/target_pack/visual_target_overview.png` — four-direction
  comparison.
- `concepts/target_pack/bone_cathedral.png` — chosen direction at rest
  (composition mode).
- `concepts/target_pack/synthesis_showcase.png` — chosen direction in
  contact (gore-pixel × HLD synthesis: floor stains, kill frame,
  attitude poses, boomerang blood-mark accumulation).

Regenerate the target boards with:

```sh
python3 -B scripts/generate_visual_targets.py
```

## Image-Generation Reference

- `concepts/phase15_image_prompts.md` — prompt set for the first true
  image-generation pass (use the polished art as silhouette/palette
  reference; AI-generated outputs are concept source until reduced to
  the locked palette and pixel-cleaned).
