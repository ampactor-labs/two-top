# Morgan's Notes

The why behind the what. Read this when you're confused about a decision six weeks from now. The agent doesn't need this; you do.

## The big picture

This game is a portfolio piece as much as it is a product. The technical achievement — a 1v1 mobile rollback brawler with provable cross-platform determinism, fuzzed soak testing, and replay-as-first-class-citizen architecture — is the moat. DigiTech R&D doesn't care about brawlers, but the broader Rust/games engineering world does, and the techniques here transfer cleanly to any audio plugin or DSP product that needs deterministic cross-platform behavior. The reverb VST and this game share more DNA than they look — both are deterministic state machines processing input streams.

Build in public throughout. Every CI green run is a tweet. The fuzzed soak harness, when it finds its first non-trivial bug, is a tweet. The replay-as-shareable-file is a Vinyl Williams network moment when it lands.

## Why Q16.16 not Q32.32

Q32.32 is wildly overkill for a 30m × 20m arena. Q16.16 with 1 unit = 1cm gives 3-millimeter range headroom and ~150-nanometer precision; players cannot perceive sub-cm deviations. Q32.32 doubles the snapshot size for no perceptible benefit and forces i128 multiplication, which is slow on 32-bit ARM (some older Android targets) and not free on 64-bit ARM either. Quake and DOOM both used Q16.16 for the same reason — sufficient range for map dimensions, sufficient precision for collision, smallest viable size.

