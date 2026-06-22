# Art Direction v2

Art overhaul spec. Supersedes the Phase 15 prep version. This document
does not supersede `ARCHITECTURE.md`, `BUILD_PLAN.md`, or `CONVENTIONS.md`;
it turns their visual intent into an asset contract at a dramatically
higher craft bar.

## North Star

2-Top is a synthesis of two pixel-art lineages.

**Lineage A — gore-pixel revival.** Duke Nukem 3D, classic Doom, and
the modern revival scene that leans into chunky pixels, hard color
contrast, blood, and attitude. This is where 2-Top's combat lives:
one-hit kills, 30-second rounds, contact frames that *land*.

**Lineage B — Hyper Light Drifter slickness.** Considered animation,
restrained palettes, atmospheric tilework, compositional discipline.
This is where the cathedral, the silhouettes, and the 1v1 tactical
deliberation live.

The synthesis: **HLD discipline composes the room, gore-revival
fills it when blood spills.** Quiet between exchanges, proud-violent
during them. The arena remembers each kill until the round resets.

Neither lineage alone is enough. "Demonic HLD" without the gore pole
goes too distant-pretty for boomerang-fu energy. "Doom on a phone"
without the HLD pole loses the readability that rollback-deliberate
1v1 needs. Both poles, applied per-mode (see Two-Mode Rule below).

## Readability Hierarchy

The priority order is:

1. Boomerang position, state, and travel direction.
2. Player body position, facing, dash, stun, and death state.
3. Hit, recall, respawn, and round-state effects.
4. Walls, hazards, pickups, and arena bounds.
5. HUD, timer, score, and touch feedback.
6. Floor texture, persistent kill stains, ambient particles, and mood.

If an effect competes with a higher-priority item, cut the effect.

## Palette

The 16-color palette is the single source of truth. The importable
palette file lives at `assets/palettes/two_top_16.gpl`; the Rust
module is `render::palette`.

| Role | Hex |
| --- | --- |
| Void | `#0B0D12` |
| Deep ash | `#171922` |
| Bruise shadow | `#2B2533` |
| Charcoal line | `#393442` |
| Cold stone | `#575A64` |
| Warm bone shade | `#7A6558` |
| Bone | `#CBBE94` |
| Hot bone | `#FFF1C2` |
| Blood dark | `#6E1632` |
| P0 blood | `#D22F45` |
| Ember | `#F06A3A` |
| Spark | `#FFD866` |
| Deep teal | `#0D6572` |
| P1 cyan | `#27C7D8` |
| Recall blue | `#476CFF` |
| Hit white | `#F8F7E8` |

Do not make player identity color-only. P0 and P1 need different
silhouettes, horns, weapon hold poses, or outline treatments so they
survive color-vision differences and video compression.

## Two-Mode Rule

The renderer operates in two modes: **composition** (HLD-quiet) for
idle/movement/in-flight throw, and **contact** (gore-revival proud)
for hit/kill/death/recall-snap. Mode switches are hard, not blended.
Composition is most of the round; contact is brief frames.

## Asset Sizes (v2)

- Player source frames: **48×48 px**, rendered at **64×64 world units**.
  The cloaked-drifter rig (the HLD overhaul) is authored at 48 px for the
  hood/cloak detail; the collision body is smaller than the sprite, so the
  sprite can carry an outline, the cloak hem/scarf, and animation smear
  without changing gameplay (the world footprint is unchanged from the old
  32 px rig).
- Boomerang source frames: **12×12 px**, rendered at **40×40 world units**.
  The boomerang must remain brighter than floor detail in every arena.
- Arena tiles: **16×16 px**. Floor motifs should stay low contrast and
  never use the same high-value accents as players, boomerangs, or hits.
- Particles: source cells vary per effect (see asset table below).
  Prefer hard opaque pixels over translucent haze.
- HUD icons: **2× the v1 cell sizes**, scaled through nearest filtering.
  Touch affordances can be code-drawn rings unless a raster treatment is
  needed later.

Use nearest-neighbor filtering for all sprite assets. Avoid fractional
pixel scaling where possible.

