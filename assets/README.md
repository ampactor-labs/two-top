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

All player frames are 32 by 32 px. Sheet dimensions are 1312 by 32 px:
41 frames in one row. `sim::AnimState` owns this animation table; the
render layer reads it. Keep this section in sync with `sim` and
`ART_DIRECTION.md` § Animation Table.

Frame ranges (anim id → atlas offset):

| Range | Animation |
| --- | --- |
| 0-5   | idle (heavy breath; shoulders rise/settle) |
| 6-11  | run (forward lean, leg cycle) |
| 12-19 | throw (3 anticipation → release smear → 4 follow-through) |
| 20-23 | dash (forward smear with side-accent afterimage) |
| 24-27 | hit (white silhouette flash → recoiling stagger) |
| 28-30 | catch (claw snaps up, spark pops, lowers) |
| 31-40 | death (stagger → fold → buckle → gore burst → corpse heap) |

Files:

- `sprites/players/duelist_a_sheet.png` — P0 "the Cur": red body, a
  broad, low, forward-hunched silhouette with curled bull horns and a
  bone pauldron.
- `sprites/players/duelist_b_sheet.png` — P1 "the Stag": cyan body, a
  taller, narrower, upright silhouette with a branching antler crown and
  a trailing tail. The Cur and Stag read apart by **body shape** under
  the silhouette-flood test, not by color alone (`Build` profiles in
  `scripts/generate_polished_assets.py`).

## Projectile

- `sprites/projectiles/bone_fang.png` — 12 by 12 px clean boomerang
  (carved sigil along the spine, asymmetric fang shape).
- `sprites/projectiles/bone_fang_marked_sheet.png` — 4-cell strip,
  12 by 12 each: clean / 1-mark / 2-mark / 3-mark. **Not loaded by the
  game.** The v2 design review (`docs/DESIGN_DIRECTION.md`) cut
  on-boomerang blood-marks: the priority-#1 fang stays clean and the
  "arena remembers the violence" cue lives in the floor stains instead.
  Sheet retained for reference only.
- `sprites/projectiles/bone_fang_trail_sheet.png` — 6 frames, 12 by 12
  each. **Not loaded by the game.** Superseded: the flight trail is to
  be a render-side ghost-stamp of the live fang (state-tinted, Recall
  Blue on return), not a pre-baked sheet (`docs/DESIGN_DIRECTION.md`).

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

- `arenas/training_floor.png` — full 320 by 480 px composed arena floor
  (checker floor, spawn marks, duel diamond). Currently the **shared**
  floor for all three arenas (Anchor/Crossing/Reliquary), which differ
  by props; per-arena floor retints are tracked in
  `docs/DESIGN_DIRECTION.md`.
- `arenas/tile_sheet.png` — 12-tile sheet (3 rows by 4 cols, 16 by 16
  each): floor primary/secondary, spawn marks (P0/P1), wall edges
  (top/bottom/left/right), wall corners (TL/TR/BL/BR).

## HUD

The minimal in-match HUD (`docs/DESIGN_DIRECTION.md` § 2) is wired in
`app/src/hud.rs` and consumes `score_pips.png` + `countdown_digits.png`
(plus a code-drawn timer bar). Still unused: `match_over_badge.png` and
`touch_controls.png` (the `MatchOver` summary is still vector text;
`timer_digits.png` is superseded by the depleting bar). The replay-viewer
scrub sheets (`scrub_bar_*`, `frame_step_buttons`) are loaded by
`replay_viewer`.

- `hud/score_pips.png` — 24 by 8 px; **3-cell 8×8 atlas** [empty,
  filled-P0, filled-P1]. The HUD draws five pips per player and indexes
  filled/empty by `MatchScore`.
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
- `hud/frame_step_buttons.png` — 32 by 16 px; 2-cell strip with
  bone-fang chevron buttons (cell 0 = back ◀, cell 1 = forward ▶) for
  the replay viewer's frame-step controls.

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
