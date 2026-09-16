# Persona 3 — "Sam, playing alone at 1am"

## Solo + metagame audit, traced through the code

---

### In persona

> Monday, 1am. Nobody's up. I tap PRACTICE. Something ambles around the
> table in a slow figure-eight and lets me kill it five times. Tier 1.
> Tuesday I get to GAUNTLET 6 and it's actually sharp — it dashes through
> everything I throw. Wednesday I lose, and I'm back to the amiable
> figure-eight for a full five kills before it wakes up. Thursday I work
> out that if I'm 1-4 down I can just tap the top strip and walk out, and
> the tier doesn't move. So now I never lose. GAUNTLET 14 by Friday. It
> plays exactly the same as GAUNTLET 10 did.
>
> I go to REPLAYS to rewatch the good match against SUDS from last week.
> The list is eight rows and all eight say "SAM WINS — BOT". Every
> gauntlet run I did this week pushed my actual matches off the screen.
> There's no scroll and no delete. There's a folder path at the bottom of
> the screen, which is the game telling me to go use a file manager.
>
> I find SUDS in RIVALS and tap SPAR THEIR SHADE. Nothing happens. No
> message. I tap it again. Nothing. I tap a ROLL row. Nothing.

Every one of those beats is in the code below.

---

## Findings

| # | Finding | Severity | Evidence |
|---|---------|----------|----------|
| 1 | A corrupt `career.json` silently erases the tier, every rivalry and every tape ring — then overwrites the evidence | 🔴 data loss | `grudge.rs:231-239`, `:241-253` vs `profile.rs:307-316` |
| 2 | Scrubbing a tape with `frame_count <= 1` panics and kills the app | 🔴 crash | `theater.rs:858`; `replay/tests/codec.rs:96-104`; `web.rs:78-85` |
| 3 | The gauntlet has no downward pressure: quitting a practice match keeps the tier | 🟠 progression | `screen.rs:1613-1636`, `grudge.rs:342-377` |
| 4 | The tier counter climbs forever; the bot stops changing at difficulty 10 | 🟠 progression | `bot.rs:117-152`, `:682-692`, `screen.rs:1131`, `intro_card.rs:34` |
| 5 | REPLAYS fully reads + fully decodes every tape on disk to draw 8 rows; nothing ever deletes one | 🟠 O(n)/growth | `theater.rs:228-277`, `replay/src/lib.rs:85-96` |
| 6 | Hard 8-row unscrollable ceilings over unbounded stores; gauntlet tapes evict Sam's real matches | 🟠 bad | `theater.rs:42`, `rivals.rs:28`, `recorder.rs:150-176` |
| 7 | `SHADE_MIN_TAPES` is checked against filenames, not measurements — a "3-tape habit" can be one tape | 🟠 bad | `rivals.rs:37-38`, `:457`, `:466-482` |
| 8 | After a `SIM_VERSION` bump, RIVALS still offers tapes and shades that silently do nothing | 🟠 bad | `rivals.rs:426-443`, `:449-500` vs `theater.rs:540-568` |
| 9 | The bot's dodge is frame-perfect with zero reaction latency — the doc claims an imperfection that isn't there | 🟡 fairness | `bot.rs:11-16` vs `:209-231`; `sim:447`, `sim:667` |
| 10 | A loss at tier 8 sends Sam through a full 5-kill match against a bot that cannot fight back | 🟡 friction | `grudge.rs:374`, `bot.rs:200-202`, `:446` |

---

## 1. 🔴 A corrupt `career.json` silently erases everything, then overwrites the evidence

`career.json` is the *only* file that holds Sam's solo progression: the
gauntlet tier, the best-ever tier, every rivalry row, every streak, every
`last_met`, and every rival tape ring. It is loaded like this:

```rust
// grudge.rs:231-239
fn load_career() -> CareerRecord {
    let Some(path) = career_path() else {
        return CareerRecord::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}
```

A parse failure is indistinguishable from a first boot. No log line, no
quarantine, no notice. Then the *first decided match after that boot*
calls `save_career` (`grudge.rs:326`, `:376`), which writes the empty
default over the file:

