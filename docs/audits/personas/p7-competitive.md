# Persona 7 — Vic, who reads the rulebook before the tutorial
### Competitive-integrity audit of the sim — READ-ONLY, code-traced

> Show me a one-hit-kill game and I'll show you where the invulnerability
> frames leak. Every fighting game has one move that's free. I found this
> one in about twenty minutes: you get a free taunt every time you die,
> and the taunt is worth a third of the game's best reward.

**Scope note.** Written inline rather than by a dedicated agent (the
parallel run that produced P1–P4 and P6 was cut off by an API limit
before P5 and P7 were written). It is **narrower** than those five: it
covers the spawn-guard/taunt/streak interaction and the sudden-death
geometry, and does not sweep the pickups, the per-arena rule tags, the
boomerang ricochet rules, or the six modifier behaviors. What is here is
traced; what is missing is named at the end. `docs/GAME_DESIGN_AUDIT.md`
was read first and its six findings are not repeated — in particular the
round-scoring finding (#1) is treated as established, not re-derived.

---

## Findings

| # | Sev | Finding | Evidence |
|---|-----|---------|----------|
| 1 | 🟠 | Every death buys a free, unpunishable taunt — a permanent one-tier discount on the streak ladder, and it breaks both docs' stated contracts | `sim/src/lib.rs:1481-1496`, `:175`, `:191`, `:1343-1364` |
| 2 | 🟡 | The respawn points sit on the exact boundary of the crumble's final safe floor — zero margin, hazard not yet a bug | `sim/src/lib.rs:2907-2912`, `:598`, `:2856-2863` |

---

## 1. 🟠 Every death buys a free taunt, and the taunt is worth a third of the ladder

Two components govern a revived player. Each has a doc comment stating a
contract. **The two contracts contradict each other**, and the code
implements neither.

`SpawnGuard` promises the guard can never be offensive:

```rust
// crates/sim/src/lib.rs:178-186
/// points are fixed per handle and kills are one-hit, so without this a
/// killer camps the spawn with a charged fang and the round snowballs.
/// While > 0 the fang/dash/fire/pyre kill systems skip the player; the
/// chasm and OOB stay lethal (walking off the world is a choice, not a
/// camp). The guard BREAKS the moment the revived player acts -- holding
/// THROW or committing a dash -- so it can never be an offensive shield.
```

`Taunt` promises the flex is always punishable:

```rust
// crates/sim/src/lib.rs:159-170
/// ... the demon plants for [`TAUNT_FRAMES`] ticks, fully vulnerable, and
/// if the flex completes uninterrupted the perfect-catch STREAK climbs one
/// tier ... Disrespect as strategy: the reward is real, the window to
/// punish it is public and generous.
```

Now the code that has to honour both:

```rust
// crates/sim/src/lib.rs:1481-1496
let threw = just_pressed(curr, prev, PlayerInput::THROW_DOWN);
if guard.0 > 0 {
    if dashing || charge.0 > 0 || threw {
        guard.0 = 0;
    } else {
        guard.0 -= 1;
    }
}
if dead.is_dying() || dashing || charge.0 > 0 {
    taunt.0 = 0;
} else if taunt.0 > 0 {
    taunt.0 -= 1;
    if taunt.0 == 0 {
        streak.0 += 1;
    }
}
```

**Taunting is not in the guard's break list.** Dash breaks it, a charge
breaks it, a THROW press breaks it. A taunt does not. And `start_taunt`
(`:1343-1364`) gates on alive / in-round / not-dashing / no-charge —
it never consults `SpawnGuard` either.

Then the arithmetic, which is not close:

```rust
pub const TAUNT_FRAMES: u32 = 42;        // :175
pub const SPAWN_GUARD_FRAMES: u32 = 45;  // :191
```

Walk a respawn. `tick_respawn` (`:2941-2975`) revives the player with
`guard.0 = 45` and `streak.0 = 0`. On the next tick the player presses
TAUNT; `start_taunt` sets `taunt.0 = 42`. `tick_taunt_and_guard` then
decrements both every tick — the guard because taunting is not an "act",
the taunt because nothing cancels it. The taunt reaches 0 after 42 ticks,
**with 3 frames of invulnerability still on the clock**, and pays out
`streak.0 += 1`.

For the whole 42 frames the fang, dash, fire and pyre kill systems skip
this player by `SpawnGuard`'s own design. The Taunt doc's "window to
punish it is public and generous" is, on a respawn, **zero frames wide**.

**What the free tier is actually worth.** The streak is not linear:

```rust
// crates/sim/src/lib.rs:566-576
/// Launch-speed multiplier from the perfect-catch streak: 1.0 / 1.12 / 1.30
/// at streak 0-1 / 2 / 3+.
pub fn streak_speed_factor(streak: u32) -> Fix {
    match streak { 0 | 1 => Fix::const_from_int(1), 2 => Fix::lit("1.12"), _ => Fix::lit("1.30") }
}
```

Streak 1 on its own buys nothing — same multiplier as 0, no lightning.
The value is positional: a player who taunts on every respawn reaches
`STREAK_LIGHTNING = 3` (full `REACH_MAX_CM` reach at any charge, plus
the 1.30× speed) on **two** perfect catches instead of three. That is a
33% discount on the game's signature skill expression, granted for a
single tap, with no counterplay, refreshed by every death.

Note what this does to the game's incentive gradient. Dying already
costs you a kill on a cumulative-kill scoreboard (`GAME_DESIGN_AUDIT`
#1), so this does not make dying *good*. It makes dying **cheaper than
designed**, and it rewards the one player who reads the frame data.

**The author saw the adjacency and half-documented it.** `TAUNT_FRAMES`'
own comment reads: *"short enough to sneak one in during the respawn
beat you just earned"* (`:172-175`). Sneaking one in during the respawn
beat is clearly intended. Sneaking one in *inside invulnerability*,
where the stated cost of the move does not exist, reads as the
unintended half — and 42 < 45 by exactly three frames is a very tight
coincidence for a number that was supposed to be punishable.

**Fix (one line, and a choice).** The one line is adding the taunt to the
guard's break list:

```rust
if dashing || charge.0 > 0 || threw || taunt.0 > 0 { guard.0 = 0; }
```

which makes taunting an "act" like every other, exactly as `SpawnGuard`'s
doc already claims. The choice is whether you *want* the respawn taunt as
a designed beat; if so, keep it and delete the streak payout for a taunt
started under guard, so the flex stays expressive but stops being
economic. Either way, one of the two doc comments needs correcting — they
cannot both be true.

---

## 2. 🟡 The respawn points sit exactly on the crumble's final safe boundary

Respawn points are fixed:

```rust
// crates/sim/src/lib.rs:2907-2912
pub fn respawn_position(handle: usize) -> Vec2F {
    match handle {
        0 => Vec2F::from_cm(0, -300),
        _ => Vec2F::from_cm(0, 300),
    }
}
```

The sudden-death crumble shrinks the safe floor to
`SUDDEN_DEATH_MIN_FACTOR = 0.4` of the arena at the buzzer (`:598`),
against `ARENA_HALF_HEIGHT_CM = 750` (`:731`). So at the floor's
smallest, the safe half-height is `750 × 0.4 = 300` cm — and the respawn
points are at `|y| = 300` cm. **Exactly on the line, with zero margin.**

And `SpawnGuard` explicitly does not help here: *"the chasm and OOB stay
lethal (walking off the world is a choice, not a camp)"* (`:180-181`).
The grace is also tightened during the crumble —
`SUDDEN_DEATH_OOB_GRACE_FRAMES = 45` against the open-play
`OOB_GRACE_FRAMES = 180` (`:602`,`:737`).

**I tried to make this a live bug and it does not quite land.** The OOB
test is on the player's centre with a strict comparison:

```rust
// crates/sim/src/lib.rs:2856-2863
let half_h = Fix::const_from_int(ARENA_HALF_HEIGHT_CM) * factor;
...
let out_of_bounds = pos.0.x.abs() > half_w || pos.0.y.abs() > half_h;
```

In I16F16, `Fix::lit("0.4")` is `26214/65536 = 0.399993896…`, so
`750 × factor` at the buzzer is `299.99542…`, which is *less* than 300 —
i.e. the respawn point is out of bounds by ~0.005 cm. But solving
`750 × (0.4 + 0.6t) > 300` for the frames either side shows this holds
only at `remaining == 0`, and `oob_death` early-returns unless
`match_state.is_in_round()` (`:2827-2829`) while `tick_match_state`
flips the round at that same expiry. So the one frame where it bites is
not reachable. **Not a bug today.**

It is filed 🟡 as a hazard because nothing protects it. There is no test
pinning `sudden_death_factor` (a workspace grep finds no callers outside
`oob_death`), nothing asserts that `respawn_position` lies inside
`ARENA_HALF_* × SUDDEN_DEATH_MIN_FACTOR`, and the margin is currently
**negative by five thousandths of a centimetre**, saved only by a
scheduling coincidence. Any retune of `SUDDEN_DEATH_MIN_FACTOR` downward,
of the respawn points outward, or of the round-expiry ordering turns a
late-round death into a respawn straight into the void on a 45-frame
clock — which, per `GAME_DESIGN_AUDIT` #1, is a round whose stakes are
imaginary anyway.

**Fix:** a two-line test — assert
`respawn_position(h).y.abs() < ARENA_HALF_HEIGHT_CM × SUDDEN_DEATH_MIN_FACTOR`
for both handles — and pull the respawn points inward (±240 cm gives a
20% margin) so the invariant holds with room rather than by luck.

---

## Checked and found sound

Things I went after and could not turn into findings:

- **`SpawnGuard` is not an offensive shield for anything except the
  taunt.** Dash, charge and throw all break it on the tick they start
  (`:1481-1486`), and the guard leaves the chasm and the void lethal, so
  the classic "invulnerable body-block" and "throw from spawn" lines are
  both closed.
- **Death forfeits everything it should.** `tick_respawn` (`:2963-2972`)
  zeroes the stolen second boomerang (`cap.0 = 1`), the pending charge,
  the catch streak and any taunt, and clears `DashState`/`StunFrames`/
  `VelocityF` — you cannot bank state through a death.
- **Owner immunity and stun immunity are both explicit and correctly
  ordered.** A fang cannot kill its thrower, `StunFrames > 0` blocks the
  hit, and `hit_boomerang_player` runs *before* `catch_boomerangs` so a
  simultaneous hit-and-catch resolves as a kill rather than a free catch
  (`:1498-1514`).
- **Running out of bounds is not free.** `oob_death` credits the
  opponent with the kill and snaps the corpse back to the spawn point
  (`:2874-2880`), so there is no "deny the kill by suiciding" line.
- **The taunt cannot be used to interrupt a committed action**, in either
  direction: `start_taunt` refuses while dashing or charging, and
  `tick_taunt_and_guard` cancels an in-flight taunt the moment a dash or
  charge begins, with no streak payout (`:1343-1364`, `:1489-1496`).
  The same-tick DASH+TAUNT race is resolved deliberately in the dash's
  favour and the ordering is documented (`:1334-1342`).

## Not covered (the gap this section does not fill)

The brief for this persona also asked for the pickups and the six
modifier behaviors, the per-arena rule tags across all seven tables, the
ricochet/`MAX_FREE_WALL_BOUNCES` rules, and the empowered/perfect-catch
economy read as a whole. None of that is audited here. A rules-lawyer
sweep of the pickup modifiers in particular is the highest-value
remaining piece — six behaviors that alter a one-hit-kill sim is exactly
where the next free move will be.