If we ever build a larger-arena game (we won't, the game design is firmly committed to tight 1v1 portrait arenas), we'd reconsider. Not now.

## Why 1cm units

The unit choice is governed by integer-part headroom. With 1 unit = 1m, we'd burn 99% of our integer space on fractional values; sub-meter precision would suffer. With 1 unit = 1mm, squared distances would overflow i32 at 32m separation. 1cm sits right in the middle: arena dimensions are 3-4-digit integers, squared values stay safely in i64, sub-cm precision via the fractional bits is overkill in the right direction.

## Why interpolation, not extrapolation, at the render boundary

Rollback netcode is *itself* a form of forward prediction at the input layer. Adding extrapolation at the render layer would mean predicting on top of predicting — small errors compound into visible artifacts (boomerangs appearing to teleport when ricochet predictions are wrong). Interpolation costs one tick (~16ms) of latency, which is invisible against the 30-50ms touch hardware baseline plus network. Sweaty competitive players need *visual stability* over *apparent responsiveness*. They will adapt to a constant 16ms; they will not adapt to visual jumps.

## Why we rolled our own interpolation

`bevy_transform_interpolation` is f32-throughout and assumes you write to `Transform` in `FixedUpdate`. We don't — our sim writes to `PositionF` (fixed-point) and the render layer derives `Transform`. Integrating their crate would mean fighting both their assumptions and ours. The 80 lines of interpolation we own are simpler than the wrapper we'd otherwise write. Build deeper not wider.

## Why we cut `bevy_roll_safe`

Phase 11 cycle 5 was supposed to land `MatchState` via `bevy_roll_safe::init_ggrs_state`. When we got there, `bevy_roll_safe` 0.7.0 (latest, also HEAD) constrained `bevy_ggrs ^0.20` and we'd already shipped Phases 8/9/10 on `bevy_ggrs = 0.21`. The crate doesn't accept us; we don't accept downgrading.

The blast radius of each escape was uneven:
- **Forking `bevy_roll_safe`** with a bumped `bevy_ggrs` would lock us to a fork forever — bevy_ggrs 0.20 → 0.21 had real breaking changes (session builder, synctest warmup tick, checksum_hasher) that the fork would need to absorb and re-absorb on every future `bevy_ggrs` bump. Permanent maintenance overhead for a feature we don't really need.
- **Downgrading `bevy_ggrs` to 0.20** would re-validate Phase 8/9/10 against the older API, almost certainly drift the cross-platform determinism baseline checksum, and require re-recording the canonical `.bmrg`. Weeks of churn for hooks we don't use.
- **Holding the phase** means waiting on someone else's release cadence with Phase 12 (networking) blocked behind it.

Going DIY instead — `MatchState` as a plain rolled-back `Resource` enum — costs us the `OnEnter` / `OnExit` lifecycle hooks `bevy_roll_safe` provides, and gives us the rest:

1. **Pattern consistency.** Every other rollback primitive in the sim (`FrameCount`, `MatchScore`, `InputHistory`) is a plain managed `Resource`. Making `MatchState` one too removes a concept boundary instead of inventing one.
2. **Independence from upstream timing.** CONVENTIONS.md already warned us: *"Downstream rollback crates lag main bevy releases — chasing latest first guarantees broken determinism CI."* Cutting the lagging crate isn't deviating from spec — it's the spec applied recursively. We never have to coordinate three crates' upgrades again.
3. **No lost capability.** Our state transitions live inside frame-counted gameplay systems that already see everything they need. The "init this when state becomes X" pattern is `if curr != prev { ... }`. Three lines beats a plugin abstraction.
4. **Phase 14 (Replay Viewer) is happier.** Scrubbing through replay-time states becomes "read the resource at frame X" instead of reconstructing `State<>` machinery from rollback snapshots.

We were already planning to build our own `RollbackSound` audio pattern (see "Why no `bevy_roll_safe` audio plugin" below). The states module was the only piece we kept — and it was the part most coupled to bevy version churn. Pulling that thread takes the whole crate out of our dep graph forever.

## Why no `bevy_roll_safe` audio plugin

`bevy_roll_safe::RollbackAudioPlugin` wraps `bevy_audio` to prevent duplicate sounds during resimulation. It works, but it's a black box and tightly couples our audio to `bevy_audio` specifically. The `RollbackSound` entity pattern (sim spawns tagged entities, render reconciles and plays) is more code, but it's *our* code, gives us per-sound priority control, hit-cancel logic, and lets us swap audio backends later. Worth the extra ~100 lines.

## Why CPU particles instead of `bevy_hanabi`

`bevy_hanabi` is a GPU compute particle system — wonderful for million-particle effects on PC. Our aesthetic wants ~50-300 chunky pixel particles per effect. The CPU side handles that with no ceremony. `bevy_hanabi` would add a substantial dependency, GPU compute requirements that some older Android devices lack, and complexity around shader compatibility. Aesthetic alignment + dependency minimalism = roll our own.

## Why postcard, not bincode

Both serialize Rust types compactly. `postcard` is no_std-friendly, smaller wire format, embedded-Rust ecosystem default, and has a more stable schema story. We don't need no_std today, but we might if we ever do anything with hardware (Daisy Seed integration, anyone?), and the embedded ecosystem alignment is the kind of "future you will thank present you" decision that costs nothing now.

## Why `cordic`, knowing it's frozen

`cordic` 0.1.5 hasn't shipped a release since May 2021. We use it anyway because the algorithm itself is mathematically stable, its API (`sin`/`cos`/`sin_cos`/`atan2`/`sqrt`) is exactly what `fixed_math` needs, and the cross-platform bit-identical determinism we need from trig is a property of the CORDIC algorithm, not the maintainer's release cadence. Pin the version, don't chase updates. If `cordic` ever breaks against a `fixed`-major bump or a Rust-edition transition, migrate to `fixed-trig` — actively maintained, overlapping API surface. Don't preemptively switch: the determinism property tests landing in Phase 1 will surface any real defect, and switching today means re-validating bit-identical trig results across the cross-platform CI matrix for no current benefit.

## Why level signals only in the wire format

If we encoded "just_pressed" / "just_released" bits and sent them, then under rollback resimulation we'd lose them — the bits would have already been consumed when we predicted. Edges have to be derived inside the sim, from the diff against `PreviousInputs`, which is rolled back. This is the same architectural shape fighting games use. It's tighter, smaller wire format, and inherently rollback-correct.

## Why a 6-frame forgiveness window

3-10 frames is the typical fighting game range. 3 frames feels strict, 10 feels mushy. Mobile touch is noisier than buttons (less mechanically precise, fingers occlude the screen), so we lean toward the lenient side without going full mush. 6 frames is 100ms — long enough to forgive an honest mistime, short enough that a buffered input doesn't fire surprisingly late.

This is tunable. If playtesting shows it's too forgiving (causes accidental actions) or not enough (players feel inputs eaten), adjust per-action — different windows for throw vs. dash if needed.

## Why no diagonal snap

Snap-to-8 is a stability crutch that caps the skill ceiling. It feels good in early playtest because nothing is jittery, but at competitive levels it removes 5-10 degree precision aiming, which is exactly where high-level players differentiate. Smooth full-analog with the radial deadzone curve we specified will feel right; if some players struggle, snap is an opt-in accessibility setting later. The default must be uncompromised.

## Why movement locks during aim

This was a feel-driven call confirming Boomerang Fu's mental model: committing to a precise throw means committing your position. It creates a *risk* dimension to aiming — you make yourself a stationary target while you line up the shot. That's skill expression. Letting players move while aiming would dilute the throw to "always optimal aim, no risk," which is less interesting.

## Why strict version matching for replays

Auto-migration is a tar pit. Every sim change becomes a versioning project. Old replays slowly accumulate as a maintenance burden. Strict matching is honest: "this replay is from a previous version, here's how to view it (archived binaries via git tags)." It also forces version discipline — if I'm changing something that breaks replays, I know it immediately.

## Rejected alternatives, briefly

- **4-player FFA** — rejected for MVP. The chaos hides bad balance, the rollback CPU budget gets tight with 4 players' inputs, and the matchmaking complexity isn't worth it for a portfolio piece. 1v1 is sharper and demos better.
- **Crypto/staked matches** — separate concern, separate product. Don't entangle.
- **Snap-to-8 stick** — cap on skill ceiling. Optional accessibility later.
- **Q32.32** — overkill, doubles snapshot size, slows ARM multiply. Q16.16 is correct.
- **Fixed virtual stick** (not floating) — forces players to look at their thumb. Floating is universally better on phones.
- **Auto-aim throw** — kills the skill expression. Swipe-to-throw is the right answer for aim.
- **`bevy_hanabi`** — overkill for our aesthetic and adds GPU compute requirements.
- **`bevy_roll_safe::RollbackAudioPlugin`** — too coupled to bevy_audio.
- **`bevy_transform_interpolation`** — f32-based, assumes different sim model.
- **bincode over postcard** — postcard is smaller, no_std-friendly, future-proofs hardware integration.
- **Float positions with seahash to "make it deterministic"** — fundamentally wrong. The hash is portable; the floats aren't. Still desyncs.

## Future you should remember

- The fuzzed soak harness is potentially extractable as `bevy_ggrs_fuzzharness` — a Rust ecosystem contribution with your name on it. After the game ships, factor it out.
- The replay-as-file model is the foundation for: spectator mode, anti-cheat, tournament infrastructure, "match of the week" content, server-side dispute resolution. None of those are MVP. All of them are unlocked.
- Lionel Williams's network is the spread vector for replay sharing. A clutch comeback is a 5-50KB file, not a screen recording. That's the build-in-public moment when the game is solid.
- The DigiTech connection is a separate track. Nothing about this game serves that goal directly. The reverb VST is its own project. Don't conflate.