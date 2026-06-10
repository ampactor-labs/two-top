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
one-hit kills, 60-second rounds, contact frames that *land*.

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

- Player source frames: **32×32 px**, rendered at **64×64 world units**.
  The collision body is smaller than the sprite, so the sprite can carry
  an outline, horns, and animation smear without changing gameplay.
- Boomerang source frames: **20×20 px**, rendered at **40×40 world units**.
  The boomerang must remain brighter than floor detail in every arena.
- Arena tiles: **32×32 px**. Floor motifs should stay low contrast and
  never use the same high-value accents as players, boomerangs, or hits.
- Particles: source cells vary per effect (see asset table below).
  Prefer hard opaque pixels over translucent haze.
- HUD icons: **2× the v1 cell sizes**, scaled through nearest filtering.
  Touch affordances can be code-drawn rings unless a raster treatment is
  needed later.

Use nearest-neighbor filtering for all sprite assets. Avoid fractional
pixel scaling where possible.

Rationale (recorded so nobody "fixes" it back): 32×32 @ 2× world scale
keeps the chunky gore-revival texel on a phone held at arm's length;
HLD-ness comes from animation acting + composition, not resolution.
Boomerang stays 40×40 world so gameplay feel is untouched.

## Animation Table (v2)

| Anim | ID | Frames | Atlas Offset | Ticks/Frame | Oneshot |
|------|----|--------|--------------|-------------|---------|
| IDLE | 0 | 6 | 0 | 9 | no |
| RUN | 1 | 6 | 6 | 5 | no |
| THROW | 2 | 8 | 12 | 3 | yes |
| DASH | 3 | 4 | 20 | 3 | no |
| HIT | 4 | 4 | 24 | 3 | yes |
| CATCH | 5 | 3 | 28 | 3 | yes |
| DEATH | 6 | 13 | 31 | 8 | yes |

Total atlas strip: **44 frames** per player sheet (44×1 @ 32×32 = 1408×32 px).

## Duelist Design

- **P0 "the Cur"**: compact, forward-hunched, two short bull horns,
  ragged half-cloak. Body P0_BLOOD, shadow BLOOD_DARK, horn/claw
  accents BONE, eyes SPARK (2 px, always hottest pixel on the body).
  IDLE = weight-shifting bob, asymmetric. RUN = hunched lope, horn-leading,
  directional smear with afterimage pixels in body color at 40% (use the
  darker body shade, not alpha). THROW = wind-up, cock, release, fly-out,
  recovery, settle. DASH = full horizontal blur. HIT = full HIT_WHITE
  silhouette flash 1 frame, then recoil. CATCH = arm snap up, 1-frame
  SPARK flash at the hand. DEATH = stagger, knee buckle, gore burst
  (P0_BLOOD/BLOOD_DARK chunks), collapse to a corpse pile that matches
  the stain corpse-mark.

- **P1 "the Revenant"**: tall, narrow shoulders, single swept-back horn,
  hanging sash/tail. Body P1_CYAN, shadow DEEP_TEAL. Same animation
  timing, distinctly different silhouette.

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

| Asset | Path | Source Cell | Layout | Sheet px | Render Size |
|-------|------|------------|--------|----------|-------------|
| P0 duelist | sprites/players/duelist_a_sheet.png | 32×32 | 44×1 | 1408×32 | 64×64 |
| P1 duelist | sprites/players/duelist_b_sheet.png | 32×32 | 44×1 | 1408×32 | 64×64 |
| Bone fang | sprites/projectiles/bone_fang.png | 20×20 | 1×1 | 20×20 | 40×40 |
| Fang spin | sprites/projectiles/bone_fang_spin_sheet.png | 20×20 | 4×1 | 80×20 | 40×40 |
| Fang trail | sprites/projectiles/bone_fang_trail_sheet.png | 20×20 | 6×1 | 120×20 | 40×40 |
| Fang marked | sprites/projectiles/bone_fang_marked_sheet.png | 20×20 | 4×1 | 80×20 | 40×40 |
| Hit burst | sprites/particles/hit_burst_sheet.png | 32×32 | 6×1 | 192×32 | 80×80 |
| Death burst | sprites/particles/death_burst_sheet.png | 48×48 | 10×1 | 480×48 | 112×112 |
| Recall pulse | sprites/particles/recall_pulse_sheet.png | 24×24 | 6×1 | 144×24 | 48×48 |
| Shatter burst | sprites/particles/shatter_burst_sheet.png | 32×32 | 6×1 | 192×32 | 80×80 |
| Stains P0 | sprites/stains/p0_stain_sheet.png | 24×24 | 4×1 | 96×24 | 48×48 |
| Stains P1 | sprites/stains/p1_stain_sheet.png | 24×24 | 4×1 | 96×24 | 48×48 |
| Bone pyre | sprites/arena/bone_pyre_sheet.png | 32×32 | 3×1 | 96×32 | 64×64 |
| Altar sigil | sprites/arena/altar_sigil_sheet.png | 32×32 | 2×1 | 64×32 | 64×64 |
| Sigil door | sprites/arena/sigil_door_sheet.png | 32×32 | 2×1 | 64×32 | 64×64 |
| Pickups | sprites/pickups/pickup_sheet.png | 24×24 | 6×1 | 144×24 | 48×48 |
| Tiles | arenas/tile_sheet.png | 32×32 | 6×1 | 192×32 | 64×64 |
| Arena floors | arenas/{anchor,crossing,reliquary}_floor.png | — | composed | 320×480 | covers 1000×1500 cm |
| Embers | sprites/particles/ember_sheet.png | 8×8 | 4×1 | 32×8 | 8×8 |
| HUD set | hud/*.png | 2× v1 | same layouts | 2× v1 | unchanged world |

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