```rust
// grudge.rs:241-253
fn save_career(record: &CareerRecord) { ... crate::paths::write_atomic(&path, json.as_bytes()) ... }
```

The bytes that would have told a human what happened are gone with them.

**This is the asymmetry that matters**, and the codebase already knows the
right answer — `profile.json`, which holds strictly *less*, gets the
careful treatment:

```rust
// profile.rs:307-316
Err(e) => {
    tracing::error!(target: "two_top::profile", error = %e,
        "profile.json is corrupt — quarantining it and reminting");
    let mut name = path.file_name()... ;
    name.push(".corrupt");
    let _ = std::fs::rename(path, path.with_file_name(name));
    LocalProfile::default()
}
```

(Note for the brief's hypothesis: `profile.rs:315`'s bare `fs::rename` is
**not** an atomic-write bypass — it is the quarantine move, and the actual
save at `profile.rs:329` uses `write_atomic` correctly. I checked all
writers in the workspace; every non-test one routes through
`paths::write_atomic`. The bypass isn't on the write side. It's that the
*read* side of `career.json` has no quarantine at all.)

Two related silent-loss paths on the same file:

* `career_path()` returning `None` (`grudge.rs:227-229`, via
  `paths::config_file`) makes both load and save no-ops with no signal.
  `paths.rs:1-12` documents this as the exact historical Android bug; there
  is still no UI that can tell Sam his progress isn't being kept.
* A full disk makes `write_atomic` fail, which logs `warn!` and returns
  (`grudge.rs:249-252`). Sam climbs four tiers, relaunches, and is back
  where he started. Nothing on screen ever says why.

**Fix:** mirror `read_profile` — quarantine `career.json` as
`career.json.corrupt`, log at `error!`, and surface a one-line notice on
the Title. Propagate the `save_career` result far enough that a persistent
failure can raise the same notice.

---

## 2. 🔴 Scrubbing a short tape panics and kills the app

```rust
// theater.rs:858
let target = ((fx * total as f32) as u32).clamp(1, total.saturating_sub(1));
```

`total` is `TheaterMode::total_frames`, set verbatim from the tape header
(`theater.rs:635`: `theater.total_frames = header.frame_count;`). When
`frame_count` is 0 or 1, this is `clamp(1, 0)`, and `Ord::clamp` asserts
`min <= max` in **release** builds. Verified by compiling the exact
expression with `-O`:

```
thread 'main' panicked at clamp.rs:4:42:
min > max. min = 1, max = 0
```

The path to it is one finger on the scrub strip.

This is not a hypothetical byte pattern. The codec *guarantees* such a
tape round-trips, and has a test that says so:

```rust
// replay/tests/codec.rs:96-104
fn empty_inputs_roundtrip() {
    let mut replay = sample_replay();
    replay.inputs.clear();
    replay.header.frame_count = 0;
    let bytes = encode(&replay).unwrap();
    assert_eq!(decode(&bytes).unwrap(), replay);
}
```

`decode` checks magic and `format_version` only (`replay/src/lib.rs:85-96`);
nothing anywhere validates `frame_count > 0` or `frame_count ==
inputs.len()`. And the theater *advertises* third-party tapes — the
REPLAYS screen literally prints *"tapes are files - swap them with a
friend, they play here"* (`theater.rs:406`) plus the folder path — while
the web theater accepts a tape fetched from a URL fragment with no
validation beyond `decode_for_sim_version` (`web.rs:78-85`).

Same header field, softer failure: an *inflated* `frame_count` makes the
scrub bar meaningless and strands a forward seek chasing at `SEEK_SPEED =
64.0` toward a frame the tape can never reach — the end-of-tape auto-pause
at `theater.rs:912-917` is gated on `seek_target.is_none()`, so it never
fires during a seek.

**Fix:** validate in `replay::decode` that `header.frame_count as usize ==
inputs.len()`, which closes both cases at the codec boundary. Belt and
braces: guard the scrub band on `total >= 2` and clamp with
`.min(total.saturating_sub(1)).max(1)` rather than `clamp`.

