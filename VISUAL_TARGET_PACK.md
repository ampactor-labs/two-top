# Visual Target Pack

This is the visual direction target for 2-Top. It is meant for the art
track while Claude Code continues systems work. The target pack is
grounded in `ARCHITECTURE.md`, `CONVENTIONS.md`, and `ART_DIRECTION.md`:
competitive readability first, occult pixel-art identity second,
ornament last.

## Direction

**Bone Cathedral, blood-marked.** Hyper Light Drifter discipline
composes the room; gore-pixel revival fills the room when blood spills.
The arena remembers the violence — kills paint the floor until the
round resets. The cathedral is quiet between exchanges and
proud-violent during them.

This is the synthesis Morgan named: gore-pixel revival × HLD slickness.
Not "demonic HLD" alone (too quiet), not Doom/Duke alone (too noisy).
Both lineages, applied per-mode.

Boards:

- `assets/concepts/target_pack/synthesis_showcase.png` — the chosen
  direction in motion, with floor stains, kill frame, attitude poses,
  and a boomerang accumulating blood marks across rounds.
- `assets/concepts/target_pack/bone_cathedral.png` — the same palette
  at rest, no contact yet.
- `assets/concepts/target_pack/visual_target_overview.png` — 4-up
  comparison of the directions considered.

These boards are aesthetic-direction targets, not production sprites.
They lock palette balance, silhouette language, and the two-mode rule.
Final pixel art is still upstream.

## Why this direction

- The boomerang stays the brightest moving object without bloom.
- The arena reads as occult and hostile while staying quiet under the
  play lane between exchanges.
- Bone, blood-red, and cyan give strong player/projectile separation on
  a dark field.
- The silhouette language works at 24×24 px: P0 is horned and compact;
  P1 is antlered and wider.
- The palette supports future arenas without turning the whole game
  into one hue family.
- The two-mode rule keeps the game from feeling either constantly-loud
  (gore-revival without HLD) or distant-pretty (HLD without
  gore-revival).

## Direction Boards

| Direction      | File                                        | Verdict                                        |
| ---            | ---                                         | ---                                            |
| Bone Cathedral | `assets/concepts/target_pack/bone_cathedral.png` | Direction target (composition mode)        |
| Synthesis      | `assets/concepts/target_pack/synthesis_showcase.png` | Direction target (contact mode)        |
| Neon Coven     | `assets/concepts/target_pack/neon_coven.png`     | Good marketing energy, too much VFX competition |
| Ash Chapel     | `assets/concepts/target_pack/ash_chapel.png`     | Elegant, but too warm and too low-contrast |
| Bloodglass Pit | `assets/concepts/target_pack/bloodglass_pit.png` | Strong, but reads more sci-fi arcade than demonic duel |

## Two Modes — the load-bearing rule

The synthesis lives in two modes that the renderer switches between
hard, frame to frame.

**Composition mode** (idle, walk, dash, in-flight throw, recall in
transit). HLD discipline.
- Restrained value range.
- Cathedral reads quiet.
- Floor texture stays under the play lane.
- No VFX clutter, no extra particles.
- Players read by silhouette, not by emission.

**Contact mode** (hit landing, kill frame, death, recall snap-catch,
boomerang ricochet). Gore-pixel revival.
- 3–4 frames of frozen-time chunky carnage.
- Hit-white flash, then radial blood spray in player color, then return
  to composition.
- The kill is a moment, not a state. Don't smear it across seconds.
- Body chunks and bone shards are okay. Glow, smoke, alpha haze are
  not.

The arena and the boomerang carry the *memory* of contact mode into
composition mode (see Blood-Marked below). Everything else returns to
quiet.

If you can't tell whether an effect belongs to composition or contact,
it doesn't belong. Cut it.

## Blood-Marked — the synthesis primitive

The arena remembers each kill until the round resets.

- A death paints a floor stain at the kill location, in the victim's
  color. The stain uses the player's **dark** palette slot
  (`Blood dark` for P0 deaths, `Deep teal` for P1 deaths) — the same
  desaturated value already used for player shadow. No new palette
  slot needed.
- Stain shape is hand-crafted, not procedural: a chunky core, a mid
  ring, an outer fleck pattern. It must read as "splat" in 6–10
  pixels at native scale, not as a Perlin smear.
- Stains are render-only state. They do not influence sim, AI, or
  pickups. Reset on round end.
- The boomerang accumulates a small blood mark per kill it lands —
  cosmetic only, never feeds back to sim. Three kills in a round and
  the fang reads "this thing has been busy" without breaking the
  silhouette.

This is the discipline answer to "where does the gore-revival pole
live in a quiet game?" It lives in the geometry, not in constant
particle emission.

## Production Rules

### Values

Keep gameplay layers in this value order:

1. Hit flash and countdown: highest value, shortest duration.
2. Boomerang body: second-highest value, persistent while active.
3. Player eyes, horns, and outer silhouette accents.
4. Player body fills.
5. HUD pips and timer.
6. Walls and hazards.
7. Floor texture, persistent stains, ambient particles.

If two adjacent layers share a value range, darken the lower-priority
layer before brightening the higher-priority layer.

Stains land on layer 7 for a reason — they should *be present* without
ever competing for the eye against the active boomerang or a hit. If a
stain reads above the boomerang in any frame, darken the stain.

### Players

Player source frames stay 24×24 px. Render scale stays 2x for a
48-world-unit body unless playtesting proves it needs adjustment.

P0:

- Compact horned silhouette.
- Red body, blood-dark shadow.
- Throw pose is a *forward lean with weapon thrust* — weight on lead
  foot, rear leg trailing. Not a centered idle with an arm extension.
- Dash smear uses ember/spark, not white.

P1:

- Wider antlered silhouette.
- Cyan body, deep-teal shadow.
- Throw/dash smear can use recall blue.
- Antlers must differ from P0 horns even in monochrome.

Both:

- Eye pixels use hot bone or hit white sparingly.
- Legs are tiny black anchors.
- Body outline carries readability; internal detail is optional.
- Hit frames may flash white, but only for 2–4 frames.
- Posture has weight. Idle is centered; throw leans into the throw;
  taunt opens the shoulders. Duke posture inside an HLD-disciplined
  silhouette.

### Boomerang

The boomerang is the game's visual protagonist.

- 12×12 px source sprite.
- Bone/hot-bone body with warm shade underside.
- Asymmetric fang shape, not a generic crescent.
- A tiny carved sigil along the spine — readable as "marked weapon",
  not as decoration.
- Flying trail: spark/ember dots, short and broken.
- Returning trail: recall-blue ticks mixed with bone, distinct by
  shape as well as color.
- Per-round blood-mark accumulation: 0 / 1 / 2 / 3+ kills paints
  flecks of the victim's color onto the body. Cosmetic only.
- No blurry glow, no smoke trail, no alpha haze.

### Kill Frame

When a hit lands, the renderer freezes time briefly.

- Frame 0: hit-white silhouette flash on the victim (2 frames).
- Frame 1: chunky radial blood spray in the victim's color, hard
  pixels, 8–12 droplets, 4 frames.
- Frame 2: bone shards and chunks scatter outward, 6 frames.
- Frame 3: persistent floor stain commits at the body's last
  position. Round-scoped.

Camera does not shake during contact mode. Screen shake is reserved
for round-level events (round start, kill confirmation badge). The
freeze + flash + spray + stain sequence carries the impact without
camera intervention — and stays deterministic-friendly (sim-derived
timing, no render randomness).

### VFX

Effects are geometric, short, and source-readable.

- Hit burst: 4 frames, radial hard-pixel spokes, hit white first,
  spark second.
- Death burst: 6 frames, body-color shards plus ember sparks, then the
  corpse mark commits.
- Recall pulse: small inward blue ticks, never a full-screen wash.
- Ambient embers: isolated 1–3 px chunks, dark enough to never be
  confused with boomerang trail.
- Persistent floor stains and the corpse mark live outside the
  particle budget — they are arena state for the round.

The rule is one bright idea per event. A hit can flash and burst; it
cannot also bloom, smoke, shake particles, and wash the screen.

### Arenas

Arena art must never compete with the boomerang.

Training arena:

- Dark stone floor.
- Low-contrast checker breaks for motion parallax.
- Thin center line and duel diamond.
- Spawn marks in muted player-color darks.
- Wall edge/corner motifs in bone shade, not spark yellow.

Future arenas can change motifs and ambient wash, but player colors,
boomerang values, HUD values, and the stain-color rule remain fixed.
Floor texture should stay quiet enough that stains read against it.

### HUD and Touch

HUD should feel like a fighting-game instrument panel, not a
decorative fantasy frame.

- Score pips: small, high-contrast, red/cyan paired with positional
  grouping.
- Timer: hit-white digits with restrained spark edge.
- Countdown: large pixel glyphs centered over the arena, no text
  labels.
- Touch controls: visible rings are subtle; hit regions are larger
  than visuals and follow iOS 44 pt / Android 48 dp minimums.

Do not rely on color alone. Use side, grouping, shape, and silhouette.

## Final Asset Pass Order

1. Replace the placeholder boomerang and trail first.
2. Build one complete P0 sheet — including the lean-throw posture and
   kill-frame variant. Test in-game before P1.
3. Build P1 by silhouette variation, not just palette swapping.
4. Implement hit burst, death burst, recall trail, and the kill-frame
   sequence (flash → spray → shards → stain commit).
