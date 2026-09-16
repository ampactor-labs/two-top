# Persona 5 — Tobi, who plays with their thumbs
### Input & game-feel audit — READ-ONLY, code-traced

> I can't tell you what's wrong with it. It plays fine. But my right thumb
> lives in the bottom-right corner and that's the dash button, so every
> throw starts with a reach. And I taunt by accident about once a match,
> which in a one-hit-kill game is a death.

**Scope note.** This section was written inline rather than by a dedicated
agent (the parallel run that produced P1–P4 and P6 was cut off by an API
limit before P5 and P7 were written). It is consequently **narrower** than
those five: it covers the touch geometry, the zone state machine, and the
latency budget, and it does not attempt the sweep of the render/feel layer
(screen shake, kill flash, kill-cam, audio) that the brief asked for. What
is here is traced and verified; what is missing is named at the end.

---

## Findings

| # | Sev | Finding | Evidence |
|---|-----|---------|----------|
| 1 | 🟡 | The DASH corner occupies the right thumb's rest position, so every throw begins with a reach | `input_touch/src/lib.rs:253`,`:256`,`:262`,`:268` |
| 2 | 🟡 | The TAUNT strip is 24% of the screen, full width, and a stray press there roots you for 0.7 s fully vulnerable | `input_touch/src/lib.rs:219`,`:238`; `sim/src/lib.rs:175`,`:1362` |
| 3 | 🟡 | The taunt's only payoff is a ladder the round boundary deletes — compounds `GAME_DESIGN_AUDIT` #1 | `sim/src/lib.rs:159-170`,`:1873`,`:1490` |

---

## 1. 🟡 The dash corner owns the right thumb's rest position

The dash button is the one fixed control on the screen, and the 2026-07-16
pass deliberately doubled its area:

```rust
// crates/input_touch/src/lib.rs:249-256
/// Fraction of the window width where the DASH corner begins. The corner
/// was 0.78/0.86 originally; the 2026-07-16 pass doubled its AREA (each
/// span x sqrt(2)) -- dash is the one control that must never be missed blind.
pub const DASH_ZONE_X_FRAC: f32 = 0.69;
pub const DASH_ZONE_Y_FRAC: f32 = 0.80;
```

That is **31% of the width by 20% of the height** of the bottom-right
corner. And the throw zone is defined as the right half *minus* it:

```rust
// crates/input_touch/src/lib.rs:268-274
pub fn is_throw_zone(pos: Vec2, window: Vec2) -> bool {
    window.x > 0.0
        && pos.x >= window.x * 0.5
        && !is_dash_zone(pos, window)
        && !is_taunt_zone(pos, window)
        && !is_quit_zone(pos, window)
}
```

Zones are latched at press — `zone_probe` reads `t.start_pos`
(`:279-285`) — so the throw stick must *begin* outside the dash corner.
On a phone held in two hands, the right thumb's neutral arc ends in the
bottom-right corner. So the floating throw stick, which is advertised as
landing "wherever the thumb lands" (`:265-267`), in practice cannot land
where the thumb rests: every charge begins with a deliberate reach up or
inward, and a hurried throw started too low is a dash instead.

This is a **stated tradeoff, not an oversight** — the comment says dash
must never be missed blind, and that is a defensible call in a game where
dash is the only defensive option. It is filed 🟡 because the cost is
real, undocumented, and lands on the control the player uses most. It is
also the kind of thing that only a device test can settle, and the
device test has not been run.

**Fix (if it is a problem at all):** it is measurable before it is
fixable. Log the distribution of throw-stick start positions from real
sessions; if the mass is piled against the dash corner's boundary, shrink
the corner's Y span and give dash a visible resting ring the thumb can
find by feel instead of by area.

---

## 2. 🟡 A quarter of the screen roots you for 0.7 seconds

```rust
// crates/input_touch/src/lib.rs:215-219
/// Fraction of the window height (from the top) that is the TAUNT strip.
/// Both thumbs live at the bottom, so a top-of-screen tap is always a
/// deliberate reach -- exactly the ergonomics a taunt deserves.
pub const TAUNT_ZONE_Y_FRAC: f32 = 0.24;
```

and what a taunt costs:

```rust
// crates/sim/src/lib.rs:159-175
/// TAUNT frames remaining (0 = not taunting). A taunt is a rooted,
/// public flex started on a fresh TAUNT press edge: the demon plants
/// for [`TAUNT_FRAMES`] ticks, fully vulnerable ...
pub const TAUNT_FRAMES: u32 = 42;
```

42 frames is 0.7 s of being rooted and fully vulnerable, in a game where
every hit is lethal. The trigger is a press anywhere in the top 24% of
the screen minus the QUIT corner (`is_taunt_zone`, `:238-241`).

**Two things make this less bad than it first looks, and both check out.**
Zones latch on `start_pos`, so dragging an aim stick up into the strip
does *not* taunt (`zone_probe`, `:279`). And `start_taunt` is gated on
being alive, in-round, not dashing, and with no charge armed
(`sim/src/lib.rs:1343-1364`), so it cannot interrupt a committed action.