---

## 3. 🟠 The gauntlet's only downward pressure is opt-in

The ladder moves in exactly one place, and only on a completed match:

```rust
// grudge.rs:351-376
let over = matches!(*state, MatchState::MatchOver);
let entered = over && !*prev_over;
...
if score.p0 >= MATCH_WIN_THRESHOLD {
    record.gauntlet_tier += 1;
    record.gauntlet_best = record.gauntlet_best.max(record.gauntlet_tier);
} else {
    record.gauntlet_tier = 0;
}
```

The in-match QUIT path files a loss *only* when the match is online:

```rust
// screen.rs:1613-1636
let online = world.resource::<NetplayConfig>().room_url.is_some()
    && !world.resource::<crate::bot::PracticeMode>().0;
if online {
    ... crate::grudge::record_abandoned_loss(&mut record, peer);
    crate::netplay::leave_online_match(world);
}
world.resource_mut::<NextState<AppScreen>>().set(AppScreen::Title);
```

In practice mode `online` is false, so a quit does nothing but change
screens. `MatchOver` is never entered, `record_gauntlet_result` never
fires, the tier is untouched.

So: Sam at 1-4 down taps the top-exit strip twice and keeps his tier. The
reset branch at `grudge.rs:374` is reachable only by a player who chooses
to sit through a loss. The ladder is a monotonic counter with an
optional penalty.

This is doubly odd because the codebase is *emphatic* about this exact
honesty elsewhere — `record_abandoned_loss`'s own doc comment
(`grudge.rs:188-192`) says *"Quitting a live online duel is a loss,
recorded on the spot."* The gauntlet gets no such rule.

**Fix:** treat a quit out of a practice match with a non-zero tier the same
way — either reset the tier, or (kinder) don't count it but require the
match be *entered* before the tier can climb again, so bailing costs a
rung's worth of time rather than nothing.

---

## 4. 🟠 The tier climbs forever; the opponent stops at 10

All three difficulty knobs saturate, and the repo has a test that pins it:

```rust
// bot.rs:117-152
fn throw_at_charge(lvl: u32) -> u32 {
    let frac = match lvl {
        0 | 1 => 0.30, 2 => 0.42, 3 => 0.55, 4 => 0.70,
        n => (0.70 + 0.03 * (n - 4) as f32).min(0.85),   // caps at lvl 9
    }; ...
}
fn threat_radius(lvl: u32) -> f32 {
    match lvl { 0 | 1 => 0.0, 2 => 120.0, 3 => 165.0, 4 => 210.0,
        n => (210.0 + 15.0 * (n - 4) as f32).min(300.0) }  // caps at lvl 10
}
fn wobble_amp(lvl: u32) -> f32 {
    match lvl { 0 | 1 => 0.42, 2 => 0.30, 3 => 0.20, 4 => 0.12,
        n => (0.12 - 0.015 * (n - 4) as f32).max(0.05) }   // caps at lvl 9
}
```

```rust
// bot.rs:688-690
assert_eq!(throw_at_charge(40), throw_at_charge(12));
assert_eq!(threat_radius(40), 300.0);
assert_eq!(wobble_amp(40), 0.05);
```

The effective level is `difficulty = tier + kills/2` (`bot.rs:446`), so
with the in-match ramp the last rung that changes anything is **tier 8**.
Tier 9 onward is one fixed opponent.

Meanwhile the number Sam sees has no ceiling at all:

```rust
// screen.rs:1131-1132
if career.gauntlet_tier > 0 { format!("GAUNTLET {}", career.gauntlet_tier) }
// intro_card.rs:34-35
let stakes = if gauntlet_tier > 0 { format!("GAUNTLET TIER {gauntlet_tier}") }
```

For a player who logs 90 minutes a week over a month, this is the whole
experience of the mode: a counter that keeps promising and a duelist that
stopped changing on Wednesday. The saturation itself is right (the comment
at `bot.rs:145-147` is correct that a 0.05-rad floor keeps it beatable
forever) — it's the *unbounded display over a bounded model* that lies.