5. Implement the persistent stain system in the render layer.
   Render-only, round-scoped.
6. Replace the training arena floor/walls.
7. Replace HUD pips and countdown.
8. Only then start arena 2/3 art.

This order protects the core read: projectile, players, hit
confirmation, persistent kill record, arena, HUD.

## Image Generation Prompts

Use these for the first true image-generation pass. Outputs are
concept source, not final sprites.

### Bone Cathedral, Blood-Marked Character Sheet

Use case: stylized-concept
Asset type: pixel-art character concept sheet.
Primary request: two demonic duelists for a portrait 1v1 mobile
rollback brawler called 2-Top. Readable at 24×24 px source size,
top-down / slight three-quarter view. P0 is a compact red horned
duelist; P1 is a wider cyan antlered duelist. Both use bone-colored
horns, tiny bright eye pixels, hard dark outlines, distinct
silhouettes, and posture with weight (idle centered, throw leaning
forward, taunt with shoulders open).
Style: disciplined limited-palette pixel art at composition rest,
chunky-pixel gore-revival posture and hit/kill frames. Bone Cathedral
tone — dark stone, occult restraint at idle; visceral chunky carnage
at contact.
Palette intent: void black, deep ash, bruise shadow, bone, hot bone,
blood red, ember, spark yellow, deep teal, cyan, recall blue, hit
white.
Must show: idle, lean-throw, dash, taunt, hit, kill-frame (white
flash + chunky blood spray + bone shards), and death/corpse-mark
poses for both players.
Avoid: text, gradients, rim-lit 3D rendering, smoke, blurry glow,
busy interior costume detail, color-only identity.

### Bone-Fang Projectile and VFX

Use case: stylized-concept
Asset type: pixel-art projectile and combat VFX concept sheet.
Primary request: asymmetric bone-fang boomerang for 2-Top, readable
at 12×12 px source size. Carved sigil along the spine. Per-round
blood-mark accumulation: clean / 1-mark / 2-mark / 3-mark variants.
Flying trail (spark/ember), returning trail (recall-blue ticks
mixed with bone), hit burst (radial spokes), death burst (body-color
shards + bone fragments), recall ticks, and ambient embers.
Style: hard-edged chunky pixels, geometric VFX, high competitive
readability, bone projectile brighter than the environment.
Must show: clean projectile, 3 marked variants, 3 flying trail
frames, 3 returning trail frames, 4 hit-burst frames, 6 death-burst
frames, 4 ambient ember frames.
Avoid: smoke clouds, blurry glow, translucent haze, full-screen
washes, complex background, text.

### Training Arena (Composition + Contact States)

Use case: stylized-concept
Asset type: pixel-art arena concept and tile motif.
Primary request: open-box training arena for 2-Top in a dark Bone
Cathedral style. Top-down portrait arena, quiet stone floor, subtle
center duel diamond, thin center line, muted red/cyan spawn marks,
bone-shade wall edge motifs. Show two states: composition (clean
between rounds) and contact-aftermath (with persistent floor stains
in player-dark colors at kill positions, no other added VFX).
Style: restrained occult pixel art, low-contrast floor detail, clean
competitive readability, stains hand-crafted not procedural.
Must show: floor tile, wall edge, wall corner, spawn mark, center
duel motif, P0 stain, P1 stain, corpse mark.
Avoid: bright yellow floor accents, dense center texture, large
ornate frames, text, gradients, particle haze, atmospheric fog.

### HUD and Touch

Use case: stylized-concept
Asset type: compact pixel HUD and touch-control concept sheet.
Primary request: HUD for a portrait 1v1 mobile brawler: red/cyan
score pips, 30-second timer, 3-2-1-GO countdown glyphs, match-over
badge, subtle virtual stick ring and throw touch ring states.
Style: minimal high-contrast pixel UI, occult but utilitarian,
readable over dark arena.
Constraints: visible controls are subtle, but implementation hit
regions must meet iOS 44 pt and Android 48 dp guidance. Information
must not rely on color alone.
Avoid: explanatory labels, ornate fantasy frames, decorative
flourishes that compete with gameplay.

## Acceptance Tests

Before calling any art final, capture screenshots at phone aspect
ratios and check:

- Can the boomerang be found in under half a second?
- Can P0/P1 be identified in grayscale?
- Does a hit event identify source direction and victim?
- Does the kill frame freeze read as proud-violent without bloating
  past 12 frames total?
- Do persistent stains stay readable as "old kill" without crossing
  the value range of active gameplay?
- Does the floor disappear behind players instead of competing with
  them?
- Are HUD values readable over every arena, including a
  late-round arena thick with stains?
- Are touch visuals visible without becoming UI clutter?
- Do animation frames snap cleanly with nearest filtering?

If the answer to any of those is no, the asset is still concept art.