Rationale (recorded so nobody "fixes" it back): the duelists moved to
48×48 with the cloaked-drifter overhaul — the hood, cloak folds, and
trailing scarf need the extra texels to read. The render world size stays
64×64, so the bump is detail density only, not a gameplay/footprint change.
Boomerang stays 12×12 source @ 40×40 world so gameplay feel is untouched.
HLD-ness comes from the silhouette discipline (hood hides the face) +
animation acting + composition.

## Animation Table (v2)

| Anim | ID | Frames | Atlas Offset | Ticks/Frame | Oneshot |
|------|----|--------|--------------|-------------|---------|
| IDLE | 0 | 6 | 0 | 9 | no |
| RUN | 1 | 6 | 6 | 5 | no |
| THROW | 2 | 8 | 12 | 3 | yes |
| DASH | 3 | 4 | 20 | 3 | no |
| HIT | 4 | 4 | 24 | 3 | yes |
| CATCH | 5 | 3 | 28 | 3 | yes |
| DEATH | 6 | 10 | 31 | 8 | yes |

Total atlas strip: **41 frames** per player sheet (41×1 @ 48×48 = 1968×48 px).

## Duelist Design

The duelists are **hooded drifters** (Hyper Light Drifter overhaul, locked
2026-06-22), not faces: a hood + high collar hides the face entirely and the
only feature is a glowing eye-slit. This sidesteps the unsolved problem of a
charming 48 px face and buys a dramatic, readable silhouette for free. Gore
lives in the arena floor (kill stains), never on the cloak — see
`docs/STYLE_BIBLE.md`.

- **P0 "the Cur"**: a **broad round-hooded brute**. Body P0_BLOOD, shadow
  BLOOD_DARK/BRUISE, lit rim EMBER, eye-slit EMBER + SPARK core (always the
  hottest pixel on the body). A bone pauldron caps the left shoulder
  (asymmetry + status). IDLE = slow hooded breath. RUN = forward lean,
  cloak streams, boots cycle; directional smear in the darker body shade,
  not alpha. THROW = wind-up, sleeve cock, release smear, follow-through.
  DASH = hard lunge + horizontal afterimage. HIT = full HIT_WHITE
  silhouette flash 1 frame, then recoil. CATCH = sleeve snap up, 1-frame
  SPARK flash at the hand. DEATH = stagger, fold, gore burst, collapse to a
  cloak heap that matches the floor stain.

- **P1 "the Stag"**: a **tall peaked-hooded herald** with a forward-flopping
  cloth tip and a long trailing scarf (the asymmetric tell, opposite the
  Cur's pauldron). Body P1_CYAN, shadow DEEP_TEAL, eye-slit RECALL-style cool
  glow + HIT_WHITE core. Same animation timing, distinctly different
  silhouette — a narrow upright column vs. the Cur's broad wedge (see the
  `Build` profiles in `scripts/generate_polished_assets.py`).

## Pixel Craft Standards

Acceptance checklist for every sprite — the generator loop iterates
until every item passes.

1. **Silhouette first**: flood the sprite to one color; it must still
   read (pose, direction, which player) at 50% zoom.
2. **Clusters, not noise**: shading in connected 2+ px clusters; no
   checkerboard dithering at this scale; no orphan single pixels except
   deliberate sparks/eyes.
3. **No pillow shading**: one light direction (top-left), shadow side
   committed.
4. **Outline**: 1 px, CHARCOAL_LINE or darker body shade — broken
   (skipped) on the lit edge for pop.
5. **No banding**: no parallel 1-px bands hugging the outline.
6. **Anim physics**: every action gets anticipation and follow-through;
   fast motion gets smear frames, not more in-betweens; loops hit the
   same pixel positions at wrap.
7. **Palette discipline**: the 16 colors only; player-identity colors
   never appear on the other player or neutral props (Recall Blue is the
   boomerang-return channel, Ember/Spark are heat accents, Hit White is
   reserved for impact).
8. **Two-mode check**: composition-mode assets (idle, tiles, floors) are
   quiet — ≤4 colors, low contrast; contact-mode assets (bursts, death,
   gore) spike contrast and pixel energy.

## Asset Table

Sheet dims below are the measured PNGs / generator `Canvas` sizes / code
`from_grid` args (the single source of truth); render-world sizes are the
in-code `custom_size`.