**Fix:** cap the tier at the last rung that moves a knob (9 or 10) and name
it — "GAUNTLET — MASTERED", or start tracking something else past the cap
(fastest clear, kills-taken). A number that keeps going up while nothing
changes is worse than no number.

---

## 5. 🟠 REPLAYS reads and fully decodes every tape on disk to draw 8 rows

```rust
// theater.rs:236-274
let mut tapes: Vec<(u64, TapeEntry)> = entries
    .filter_map(|e| {
        let path = e.ok()?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("bmrg") { return None; }
        let bytes = std::fs::read(&path).ok()?;
        let replay = replay::decode(&bytes).ok()?;
        let h = &replay.header;
        ...
    })
    .collect();
tapes.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
tapes.truncate(LIST_MAX);
```

Every `.bmrg` in the directory is read in full and deserialized in full —
header *and* the entire `Vec<FrameInputs>` — and then 492 of 500 are
thrown away. The codec offers no header-only path:
`replay::decode` (`replay/src/lib.rs:85`) is
`postcard::from_bytes::<Replay>` over the whole struct.

Measured tape density from the committed canonical demo: 14,418 bytes for
1,800 frames = 8 bytes/frame, i.e. the minimum 4 bytes × 2 seats. Real
play with deflected sticks and live `aim_angle` pushes fields into
2-byte varints, so 8–16 bytes/frame. A 3-minute match ≈ 85–170 KB.

Sam at ~30 matches/week reaches ~1,500 tapes and ~180 MB inside a year.
Every REPLAYS entry then reads 180 MB off Android external storage and
runs 1,500 full postcard decodes **synchronously on the main thread** —
`enter_replays` is a plain `OnEnter` system (`theater.rs:1060`), not a
task. That is ANR territory, and it is paid to render eight lines of text.

And nothing ever removes a tape. A workspace-wide search for
`remove_file` / `retain` / `prune` over `crates/app/src` finds exactly one
`remove_file`, and it is `write_atomic`'s cleanup of its own `.tmp`
sibling (`paths.rs:74`). There is no cap, no rotation, no eviction, and
no delete affordance on any screen. On a 2 GB-free phone the eventual
failure is `write_atomic` returning `Err`, which logs `warn!`
(`recorder.rs:282-285`) and sets `last_saved.0 = None` — the summary card
quietly stops mentioning a replay and nothing else says a word.

**Fix:** two independent changes, both cheap. (a) Add
`replay::decode_header(&[u8])` that stops after the header and have
`scan_tapes` use it — the list becomes O(n) small reads instead of O(bytes
on disk). (b) Give the recorder a ring: keep the newest N tapes (N ≈ 50),
delete the rest on save, and never delete one a rival ring references
(`RivalRecord::tapes`, `grudge.rs:48`).

---

## 6. 🟠 Hard 8-row ceilings over unbounded stores — and the gauntlet floods the one that matters

Both metagame lists are a fixed eight rows with no scrolling:

```rust
// theater.rs:42   const LIST_MAX: usize = 8;
// rivals.rs:26-28  /// Most rivals listed; the ledger keeps everyone, the screen keeps a
//                  /// thumb's worth (the REPLAYS ceiling, same reasoning).
//                  const LIST_MAX: usize = 8;
```

The stores behind them are unbounded. `CareerRecord::rivals` is a
`BTreeMap` (`grudge.rs:129`) that only ever gains entries
(`grudge.rs:196`, `:209`, `:313`, `:332`) and is never pruned; the replays
directory likewise (finding 5). So:

