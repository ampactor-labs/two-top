# The practice ladder, measured

The practice bot is tuned against numbers, not against how a match felt.
The numbers come from `bot_bench`, which plays whole matches headless
through the real sim (same plugins, arena geometry, spawns and round flow
as a live match) with each seat driven by the bot brain at a fixed tier,
by an idle seat, or by a *strafer*:

```bash
cargo run -p bot_brain --bin bot_bench --release -- 40   # matches per row
cargo run -p bot_brain --example idle_probe --release     # idle vs tier 0, every arena, both spawns
```

The brain is `crates/bot_brain` (pure policy plus the headless bench); the
app only feeds it the match and the gauntlet tier (`crates/app/src/bot.rs`).
The parts of the ladder that must never regress are CI gates in
`crates/bot_brain/tests/ladder.rs`.

## What the bench can and cannot say

Bot against bot measures the bot, not the bot against a person. Two bots
that both throw at where the other is standing reward tempo over aim; a
person dodges, feints and leads. So the ladder is read three ways, and
only the first two are targets:

1. **Newcomer floor.** An idle player against tier 0 must meet a finished
   match, on every arena, from either spawn — the player learns what a
   fang in flight looks like within the first minute.
2. **Against a moving target.** Each tier against a *strafer* (never
   throws, never stops walking, a fresh heading every 0.5–1.5 s): how long
   it takes to land five. Should fall as the tier rises.
3. **Against itself.** Tier n against tier n+1, and across wider gaps.
   Wide gaps should be decisive; neighbours are close by design (one
   notch of each knob), so a neighbour pair near a coin flip is expected
   and not a defect.

## Targets

| Measure | Target |
|---|---|
| idle vs tier 0 | every match finishes; under 180 s on every arena and spawn |
| strafer vs tier n | time to five falls with the tier |
| tier 1 vs tier 10 | tier 10 wins at least 3 in 4 (CI: 15 of 20) |
| tier 10 vs tier 10 | at most 1 round in 20 ends with no kill |

## Last measured run (40 matches per row, arenas cycled, seats swapped)

| target | bot | bot wins | target wins | unfinished | mean match s | empty rounds |
|---|---|---:|---:|---:|---:|---:|
| idle | tier 0 | 40 | 0 | 0 | 88 | 6/130 |
| strafer | tier 0 | 40 | 0 | 0 | 103 | 22/145 |
| strafer | tier 1 | 40 | 0 | 0 | 49 | 0/79 |
| strafer | tier 2 | 40 | 0 | 0 | 43 | 0/73 |
| strafer | tier 3 | 40 | 0 | 0 | 45 | 0/74 |
| strafer | tier 4 | 40 | 0 | 0 | 44 | 0/73 |
| strafer | tier 5 | 39 | 1 | 0 | 38 | 0/70 |
| strafer | tier 6 | 40 | 0 | 0 | 39 | 0/67 |
| strafer | tier 7 | 40 | 0 | 0 | 38 | 0/64 |
| strafer | tier 8 | 40 | 0 | 0 | 35 | 0/64 |
| strafer | tier 9 | 40 | 0 | 0 | 37 | 0/66 |
| strafer | tier 10 | 40 | 0 | 0 | 34 | 0/58 |

| A | B | A wins | B wins | unfinished | mean match s | kills / round | empty rounds |
|---|---|---:|---:|---:|---:|---:|---:|
| tier 0 | tier 1 | 1 | 39 | 0 | 37 | 4.06 | 0/62 |
| tier 1 | tier 2 | 19 | 21 | 0 | 38 | 4.86 | 0/65 |
| tier 2 | tier 3 | 18 | 22 | 0 | 41 | 4.27 | 0/70 |
| tier 3 | tier 4 | 19 | 21 | 0 | 46 | 4.03 | 0/76 |
| tier 4 | tier 5 | 25 | 15 | 0 | 46 | 3.92 | 0/78 |
| tier 5 | tier 6 | 24 | 16 | 0 | 48 | 3.87 | 0/78 |
| tier 6 | tier 7 | 20 | 20 | 0 | 49 | 3.84 | 0/80 |
| tier 7 | tier 8 | 16 | 24 | 0 | 51 | 3.57 | 0/84 |
| tier 8 | tier 9 | 11 | 29 | 0 | 54 | 3.65 | 0/86 |
| tier 9 | tier 10 | 9 | 31 | 0 | 64 | 3.06 | 0/99 |
| tier 10 | tier 10 | 22 | 18 | 0 | 70 | 3.09 | 2/104 |