| Asset | Path | Source Cell | Layout | Sheet px | Render (world) |
|-------|------|------------|--------|----------|----------------|
| P0 duelist (Cur) | sprites/players/duelist_a_sheet.png | 48×48 | 41×1 | 1968×48 | 64×64 |
| P1 duelist (Stag) | sprites/players/duelist_b_sheet.png | 48×48 | 41×1 | 1968×48 | 64×64 |
| Bone fang | sprites/projectiles/bone_fang.png | 12×12 | 1×1 | 12×12 | 40×40 |
| Fang trail † | sprites/projectiles/bone_fang_trail_sheet.png | 12×12 | 6×1 | 72×12 | — |
| Fang marked † | sprites/projectiles/bone_fang_marked_sheet.png | 12×12 | 4×1 | 48×12 | — |
| Hit burst | sprites/particles/hit_burst_sheet.png | 24×24 | 4×1 | 96×24 | 64×64 |
| Death burst | sprites/particles/death_burst_sheet.png | 24×24 | 6×1 | 144×24 | 80×80 |
| Recall pulse | sprites/particles/recall_pulse_sheet.png | 16×16 | 4×1 | 64×16 | 48×48 |
| Ambient ember | sprites/particles/ambient_ember_sheet.png | 8×8 | 4×1 | 32×8 | 8×8 |
| Stains P0/P1 | sprites/stains/p{0,1}_stain_sheet.png | 16×16 | 4×1 | 64×16 | 32×32 |
| Bone pyre | sprites/arena/bone_pyre_sheet.png | 32×32 | 3×1 | 96×32 | 64×64 |
| Altar sigil | sprites/arena/altar_sigil_sheet.png | 32×32 | 2×1 | 64×32 | 64×64 |
| Sigil door | sprites/arena/sigil_door_sheet.png | 32×32 | 2×1 | 64×32 | 64×64 |
| Bone bridge tile | sprites/arena/bone_bridge_tile.png | 32×64 | 1×1 | 32×64 | per-arena |
| Chasm strip | sprites/arena/chasm_strip.png | 32×64 | 1×1 | 32×64 | per-arena |
| Pickups | sprites/pickups/pickup_sheet.png | 24×24 | 6×1 | 144×24 | 1.4× hitbox |
| Tiles ‡ | arenas/tile_sheet.png | 16×16 | 4×3 | 64×48 | composed |
| Arena floors | arenas/{anchor,crossing,reliquary}_floor.png | — | composed | 320×480 | covers ~1100×1600 cm |
| HUD set § | hud/*.png | per file | per file | per file | overlay |

† Generated but **not loaded** by the game — on-fang blood-marks were cut
and the flight trail reimplemented as render ghost-stamps; see
`docs/DESIGN_DIRECTION.md` § Boomerang. ‡ Source tiles for the composed
`training_floor.png`; not loaded directly at runtime. § `score_pips`,
`timer_digits`, `countdown_digits`, `match_over_badge`, `touch_controls`
— the in-match HUD (`app/src/hud.rs`) uses `score_pips.png` (now a 3-cell
atlas) + `countdown_digits.png` + a code-drawn timer bar; `match_over_badge`
/ `touch_controls` / `timer_digits` remain unused. `scrub_bar_*` /
`frame_step_buttons` are loaded by `replay_viewer`. Each arena now loads its
own retinted floor (`{anchor,crossing,reliquary}_floor.png`); `training_floor.png`
is kept as the Anchor-identical base.

## Visual Rules

- The boomerang should be the brightest moving object unless a hit
  flash is active.
- Player silhouettes should read at thumbnail size. Details inside the
  silhouette are optional; the outline is not.
- Hit effects are short, geometric, and source-readable.
- Each death paints a persistent floor stain at the kill location.
- Ambient particles must never cross the same value range as gameplay
  particles.
- Arena floors should use texture density sparingly.
- Do not use bloom, blur, large translucent clouds, or fullscreen
  color washes for gameplay-critical information.
- Color grading, screen shake, particles, persistent stains, and
  camera flourishes are render-only and must never feed back into sim
  state.

## Generation Strategy

All art is deterministically generated from ASCII pixel grids by
`scripts/generate_polished_assets.py`. The generator writes a contact
sheet to `assets/concepts/contact_sheet_v2.png` showing every sheet at
2× with labels for visual review.