* **Rival #9 is unreachable forever.** `ranked()` sorts by meetings then
  recency and truncates (`rivals.rs:61-67`). A rival outside the top 8 has
  no detail view, so their tape ring cannot be played and their shade
  cannot be summoned. Sam's one-off opponents are stored, counted, and
  invisible. (`ranked()` also deep-clones every `RivalRecord` — name plus
  up to 4 tape filenames — *before* truncating, and `build_rivals_ui`
  clones the entire `CareerRecord` at `rivals.rs:143`. At 500 rivals
  that's a few thousand allocations per screen build. Not per-frame, so
  it's a smell rather than a stall.)

* **Practice tapes evict Sam's real matches.** The recorder has no
  practice guard — `save_replay_on_match_over` (`recorder.rs:150-176`)
  gates only on `MatchOver`, `rec.saved` and `rec.frames.is_empty()`, and
  `header_names` explicitly names the bot seat (`recorder.rs:128-134`:
  `return [Some(profile.name_string()), Some(far)];`). So every gauntlet
  run writes a tape, the list is newest-first, and after eight gauntlet
  matches *every online match Sam has ever played is off the screen.* For
  a persona whose entire mode is the bot ladder, the replay theater
  becomes a list of his own practice sessions within one night.

The escape hatch the screen offers is a printed filesystem path
(`theater.rs:418-427`) — i.e. "go use a file manager."

**Fix:** the sharp, cheap version is a filter toggle on REPLAYS (DUELS /
PRACTICE / ALL) plus a per-row long-press delete; the complete version is
a scrolling list. For RIVALS, either make the list scroll or add a search,
so a rivalry can't become permanently unreachable by being old.

---

## 7. 🟠 The shade's 3-tape minimum is enforced on filenames, not on measurements

The constant states the intent plainly:

```rust
// rivals.rs:36-38
/// Ring tapes needed before a shade can be fitted — fewer reads one
/// match's mood, not a habit.
const SHADE_MIN_TAPES: usize = 3;
```

But the check runs against the ledger's *filename list*, before anything
is read:

```rust
// rivals.rs:457
if tapes.len() < SHADE_MIN_TAPES { return; }
```

and then each tape can drop out for three independent reasons, with only
an `is_empty` floor at the end:

```rust
// rivals.rs:466-482
for tape in &tapes {
    let Ok(bytes) = std::fs::read(dir.join(tape)) else { continue; };
    let Ok(replay) = replay::decode_for_sim_version(&bytes, sim::SIM_VERSION) else { continue; };
    let Some(handle) = crate::shade::rival_handle(&replay, &rival_name, &my_name) else { continue; };
    stats.push(crate::shade::extract(&replay.inputs, handle));
}
if stats.is_empty() { ...warn...; return; }
let style = crate::shade::fit(&stats);
```

So **one** usable tape out of three produces a shade, and `fit` happily
averages a one-element slice (`shade.rs:97-98`: `let n =
stats.len().max(1) as f32;`). The exact thing the constant exists to
prevent — "one match's mood" — is what ships, and nothing in the UI
distinguishes it from a three-tape fit.

The drop-out cases are ordinary, not exotic: a tape deleted by hand from
the advertised folder; a tape from before the last `SIM_VERSION` bump
(finding 8); and a renamed rival — `rival_handle` (`shade.rs:84-91`) needs
either their *current* ledger name or *my current* name on the header, and
if both people have redialed their names since, every old tape measures
nobody.

**Two adjacent honesty gaps on the same feature**, worth stating since the
brief asks whether the fit claims more than it delivers:

* **`extract` never reads the stick.** It touches `cur.buttons` only
  (`shade.rs:46-59`); `stick_x`, `stick_y` and `aim_angle` are never
  looked at. A rival who only ever walked left and a rival who never moved
  fit to *identical* shades, because movement is not a measured axis at
  all. `BotStyle::range` is not measured either — it is *inferred* from
  throw rate (`shade.rs:107`: `range: (520.0 - (throws / 14.0)...)`),
  which `bot.rs:56-58` then documents as "their tempo: pressers close in."
  That's a guess wearing a measurement's clothes.
* **Recall taps are counted as throws.** `sim/src/lib.rs:1452` is explicit
  that a fresh THROW press "is also the recall trigger." `extract` counts
  every `THROW_DOWN` rising edge as a throw (`shade.rs:48-50`) and divides
  total held frames by that count (`:66-69`). A player who charges for 30
  frames and then taps recall for 2 measures as a mean hold of 16, so
  `commit_frac` lands near 0.47 instead of 0.88. Every shade is
  systematically fitted as a shallower charger than its rival.

**Fix:** move the minimum past the reads — `if stats.len() <
SHADE_MIN_TAPES { refuse and say so }`. Exclude short holds (< ~6 frames)
from the `mean_hold_frames` denominator so recalls stop diluting it. And
either measure a movement statistic or stop describing `range` as the
rival's.

---

## 8. 🟠 After a `SIM_VERSION` bump, RIVALS keeps offering things that silently do nothing

The theater handles the no-migrations law beautifully. Foreign-version
tapes are listed, dimmed, labeled with their version, and a tap answers on
screen:

```rust
// theater.rs:245  let foreign_version = (h.sim_version != sim::SIM_VERSION).then_some(h.sim_version);
// theater.rs:379-383  color = BONE.with_alpha(0.35) for foreign tapes
// theater.rs:540-552
if let Some(v) = entry.foreign_version {
    raise_tape_notice(world, format!(
        "that tape speaks SIM v{v} - this build speaks v{}\nold tapes play on the build that recorded them", ...));
    return;
}
```

The comment at `theater.rs:159-162` even states the principle: *"the
no-migrations law should read as a law, not as data loss."*

RIVALS got none of it. Both of its tape paths are log-only:

```rust
// rivals.rs:434-442
match replay::decode_for_sim_version(&bytes, sim::SIM_VERSION) {
    Ok(replay) => { ... crate::theater::start_playback(world, replay); }
    Err(e) => { tracing::warn!(target: "two_top::rivals", error = %e, "rivalry tape rejected"); }
}
```

```rust
// rivals.rs:479-482
if stats.is_empty() {
    tracing::warn!(target: "two_top::rivals", "no readable tape names the rival's seat — no shade");
    return;
}
```

And the offers are drawn from data that knows nothing about versions —
the SPAR THEIR SHADE band appears on `r.tapes.len() >= SHADE_MIN_TAPES`
(`rivals.rs:254`) and the ROLL rows on `r.tapes` (`rivals.rs:274-283`),
both of which are just filenames in `career.json`. So the day after a
version bump, Sam's rivalry detail still shows four ROLL rows and a
glowing SPAR THEIR SHADE, and every one of them is dead on tap with zero
feedback. Same silent failure if a tape was deleted from the folder the
REPLAYS screen told him to go browse.

**Fix:** `raise_tape_notice` already exists and the refusal copy is already
written. Give RIVALS the same two behaviours — stat the ring's headers
when building the detail view, dim unplayable rows, and hide (or gray) the
shade band when fewer than `SHADE_MIN_TAPES` tapes actually decode.

---

## 9. 🟡 The bot's dodge is frame-perfect, and the module doc claims otherwise

The header makes a specific promise:

```rust
// bot.rs:11-16
//! The policy is a readable duelist, not an aimbot: ... Deliberate
//! imperfections (aim wobble, fixed decision cadence) keep it beatable.
```

Aim wobble is real (`bot.rs:305-315`). The "fixed decision cadence" is
not — grepping the file, the only frame-cadence gates are on the *recall*
press (`v.frame % 24 < 2`, `bot.rs:247`), the charge re-press
(`v.frame.is_multiple_of(8)`, `bot.rs:281`) and the orbit swing direction
(`v.frame / 120`, `bot.rs:293`). The survival dodge has none:

```rust
// bot.rs:209-231
if (lvl >= 2 || v.style.is_some())
    && let Some((tpos, tvel)) = v.threat
{
    let to_me = v.me - tpos;
    let radius = v.style.map_or_else(|| threat_radius(lvl), |s| s.dodge_radius);
    if tvel.dot(to_me) > 0.0 && to_me.length() < radius {
        ...
        let buttons = if v.can_dash { PlayerInput::DASH_DOWN } else { 0 };
        return input_from(steer(v, dir), buttons);
    }
}
```

`drive_bot` runs every `ReadInputs` tick and reads the fang's exact
position *and velocity* straight off `VelocityF` (`bot.rs:505-529`), plus
its own exact dash availability (`bot.rs:497`: `can_dash =
matches!(dash, DashState::Idle)`). So it dashes on the precise tick the
fang crosses the radius, every time, with no perception or motor latency.

Quantified: `THROW_SPEED_CM_PER_TICK = 32` (`sim/src/lib.rs:447`) and the
gauntlet's top `threat_radius` is 300 cm, so the bot acts on ~9.4 ticks =
**~156 ms** of warning. At tier 4 (`threat_radius = 210`) it's ~110 ms.
Both are at or under human simple visual reaction time. With
`DASH_DURATION_FRAMES = 5` + `DASH_COOLDOWN_FRAMES = 20`
(`sim/src/lib.rs:666`, `:667`) the reflex re-arms every 25 ticks, and a
single thrower can only have one primary fang out at a time — so at high
tier the bot dodges essentially every direct throw it is not already
mid-dash for. The counterplay collapses to ricochets, steered recalls, and
baiting the dash; the straight throw simply stops being a move.

Mitigating, and worth saying: the trigger is a broad cone (`tvel.dot(to_me)
> 0.0` accepts anything up to 90° off-line), so it burns dashes on near
misses too. And the wobble floor really does keep it beatable. This is a
🟡, not a 🔴 — but the doc should not claim a cadence that isn't in the
code.

**Fix:** either add the latency the comment implies (buffer the threat read
by 6–10 ticks, or gate the dodge branch on a frame cadence) or correct the
comment. The first is ~3 lines and makes the top of the ladder feel like a
player instead of a trap.

---

## 10. 🟡 A loss at tier 8 sends Sam back through a match against a bot that can't fight

```rust
// grudge.rs:373-375
} else {
    record.gauntlet_tier = 0;
}
```

```rust
// bot.rs:446
let difficulty = tier + world.resource::<sim::MatchScore>().p0 as u32 / 2;
```

```rust
// bot.rs:198-202
// Level 0 — a passive sparring dummy: it just ambles around slowly and
// never throws or dodges ...
if lvl == 0 && v.style.is_none() {
    return input_from(wander_dir(v), 0);
}
```

Walk the rung-1 match: at `p0 = 0` and `p0 = 1`, `difficulty` is 0 — a
Lissajous drift (`bot.rs:320-325`) that never throws and never dodges. At
`p0 = 2` it reaches level 1: still `threat_radius(1) == 0.0` (no dodge)
and 0.30-charge lobs. Only at `p0 = 4` does `threat_radius(2) = 120` turn
the dodge on, one kill from the end.

So four of the five kills in the first rung are against something that
cannot meaningfully contest them — and because a loss resets to 0, Sam
replays that match every single time he fails. The tutorial ramp is
correct as a *first* experience; as the standing punishment for losing at
tier 8 it is the opposite of what a ladder is for.

**Fix:** reset to `gauntlet_best.saturating_sub(2)` (or a floor of 2)
rather than 0. The dummy tier is worth exactly one visit.

---

## Checked and sound (tried to break these, couldn't)

* **Bot input is genuinely wire-format.** `bot_decide` returns a real
  `PlayerInput` built through `quantize` → i8 clamp (`bot.rs:155-176`) and
  is inserted into `LocalInputs<GgrsCfg>` at `BOT_HANDLE`
  (`bot.rs:545-547`), so the recorder's `capture_tick_inputs` — which
  harvests from `PlayerInputs` *inside* `GgrsSchedule` (`recorder.rs:74-114`)
  — records practice matches like any other. Gauntlet tapes replay
  correctly. The f32 policy math lives in `app`, never in `sim`, and
  rollback resimulation replays *stored* inputs rather than re-deriving
  them, so it cannot desync. This is right.
* **Shade sparring moves neither ladder, and can't feed on itself.**
  `record_gauntlet_result` bails when a shade is armed (`grudge.rs:357-362`),
  `record_match_result` bails on `practice.0` (`grudge.rs:292`), and
  `note_rival_tape` is only called for a live online duel
  (`recorder.rs:239-244`), so a shade match never lands on the ring that
  fitted it. `disarm_shade` on `OnEnter(Title)` (`bot.rs:548-553`) closes
  the leak. Carefully done.
* **Every real writer uses `paths::write_atomic`**: `recorder.rs:272`,
  `grudge.rs:249`, `profile.rs:329`, `settings.rs:117`, `room_code.rs:241`,
  `attest.rs:232`, `logging.rs:108`. The only non-test `fs::rename` outside
  the helper is the profile quarantine (`profile.rs:315`), which is
  deliberate. `crash.log` overwrites rather than appends, so it can't grow.
* **No panic on corrupt file *content*.** Every load path uses `.ok()` /
  `let Ok(..) else`; `date_label`'s `MONTHS[(m as usize - 1) % 12]`
  (`theater.rs:302`) can't index out of range; `u128::from_str_radix(key,
  16).unwrap_or(0)` (`rivals.rs:99`, `:484`) is guarded. The only panic I
  found on file-derived data is finding 2, and it comes from the *header
  field*, not from parsing.
* **Settings survive anything.** `#[serde(default)]` on the struct
  (`settings.rs:32`) plus `Settings::clamped` with a NaN-safe
  `clamp_finite` (`settings.rs:70-92`) means a hand-edited or partial
  `settings.json` degrades to defaults per-field. `CareerRecord` and
  `RivalRecord` also carry `#[serde(default)]` (`grudge.rs:32`, `:120`)
  with a regression test (`grudge.rs:463-471`), so *field additions* are
  safe — it's only outright corruption that's unhandled (finding 1).
