# 2-Top Style Bible

The opinionated anchor for the pixel art — what makes it *this game's*, not
generic. Research is blunt that autonomous AI art loops regress to the mean;
originality here is a **consistent, enforced set of choices**, not freestyling.
All art is code-generated (`scripts/generate_polished_assets.py`), palette-
locked, deterministic, and verified by gates (see § QA). No external editor.

See also: `ART_DIRECTION.md` (sizes/animation), `docs/DESIGN_DIRECTION.md`
(the v2 decisions), `VISUAL_TARGET_PACK.md` (aesthetic target).

## North star

Gore-pixel revival (Doom/Duke attitude: chunky, hard contrast, blood) ×
Hyper Light Drifter slickness (restrained palette, composed quiet, considered
animation). **HLD composes the room; gore fills it when blood spills.** Quiet
between exchanges, proud-violent during them. The arena remembers each kill.

## The palette is law — 16 colors, structured as ramps

`assets/palettes/two_top_16.gpl` is the single source of truth (mirrored in
`render::palette`). It is not a random 16 — it is **four hue-shifted ramps**
that all branch dark→light, which is what produces cohesion:

| Ramp | dark → light |
|---|---|
| **Heat / red** (Cur) | Bruise Shadow → Blood Dark → P0 Blood → Ember → Spark |
| **Cool / cyan** (Stag) | Bruise Shadow → Deep Teal → P1 Cyan → (Hit White) |
| **Bone** | Void → Warm Bone Shade → Bone → Hot Bone |
| **Stone** | Void → Deep Ash → Bruise Shadow → Charcoal Line → Cold Stone |

Plus channel reserves: **Recall Blue** = boomerang-return only. **Hit White** =
impact (+ sparing cool gleams). **Spark** = the hottest pixel, reserved for
eyes / heat cores.

## Craft laws (enforced in the generator)

1. **Committed light, top-left.** Shading is directional, never pillow. The lit
   top/left rim steps one toward LIGHT; the bottom/right rim steps one toward
   DARK (`_shade` for duelists, `_ramp_shade` for other lit assets).
2. **Hue-shift the ramp, don't just dim.** Light shifts warm (red→ember→spark),
   shadow shifts cool (→bruise/deep-teal). Same color count, far more depth.
3. **Directional light is for lit *material* only.** Foreground sprites
   (duelists, pickups, bone pyre) get it. **Emissive** assets (VFX bursts,
   glowing sigils/doors), **tiling** assets (bridge/chasm — seams), and the
   **quiet floors** use ramp *colors* but no directional light — side-lighting
   a glow or an explosion is a craft error.
4. **Silhouette first.** Every actor must read as a solid-black shape at
   thumbnail size. Identity is body shape, then color — never color alone.
5. **Clusters, not noise.** Shade in 2+px connected clusters; no checkerboard
   dither at this scale; orphan single pixels only for deliberate sparks/eyes.
6. **The floor stays quietest.** Tier-6: darkest palette roles only, never
   brighter than the dimmest gameplay pixel. Procedural blends snap to palette
   (`_nearest_palette`) — a smooth gradient isn't pixel art.

## Signature motifs (the "tell" — keep these consistent)

- **Cur vs Stag as warm-vs-cool, broad-vs-tall.** The Cur (P0) is a low broad
  forward wedge, **warm molten** rim-light (ember). The Stag (P1) is a tall
  narrow upright column, **cool crisp** (deep-teal/bruise shadows, hit-white
  corner gleams). Read apart in pure black *and* in temperature.
- **Spark eyes are always the hottest pixel on a body.**
- **Occult bone-cathedral.** Bone crenellation, a central duel-sigil, teal
  ritual rings, stacked-skull pyres. Sigils glow teal→recall-blue when active.
- **Blood-marked arena.** Violence lives in the *floor* (persistent kill
  stains, victim-colored), never caked on the priority-#1 boomerang.
- **Asymmetry as identity.** The Cur's bone pauldron (one shoulder); the Stag's
  trailing tail (one side). A broken silhouette reads as a hand, not a stamp.

## How to make art here (the vision loop)

1. Edit the parametric generator (a `Build` profile, a ramp, a part function).
2. Regenerate: `python3 scripts/generate_polished_assets.py`.
3. **Read the PNG and critique it** against the laws above (silhouette, palette,
   readability, hue-shift present, clusters). Iterate.
4. Gate before commit (also enforced in CI):

```sh
python3 scripts/check_palette.py       # every pixel ∈ the 16
python3 scripts/check_silhouettes.py   # Cur vs Stag read apart
```

Never reach for a non-deterministic generator or external editor for shipped
assets — the generator is the source of truth, and determinism is load-bearing
(the cross-platform matrix checks committed bytes).
