# Art Direction

Phase 15 prep. This document does not supersede `ARCHITECTURE.md`,
`BUILD_PLAN.md`, or `CONVENTIONS.md`; it turns their visual intent into
an asset contract.

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
1v1 needs. Both poles, applied per-mode (see `VISUAL_TARGET_PACK.md` §
Two Modes).

The priority order is:

1. Boomerang position, state, and travel direction.
2. Player body position, facing, dash, stun, and death state.
3. Hit, recall, respawn, and round-state effects.
4. Walls, hazards, pickups, and arena bounds.
5. HUD, timer, score, and touch feedback.
6. Floor texture, persistent kill stains, ambient particles, and mood.

If an effect competes with a higher-priority item, cut the effect.

## Research Anchors

- Apple HIG game controls: frequent controls belong near the thumbs,
  controls must avoid device safe areas, and primary touch controls
  should be at least 44 by 44 pt with clear press states.
- Android accessibility guidance: touch targets should be at least
  48 by 48 dp, with about 8 dp spacing between adjacent targets.
- Riot's gameplay-clarity writing is the right model for a competitive
  game: characters and VFX must keep gameplay information visible, while
  environments stay cheaper, quieter, and less detailed.
- Xbox accessibility contrast guidance applies directly to the HUD and
  gameplay cues: important text and non-text elements need enough
  contrast against the current background.
- Pixel art references consistently point to the same constraints:
  limited palettes, strong value separation, readable silhouettes, and
  outlines that support shape before detail.

Useful references:

- https://developer.apple.com/design/human-interface-guidelines/game-controls
- https://support.google.com/accessibility/android/answer/7101858
- https://www.riotgames.com/en/news/valorant-shaders-and-gameplay-clarity
- https://www.riotgames.com/en/artedu/visual-effects
- https://learn.microsoft.com/en-us/gaming/accessibility/xbox-accessibility-guidelines/102
- https://lospec.com/palettes/

## Palette

Phase 15 asks for 16-color palette enforcement. Start with this working
palette and adjust only after testing screenshots on desktop and phone.
The importable palette file lives at `assets/palettes/two_top_16.gpl`.

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

## Asset Sizes

- Player source frames: 24 by 24 px, rendered at 48 by 48 world units.
  The collision body is smaller than the sprite, so the sprite can carry
  an outline, horns, and animation smear without changing gameplay.
- Boomerang source frames: 12 by 12 px, with a separate 3-frame trail
  strip. The boomerang must remain brighter than floor detail in every
  arena.
- Arena tiles: 16 by 16 px. Floor motifs should stay low contrast and
  never use the same high-value accents as players, boomerangs, or hits.
- Particles: 2 by 2, 3 by 3, and 4 by 4 px chunky sprites. Prefer hard
  opaque pixels over translucent haze.
- HUD icons: 16 by 16 px source, scaled through nearest filtering.
  Touch affordances can be code-drawn rings unless a raster treatment is
  needed later.

Use nearest-neighbor filtering for all sprite assets. Avoid fractional
pixel scaling where possible.

## Phase 15 Minimum Set

1. One full playable character sheet:
   - idle: 4 frames
   - throw: 6 frames
   - dash: 2 frames
   - hit: 2 to 4 frames
   - death: 6 frames
2. A second player treatment:
   - minimum: same animation timing, altered silhouette, different
     palette slots
   - better: separate head/shoulder silhouette and dash smear shape
3. Boomerang:
   - bone-fang projectile, 12 by 12 px
   - 3 trail frames
   - flying and returning must differ by shape or trail, not only color
4. Particles:
   - hit burst
   - boomerang trail
   - death burst
   - ambient ember
5. Training arena:
   - floor tile
   - wall edge/corner tiles
   - spawn marks
   - subtle center-line or duel-ring motif
6. HUD:
   - score pips
   - round timer treatment
   - countdown digits
   - match-over badge

## Visual Rules

- The renderer operates in two modes: **composition** (HLD-quiet) for
  idle/movement/in-flight throw, and **contact** (gore-revival
  proud) for hit/kill/death/recall-snap. Mode switches are hard, not
  blended. Composition is most of the round; contact is brief frames.
- The boomerang should be the brightest moving object unless a hit
  flash is active.
- Player silhouettes should read at thumbnail size. Details inside the
  silhouette are optional; the outline is not. Posture has weight —
  idle is centered; throw leans forward into the throw; taunt opens
  the shoulders.
- Hit effects are short, geometric, and source-readable. A player
  should know who caused a hit from the burst direction and color
  pairing. The kill-frame sequence (flash → spray → shards → stain)
  caps at ~12 frames total; everything else is composition mode.
- Each death paints a persistent floor stain at the kill location, in
  the victim's dark palette slot (`Blood dark` for P0, `Deep teal`
  for P1). The stain is hand-crafted (chunky core, mid ring, edge
  flecks), render-only state, and resets on round end. The arena
  remembers the round's violence; that is where the gore-revival pole
  lives between kills.
- The boomerang accumulates a small blood mark per kill it lands —
  cosmetic only, never feeds sim. Carved sigil along the spine
  signals "marked weapon", not decoration.
- Ambient particles must never cross the same value range as gameplay
  particles.
- Arena floors should use texture density sparingly. Mobile thumbs
  will already occlude the lower corners; the center play lane must
  stay clean. Stain density must stay readable late in a round.
- Do not use bloom, blur, large translucent clouds, or fullscreen
  color washes for gameplay-critical information.
- Color grading, screen shake, particles, persistent stains, and
  camera flourishes are render-only and must never feed back into sim
  state.

## Generation Strategy

Do not batch-generate every final sprite sheet at once. Generate in this
order:

1. Four concept sheets: character pair, boomerang/VFX, training arena,
   HUD/touch treatment.
2. Lock the palette and silhouettes from those sheets.
3. Generate or hand-pixel one character's complete animation sheet.
4. Implement the atlas path and nearest filtering.
5. Test gameplay readability on desktop and a real phone.
6. Generate the remaining final assets only after the first sheet works
   in-game.

AI-generated pixel art should be treated as concept or rough source
unless the output passes manual pixel cleanup. Frame-to-frame consistency
matters more than one beautiful still frame.

## Current Seed Pack

The first placeholder pack is generated by
`scripts/generate_placeholder_art.py`. Review the contact sheet at
`assets/concepts/phase15_contact_sheet.png`, then replace or refine the
individual files listed in `assets/README.md`.

The visual target comparison pack is generated by
`scripts/generate_visual_targets.py`. The chosen direction is
`Bone Cathedral, blood-marked`, documented in `VISUAL_TARGET_PACK.md`,
with boards in `assets/concepts/target_pack/`. The pack ships two
target boards — `bone_cathedral.png` (composition mode, at rest) and
`synthesis_showcase.png` (contact mode, with stains, kill frame,
attitude poses, and the boomerang's per-round blood-mark accumulation).

When image generation is available, start from the prompts in
`assets/concepts/phase15_image_prompts.md`. Do not treat those generated
images as implementation-ready until they have been reduced to the
working palette, checked for frame consistency, and tested in-game.