* **Zero rivals is handled.** `rivals.rs:158-166` prints "NO RIVALS YET /
  FIND AN OPPONENT AND MAKE ONE". Zero tapes is handled at
  `theater.rs:341-345`.
* **The scrub debounce is correct** and has a jitter test with an LCG
  (`theater.rs:1140-1161`); the snapshot ring is deliberately never pruned
  and the comment at `theater.rs:924-928` records the bug that taught them.
  `nearest_before`'s reverse scan and the capture-side duplicate check are
  both O(frames/60) on seeks only — not a per-frame cost.

## One per-frame note (not Sam's path)

`update_summary` (`screen.rs:1411`) and `update_intro_card`
(`intro_card.rs:107`) call `career.rivalry_line(peer.0)` **every frame**
while their card is visible, and `rivalry_line` → `display_name` →
`self.rivals.iter().any(|(k, r)| *k != key && r.name == name)`
(`grudge.rs:151`) is a full O(rivals) scan with a string compare per row,
plus several `format!` allocations and a `Text2d` rewrite per frame. At
500 rivals that's 30k string compares a second through the whole ~4 s
MatchOver hold. It's cheap enough not to make the table — and it never
fires for Sam, because `rivalry_line` short-circuits on `peer?`
(`grudge.rs:163`) and practice has no peer. Flagging it because it's the
one growing-list-per-frame pattern in the metagame, and it gets worse
exactly as the ledger fills.

## Recommended order

1. **#2** — a one-line codec assert stops a crash reachable from any traded
   or web-linked tape.
2. **#1** — copy `read_profile`'s quarantine into `load_career`. Minutes.
3. **#5b + #6** — a recorder ring and a practice/duel filter. This is the
   change Sam feels most within one week.
4. **#3 + #4 + #10** — the ladder's three rules (quit penalty, cap, reset
   floor). All three are single-expression edits in `grudge.rs`/`bot.rs`
   and together they turn the gauntlet from a counter into a ladder.
5. **#7 + #8** — shade honesty; #8 reuses `raise_tape_notice`, which
   already exists and already has the copy written.
6. **#5a** — `decode_header`, once someone's library is big enough to
   notice.
7. **#9** — 3 lines, or one honest comment.