What remains is the bare case: a press that *starts* in the top quarter
while alive and idle. The comment's premise — "a top-of-screen tap is
always a deliberate reach" — holds for a two-thumbed grip and fails for
the one-handed grip, the re-grip after a phone shifts, and the tap aimed
at anything the app draws up there.

**Fix:** require a deliberate gesture rather than a large area — a
double-tap, or a press held past a few frames — or shrink the strip and
draw it, so the reach is visibly a button rather than a quarter of the
screen that happens to be armed.

---

## 3. 🟡 The taunt's payoff is deleted by a timer that does nothing else

This is not a new mechanism; it is `GAME_DESIGN_AUDIT.md` #1 seen from
the thumbs. Worth recording because it changes that finding's blast
radius.

A completed taunt pays out exactly one thing:

```rust
// crates/sim/src/lib.rs:1490-1493
} else if taunt.0 > 0 {
    taunt.0 -= 1;
    if taunt.0 == 0 {
        streak.0 += 1;
    }
}
```

`CatchStreak` is the perfect-catch ladder — the game's signature skill
expression, feeding throw speed and, at `STREAK_LIGHTNING = 3`, full
board reach at any charge (`:552-575`). And `reset_round_state`
(`:1873`) wipes `CatchStreak` at every round boundary, a boundary that
`GAME_DESIGN_AUDIT` #1 proves changes nothing on the scoreboard.

So the taunt asks the player to accept 0.7 s of total vulnerability in
exchange for a rung on a ladder that a scoreless timer deletes. The
existing audit already established that the clock's only gameplay effect
is to punish the player who is playing best; this is the second mechanic
that routes its entire payoff through the state the clock destroys. Any
fix to #1 should be checked against the taunt, not just the catch.

---

## The latency budget, for the record

Not a finding — a number the project does not currently write down
anywhere, assembled from the constants:

| Stage | Cost | Source |
|---|---|---|
| Touch sample → sim tick | ≤ 1 frame (16.7 ms) | `TICK_HZ = 60`, `sim/src/lib.rs:53` |
| Online input delay | 2 frames (33.3 ms) | `ONLINE_INPUT_DELAY`, `netplay.rs:50` |
| Render interpolation | ≤ 1 tick (16.7 ms) | `render/src/lib.rs:6-12`,`:219-238` |
| Panel / touch digitizer | not measurable from the repo | — |

Local practice ≈ **33 ms + panel**; online ≈ **67 ms + panel**. That is a
normal budget for a rollback fighter and the interpolation cost is a
deliberate, documented choice (`MORGAN_NOTES.md`, interpolation over
extrapolation). It is recorded here because P2 #1 recommends raising
`ONLINE_INPUT_DELAY` to 3–4 to buy prediction headroom, and this is the
budget that recommendation spends from.

---

## Checked and found sound

- **Touch coordinates are density-independent.** `WindowSize` is
  populated from `w.width()`/`w.height()` (`app/src/lib.rs:1259-1263`),
  which is logical pixels, and bevy_winit converts touch events from
  `winit::dpi::LogicalPosition` (`bevy_winit-0.18.1/src/converters.rs:52`).
  So `STICK_MAX_RADIUS_PX = 80.0` is 80 logical px ≈ 80 dp ≈ 12.7 mm on
  any density. I went looking for a DPI bug here (there is not a single
  `scale_factor` reference in the workspace) and there isn't one — the
  units line up.
- **Zones latch at press, not at drag.** `zone_probe` reads
  `t.start_pos` (`:279-285`) and all four selectors are sticky on the
  touch id (`:292-364`), so no control can be stolen mid-gesture.
- **Southpaw mirrors every probe uniformly**, including the zones where
  it is a no-op (`:279-285`, `:341-345`) — no asymmetric hole.
- **The forgiveness affordances exist and are deliberate:**
  `DASH_BUFFER_TICKS = 7` (`sim:677`) and
  `THROW_FORGIVENESS_FRAMES = 6` (`sim:622`). Both are the right shape
  for a 60 Hz rollback game.
- **The deadzone is player-adjustable and plumbed live** into
  `input_touch`'s `StickDeadzone` (`settings.rs:3-6`,`:26-27`), with a
  smooth radial curve rather than a hard cutoff (`:196-213`).

## Not covered (the gap this section does not fill)

The brief for this persona also asked for the **game-feel layer** —
screen shake, kill flash, kill-cam, hit-stop, the synthesized audio and
the Android haptics — read against how they land in the hand. None of
that is audited here. `HIT_STOP_FRAMES = 6` (`sim:313`) and the haptics
settings toggle (`settings.rs:35`,`:54`) were the only parts touched, and
only in passing. That work remains open.

**One cross-reference:** P4 #10 establishes that no UI surface in `app`
ever sees a mouse — `input_touch`'s mouse synthesis writes `TouchState`,
not `Touches` (`input_touch/src/lib.rs:431-470`) — which makes every
touch affordance described above dead on a desktop browser. That finding
belongs to P4 and is not repeated here.
