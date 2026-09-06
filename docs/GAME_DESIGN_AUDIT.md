# Game Design Audit — the match, not the art

`DESIGN_DIRECTION.md` audited the art and closed out. This is the other
half: the **game**, and the surfaces you actually touch to get into one.

Method: read the sim's tuning constants against the systems that consume
them, and the screen layer against its own coordinate conventions. Every
finding below cites the file and line that proves it. Where the operator
has played the build (couch and network, Sept 2026), their report is
treated as primary evidence and the code is read to explain it — not the
other way round.

The headline: **the round is a scoring no-op**, and the operator felt it
from the seat before anyone read the code. Everything else is smaller.

---

## 1. The round does not score — 🔴 open, structural

**Reported from play:** *"the clock is vestigial."* It is, and the
mechanism is worse than vestigial — the clock is a net negative.

`MatchScore` is a **cumulative kill counter**, not a round tally. Nothing
resets it at a round boundary: `reset_round_state` (`sim/src/lib.rs:1873`)
wipes fangs, `ThrowCapacity`, `ThrowCharge`, `CatchStreak`, `Taunt` and
`SpawnGuard` — and deliberately never touches the score.
`tick_match_state` (`:2996`) ends the match when either side crosses
`MATCH_WIN_THRESHOLD = 5` **kills**, checked identically inside `InRound`
and `RoundOver`.

So a round expiring changes nothing on the scoreboard. Trace what the
boundary actually costs:

| Beat | Frames | Consequence |
|---|---|---|
| `RoundOver` | 60 | none |
| `Countdown` 3·2·1 | 180 | none |
| **Total** | **240 (4.0 s)** | **none** |

Four seconds of dead time, per boundary, buying nothing. In a first-to-5
race that is a lot of standing around.

**And the boundary is not actually neutral — it is hostile.** The one
piece of state the clock *does* touch is `CatchStreak`, which it deletes.
The perfect-catch ladder is the game's signature skill expression: catch
inside `PERFECT_CATCH_WINDOW_FRAMES = 10`, earn `Empowered` (1.3× speed),
chain to `STREAK_LIGHTNING = 3` for full board reach at any charge
(`:2119`). A player who has built that ladder loses it to a timer that
otherwise means nothing. **The clock's only gameplay effect is to punish
the player who is playing best.**

The sudden-death crumble compounds it. `SUDDEN_DEATH_FRAMES = 480`
shrinks the safe floor to `0.4` of the arena over the last 8 s of every
round *"so no round peters out at range"* (`:595`). It is squeezing two
players toward a buzzer that has no stakes behind it. The pressure is
real; the deadline it points at is imaginary.

The code knows. `MATCH_WIN_THRESHOLD`'s own comment (`:1032`) admits the
divergence: *"BUILD_PLAN's older 'first to 5 round wins' framing is
satisfied in spirit."* It is not satisfied in spirit. Round wins and
cumulative kills are different games — the first bounds a comeback, the
second does not.

**Three ways out, cheapest first:**

- **(a) Delete the round.** One 5-kill race. Keep the crumble but drive
  it off a single match clock, so the squeeze aims at something. Cheapest
  change, removes the 4 s beats entirely, and makes the crumble honest.
- **(b) Make the round score.** Round winner = most kills that round (or
  first kill, TowerFall-style); `MatchScore` becomes round wins and
  *does* reset. Restores the comeback structure and gives the buzzer
  teeth. Most work, most conventional.
- **(c) Keep the shape, fix the theft.** Leave scoring alone but carry
  `CatchStreak` across the boundary and cut `ROUND_OVER_FRAMES` +
  countdown to one short beat. Smallest diff; the clock stays
  decorative but stops taxing the better player.

Any of these bumps `SIM_VERSION` (currently 14) and invalidates existing
tapes. That is the real cost, and it argues for doing it once, soon,
rather than twice.

---

## 2. Two screen-coordinate conventions — 🔴 open, root cause

**Reported from play:** *"some of the views are glitchy and
inconsistent."* There is a mechanical reason.

The app carries **two** screen-space systems and hand-converts between
them:

- **`ScreenAnchor`** (`app/src/anchor.rs`) — normalized `(-1..1)`,
  **y-up**, world units, re-derived each frame from the live camera rect
  after follow-cam, kill-cam zoom and shake. Adopted by 14 modules.
- **NameEntry's keyboard** (`app/src/profile.rs:61`) — window-fraction
  `(0..1)`, **y-down**, its own grid: `KEY_TOP = 0.44`,
  `KEY_ROW_H = 0.082`, `KEY_W = 0.094`.

