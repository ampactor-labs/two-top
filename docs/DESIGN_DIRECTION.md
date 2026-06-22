# Design Direction — v2 art decisions

Locked decisions for the four open art questions surfaced by the v2
graphics audit. Method: a web-researched expert panel role-playing five
designers/devs whose work is directly upstream of this game —
**Alx Preston** (Hyper Light Drifter — the literal "Lineage B" north
star), **Maddy Thorson** (TowerFall — fast one-hit pixel arena versus,
the closest cousin), **Jan Willem Nijman** (Vlambeer, *The Art of
Screenshake* — game feel), **David Sirlin** (Fantasy Strike, *Playing to
Win* — competitive clarity), and **Pedro Medeiros / saint11** (Celeste
pixel art — silhouette craft).

All five agreed on all four decisions. This doc is the synthesis;
`ART_DIRECTION.md` remains authoritative on sizes/palette/animation.

The single cross-cutting verdict: **kill the vector default font.** A
vector font next to nearest-neighbor sprites is the most style-breaking
thing in the build ("screams placeholder"). Every text element below is
a pixel-art sheet.

---

## 1. Player silhouette — ✅ DONE

**Decision (unanimous, rated the #1 issue):** the Cur and Stag must read
apart by **body shape** under the silhouette-flood test (fill solid
black; distinguishable at thumbnail size, under video compression, for
color-blind players), not by color or headgear alone.

**Implemented.** `scripts/generate_polished_assets.py` now carries two
`Build` profiles threaded through every body-drawing function:

- **Cur (P0):** broad, low, forward — wide shoulders/stance, head sunk
  into the shoulders (no neck), bone pauldron, curled bull horns. A
  low broad wedge. *(Pixels unchanged from before this pass.)*
- **Stag (P1):** tall, narrow, upright — pulled-in shoulders/arms,
  legs close, head lifted onto a visible **neck**, branching antler
  crown raised, and a trailing **tail** breaking the lower-left
  silhouette. A narrow vertical column.

Verified: both pass the flood test as distinct shapes; idle footprint
21px (Cur) vs 17px (Stag). The Stag re-proportioned; the Cur is
byte-identical.

---

## 2. In-match HUD — minimal pixel overlay — ✅ DONE

**Decision (unanimous): build a HUD — but the smallest one that can
exist.** Shipping with no HUD plus a vector font hides the win condition
(score) and the pace-maker (clock) of a 30s, one-hit, best-of-5 duel.
Minimalism here is "no numbers/chrome," not "no information." It lives
at the bottom of the readability hierarchy (tier 5) and never competes
upward.

Show **exactly three** things, as **overlay** (not diegetic — a phone is
too small and a duel too fast for a diegetic counter), nearest-neighbor,
consuming the existing unused `hud/*.png` sheets:

- **Score pips** — five pips per player, top corners (P0 left / Blood,
  P1 right / Cyan). Empty = Charcoal Line outline on Deep Ash; won =
  solid team color with a 1px Hot Bone inner highlight (so a filled pip
  reads even in the team hue). At **match point**, the next-needed pip
  pulses Hot Bone ~2 Hz (stakes felt, not read). `score_pips.png`.
- **Round clock** — top-center. A thin depleting bar (Preston/Thorson
  preference) *or* the 2-digit `timer_digits.png` (Sirlin/Nijman/
  Medeiros) in Bone, shifting to **Ember in the final ≤5s** as the only
  emphasis (no flashing). Pick the bar first; it's the more HLD-quiet
  read. Keep both options open until in-game test.
- **Countdown** — big centered `countdown_digits.png` (3·2·1·GO), one
  Hit White screen-flash + the existing camera zoom-punch on GO.

**Do not show:** health/energy bar (one-hit — the body *is* the health
bar), boomerang-state icon (never proxy priority #1), names, damage
numbers, minimap, kill feed. HUD palette: only Cold Stone / Charcoal
Line / Deep Ash for the neutral frame + the two team hues; borrow
nothing from Hit White or the Blood/contact channels so combat flashes
never read as HUD.

Mobile: hug the top safe-area (notches/thumbs eat the bottom corners);
touch hit-regions ≥ iOS 44pt / Android 48dp.

---

## 3. Boomerang marks & trail — ✅ DONE

**Decision (unanimous, every panelist's "sharpest cut"):** cut the
accumulating blood-marks **off** the boomerang; **implement** a short
flight trail.

- **On-fang blood-marks: CUT.** Caking damage texture onto the #1
  readability object over a round is "detail attacking the read." The
  "arena remembers the violence" fantasy already lives correctly in the
  floor stains (tier 6). `bone_fang_marked_sheet.png` is retained for
  reference but not loaded.
- **Flight trail: IMPLEMENT**, as a render-side readability instrument,
  not a decorative smear:
  - 3–6 discrete **nearest-neighbor ghost-stamps** of the live 12px fang
    sampled along the interpolated path — **not** an alpha-blur ribbon
    or a pre-baked sheet (honors no-bloom / nearest-neighbor).
  - **SHORT** (~one body-width / ≤6 frames of history) so it never forms
    a wall across the arena.
  - **Color encodes state** (the load-bearing part): outbound trail in a
    quiet owner/neutral channel; on **recall, the whole trail switches
    to Recall Blue** so "it's coming back to me" reads with no HUD. When
    a modifier is active, the trail may tint to that modifier's color
    (surfacing modifier state = good). **Kill the trail entirely while
    the fang is held/inert.**
  - Render-only, sim-independent (sampled from interpolated transforms);
    never feeds rollback.

`bone_fang_trail_sheet.png` is superseded by this ghost-stamp approach.

---

## 4. Arena floor identity — one quiet language, per-arena retint — ✅ DONE

**Decision (clear majority): do NOT paint three busy bespoke floors.**
Keep one shared quiet composition language; let the **props + interactive
geometry** (bone bridge + chasm, sigil doors) carry identity, with at
most one restrained per-arena move. Three loud floors would be three
simultaneous readability-hierarchy violations.

Shared "quiet floor" contract (enforce literally): the floor uses only
the darkest 3–4 palette roles (Void / Deep Ash / Bruise Shadow /
Charcoal Line, at most Cold Stone for line-work) — **never** team colors,
Bone/Hot Bone/Hit White, or Ember/Spark — so the brightest floor pixel
is structurally dimmer than the dimmest gameplay pixel. The center
duel-diamond is the shared anchor across all three.

Per-arena identity = one quiet **hue-register** shift (value-identical,
hue only, so readability is untouched) + the props:

- **Anchor** — neutral Deep Ash / Charcoal grid. The baseline "home"
  stage, honest and symmetric.
- **Crossing** — colder (Bruise Shadow / Cold Stone cast); the chasm is
  literal **Void** (reads as "no floor / death"), the bone bridge
  Warm-Bone-Shade glowing warm against it (the bridge *is* the
  figure-ground event), altar sigils in Deep Teal.
- **Reliquary** — warmer-but-dead (Deep Ash with a Bruise Shadow
  undertone), Deep-Teal sigil-door glow for the sealed-temple mood.

Stop shipping a single file literally named `training_floor.png` for all
three; produce `anchor/crossing/reliquary_floor.png` as retints of the
shared composition (or retint at load), and update the loader to pick by
`SelectedArena`.

---

## North-star note

The gore × HLD synthesis holds, but the panel flagged it is currently
**unbalanced toward the HLD (quiet) half by accident** — the discipline
is built (quiet floor, restrained palette, clean read) but the contact
*punch* that makes the synthesis sing is under-built. The contact-mode
juice (kill flash, hit-pause, the GO/kill screen-punch) should be pushed
harder so the room is genuinely "quiet between exchanges,
proud-violent during them."

## Status

| # | Decision | Status |
|---|----------|--------|
| 1 | P0/P1 body silhouette | ✅ implemented (generator `Build` profiles) |
| 2 | Minimal pixel-art HUD | ✅ implemented — `app/src/hud.rs`: score pips + depleting timer bar + 3·2·1/GO countdown, consuming `hud/score_pips.png` (now a 3-cell atlas) + `hud/countdown_digits.png` |
| 3 | Cut on-fang marks; build ghost-stamp trail | ✅ marks cut (unloaded); ghost-stamp trail implemented — `render::spawn_boomerang_trail`, Recall-Blue on return |
| 4 | One quiet floor + per-arena retint | ✅ implemented — `arena_floor()` palette-swap retints + arena-aware loader with live lobby preview |

### Still open (follow-ups)

- **HUD on mobile**: the HUD is world-anchored to arena corners (correct on the
  desktop whole-arena camera); a dedicated screen-space HUD camera for the
  mobile follow-cam is the remaining refinement.
- **Match-over badge**: the `MatchOver` summary is still vector text;
  swapping it for `hud/match_over_badge.png` + pixel score is a small follow-up.
- **Contact-mode punch** (north-star note): push kill flash / hit-pause / GO
  screen-punch harder so the gore×HLD contrast sings.