Wider gaps (40 matches each): tier 2 vs 5: 14–26 · tier 3 vs 6: 15–25 ·
tier 4 vs 7: 13–27 · tier 5 vs 8: 21–19 · tier 1 vs 10: 1–19 (20 matches).

Two identical tier-10 bots split 22–18, which is the noise floor: about
±4 wins in 40.

### Open

- Tiers 4–6 against their next neighbour lean the wrong way (25–15,
  24–16), and tier 5 vs 8 is level. The heavier commit charge of the
  higher tier costs it tempo bot against bot; against a person the same
  charge is a faster fang that is harder to dodge. Not tuned further
  without a person on the other side — the next signal is play-testing.
- The strafer stands in for a moving person but never dodges and never
  fights back, so its column measures offense only.

## What the bench found (and what changed)

Each of these was a number before it was a fix:

- **Tier 0 never threw.** An idle newcomer looped 30 s rounds at 0-0
  forever. Tier 0 is a sparring partner now: slow, planted, telegraphed
  lobs on a 3 s beat, no dodging, no steered recalls.
- **The chasm.** The brain did not know the Crossing's moat; every tier
  walked into it (an idle player beat tier 0 there 5-0). Lethal ground is
  steered around like cover.
- **Boxed in.** On the Gallery the orbit's pull and the cover slide
  cancelled out inside the fenced centre, and the bot threw into a bar for
  ten minutes. With cover in the way the bot now routes (a breadth-first
  search over a body-padded grid) to the nearest spot with a clear line,
  and the sparring partner holds its throw until it gets there.
- **A frozen wobble.** For large seeds `seed as f32` swallowed the frame
  term, so the aim wobble was one fixed offset for the whole match. The
  live game passes seed 0 and was not affected; the bench was.
- **Throwing the fang away.** A dodge while charging released THROW with
  the stick pointed sideways. It now lets the fang go at the foe and
  buffers the dash (the sim fires it the tick after the release).
- **Aim at the wrong spot.** The bot threw at where its foe was. A fang
  takes a quarter second to cross the dueling range, and anyone walking
  has moved a body width by then, so a sharp tier's tight aim missed as
  often as a green tier's spray. Throws now lead the target by a share of
  its motion that grows with the tier (none at tier 1, all of it from 8).
- **Dodging back into the fang.** After a dodge, the reflex picked the
  side nearer the centre on every tick, which was back across the lane,
  and the bot walked into the fang it had just dodged (most of tier 10's
  deaths). It now dodges out on the side it is already on, skips fangs
  that will pass wide, and its footwork stays out of an incoming fang's
  lane until the fang has passed.

Before these, tier 1 beat tier 10 27–13; after, tier 10 wins 19–1.

## The knobs

All in `crates/bot_brain/src/lib.rs`, one notch per tier, each saturating
by difficulty 11 (tier 10 plus the in-match rubber band's notch):

| Knob | Tier 1 | Tier 10 | Function |
|---|---|---|---|
| commit charge | 30 % | 71 % | `throw_at_charge` |
| aim wobble | 0.45 rad | 0.135 rad | `wobble_amp` |
| lead | none | full | `lead_frac` |
| perception lag | 14 ticks | 5 ticks | `perception_lag` |
| dodge bubble | — (from 3: 100 cm) | 240 cm | `threat_radius` |
| reads a fang | — (from 3: 25 %) | 74 % of windows | `dodge_chance` |

In a match, the tier moves one notch toward whoever is behind by two kills
or more (`rubber_band`), never below the sparring partner.