The seam is visible in the source as inlined conversion arithmetic:

```rust
const NAME_ANCHOR_Y:  f32 = 1.0 - 2.0 * 0.335;   // profile.rs:59
const ENTRY_ANCHOR_Y: f32 = 1.0 - 2.0 * 0.28;    // profile.rs:63
```

That `1.0 - 2.0 * f` is a y-down-fraction → y-up-normalized conversion,
written by hand, twice, as a constant. Both are then fed straight into the
other system:

```rust
ScreenAnchor::new(0.0, NAME_ANCHOR_Y,  0.0, 0.0)   // profile.rs:365
ScreenAnchor::new(0.0, ENTRY_ANCHOR_Y, 0.0, 0.0)   // profile.rs:382
```

A layout authored in one coordinate space, converted by a literal, and
handed to a second space that re-derives itself from the camera every
frame. The conversion is correct today; nothing keeps it correct.

The failure mode this produces is **the drawn button and its tap target
drift apart**, because `key_center()` and `key_at()` compute in
window-fraction while the glyph renders through the anchor path. It is
exactly the bug class the capture harness keeps catching — see
`49858bc` *"three layout bugs die before any phone"* and `67f6419`
*"the two bugs it flushed out."* The harness is catching instances; the
convention split is the generator.

**Fix:** one convention. `ScreenAnchor` is the better-engineered of the
two (it is camera-derived and shake-stable by construction), so port
NameEntry's grid onto it and delete the conversion constants. Derive the
hit-test from the same anchor the glyph renders at, so they cannot
disagree.

---

## 3. The vector-font verdict shipped on one surface out of ten — 🟠 open

`DESIGN_DIRECTION.md` opens with a verdict it calls **cross-cutting** and
unanimous across all five panelists:

> **kill the vector default font.** A vector font next to
> nearest-neighbor sprites is the most style-breaking thing in the build
> ("screams placeholder"). Every text element below is a pixel-art sheet.

It shipped for the **in-match HUD** and nowhere else. There is no pixel
font in the build — `runes.rs` is the demon mint (chest marks + shadow
palette), not type. Vector `Text` sites by module:

| Module | Sites | Module | Sites |
|---|---|---|---|
| `theater.rs` | 19 | `share.rs` | 6 |
| `screen.rs` | 15 | `touch_controls.rs` | 6 |
| `profile.rs` | 9 | `room_code.rs` | 4 |
| `arena_select.rs` | 8 | `intro_card.rs` | 3 |
| `settings.rs` | 8 | `rivals.rs` | 2 |
| `lobby_overlay.rs` | 7 | | |

So the game switches typographic systems the moment you leave the match,
and switches back when you enter one. That *is* the inconsistency — it is
not a subtle one, and it is the single largest "screams placeholder"
surface left in the build.

`DESIGN_DIRECTION.md`'s status table marks this ✅ done via decision #2.
**That table overstates completion** and should be corrected: #2 delivered
the HUD; the cross-cutting verdict above it is ~10% shipped.

Cheapest honest fix is one pixel-font sheet + a `text()` helper, adopted
screen by screen, `theater.rs` and `screen.rs` first (34 of 87 sites).

---

## 4. Nearest-neighbour art at fractional scale, exactly when you stare
   at it — 🟠 open

`ImagePlugin::default_nearest()` is set (`app/src/lib.rs:118`), per the
art direction's no-filtering mandate. `apply_screen_anchors`
(`anchor.rs:142`) then does:

```rust
tx.scale = Vec3::splat(rect.scale);
```

`rect.scale` is the camera's live ortho scale — `1.0` at rest, animating
under the kill-cam. This is *correct*: it cancels the zoom so anchored UI
holds constant screen size, and the comment above it records the real bug
it fixed (summary text running off both edges, buttons overlapping, "on
every phone, in every correct match").

The side effect is that anchored pixel art renders at a **non-integer
scale** whenever the camera is not at rest — and per that same comment the
kill-cam *"zooms 1.6× and holds there for the whole MatchOver summary, by
design."* Nearest-neighbour sampling at 1/1.6 gives an uneven pixel grid:
some source pixels land on 1 screen pixel, some on 2. The result is
visible pixel crawl on the score pips and countdown digits, held on
screen for the entire summary — the moment the player looks longest.

**Fix:** snap anchored-UI scale to a rational step (nearest 1/2 or 1/4)
rather than the raw float, or exempt anchored pixel sheets from the zoom
cancellation and size them in screen pixels directly.

---

## 5. The anchor silently owns `Transform::scale` — 🟡 hazard, not yet a bug

`apply_screen_anchors` overwrites `scale` on **every** anchored entity,
every frame, unconditionally. Any future scale animation on anchored UI
will be erased with no error — or win on some frames and lose on others
if a system ordering changes, which reads as flicker.

The HUD currently threads this needle correctly and deliberately: the
match-point pulse drives **alpha** (`hud.rs:215`) and the pip-slam drives
**`sprite.custom_size`** (`hud.rs:209`), neither of which the anchor
touches. That is the right pattern — it is just undocumented, and the next
person to animate an anchored element will not know it.

**Fix:** one line in `ScreenAnchor`'s doc comment stating that the anchor
owns `Transform::scale` and that size animation goes through
`custom_size`.

---

## 6. Doc drift against code — 🟡 open, cheap

Three cases, all documentation-only:

1. **`DESIGN_DIRECTION.md` § Still open, item 1** claims the HUD is
   world-anchored and a screen-space HUD camera is "the remaining
   refinement." **Done** — `anchor.rs` exists and 14 modules use it.
   The follow-up should be struck.

2. **`BOOMERANG_HALF_EXTENT_CM`** (`sim/src/lib.rs:491`) — the comment
   reads *"Smaller than the player's 16 cm: ~10 cm gives a 20 cm
   catch/hit footprint."* All three numbers are stale: the constant is
   **13** (→ 26 cm footprint) and `PLAYER_HALF_EXTENT_CM` is **20**.

3. **`BOOMERANG_MAX_THROW_DISTANCE_CM = 1000`** (`:440`) documents itself
   as governing full-power throws — *"a full-power throw threatens most
   of the board."* It does not. Real throws always carry a `ThrowReach`
   (`:2138`), and `recall_boomerangs` only falls back to this constant
   when that component is absent (`:2599`), which is bare test fangs.
   The number that governs live play is `REACH_MAX_CM = 1100`.

---

## Scope note: where the effort went

Not a defect, but worth stating plainly, because it explains how #1
survived to a shipping build.

`app` is 15,893 lines. Roughly **5,000** of them are metagame and
side-channel: `theater` 1338, `profile` 845, `room_code` 657, `rivals`
619, `grudge` 546, `share` 452, `attest` 383, `shade` 215. Against that,
`hud.rs` is 288 lines and `touch_controls.rs` is 574.

The metagame is well built and the operator has played the game, so this
is not "shipped without testing." It is narrower than that: the six NORTH
pillars — rivalry ledgers, signed results, web theater, minted demons,
shades, the sit-down ritual — all describe, replay, and attest **the
outcome of a match**, and the match itself still doesn't know what a round
is for. The infrastructure for remembering results outgrew the rules that
produce them.

Finding #1 is cheap to fix and gates the meaning of everything downstream:
a rivalry ledger, a sealed `MatchStatement`, and a gauntlet tier all
inherit whatever "score" means. Settle it before more is built on top.

---

## Status

| # | Finding | Severity | Evidence |
|---|---------|----------|----------|
| 1 | The round does not score; the clock taxes the better player | 🔴 structural | `sim:1873`, `sim:2996`, `sim:1032` + play report |
| 2 | Two screen-coordinate conventions, hand-converted | 🔴 root cause | `profile.rs:59-63` vs `anchor.rs` + play report |
| 3 | Vector-font verdict shipped on 1 surface of ~10 | 🟠 open | ~96 vector `Text` sites; no pixel font |
| 4 | Nearest-neighbour UI at fractional scale during kill-cam hold | 🟠 open | `lib.rs:118`, `anchor.rs:142` |
| 5 | Anchor silently owns `Transform::scale` | 🟡 hazard | `anchor.rs:142`; HUD dodges it correctly |
| 6 | Doc drift (3 cases) | 🟡 open | `DESIGN_DIRECTION.md`; `sim:491`, `sim:440` |

### Recommended order

1. **#1**, choosing (a), (b) or (c) — one `SIM_VERSION` bump, done once.
2. **#2**, which retires a recurring bug class rather than another instance.
3. **#6**, minutes of work, and #6.1 is currently misleading a reader
   into redoing finished work.
4. **#5**, one doc comment, before someone trips it.
5. **#3** and **#4** together — both are "the UI layer doesn't match the
   art direction," and the font work will touch the same call sites.

### Not findings

Checked and sound: `SpawnGuard`'s break-on-act rule (`sim:182`) closes the
obvious offensive-shield exploit; `RESPAWN_FRAMES = 180` is defensible for
a kill race though it would want re-tuning under fix (b); the anchor
system's `AutoMin` re-derivation (`anchor.rs:92`) is correct and its
"keep last good rect" guard handles the un-populated-window case cleanly.
