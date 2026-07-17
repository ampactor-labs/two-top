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

We had no use for the crate's audio plugin either (see "Why no `bevy_roll_safe` audio plugin" below — audio never touches the sim, so there's nothing to roll-safe). The states module was the only piece we'd have kept — and it was the part most coupled to bevy version churn. Pulling that thread takes the whole crate out of our dep graph forever.

## Why no `bevy_roll_safe` audio plugin

`bevy_roll_safe::RollbackAudioPlugin` wraps `bevy_audio` to suppress duplicate sounds during resimulation — the plugin exists because, if the sim itself spawned audio, rollback resim would re-fire every cue. We don't need it because *audio is never a sim concern at all.* Cues live entirely in `app` (`crates/app/src/audio.rs`), fired off `Local` previous-state edges that read *current* sim state — never `Added`/`Changed`, never rolled-back `AudioPlayer` entities. Sound is cosmetic and the sim never spawns it, so resim can't double-fire it; there's nothing for a rollback-audio plugin to guard. The plugin solves a problem we structurally don't have.

## Why audio (and haptics) live in `app`, not `render`

Audio needs an output device the way windowing needs a display — so `bevy_audio`/`cpal` belong with the device-owning `app` crate, never the determinism-core `render` crate that the 4-platform replay matrix rebuilds on every target. Keeping `render` `cpal`-free keeps the matrix build lean and keeps audio orthogonal to the bit-identical-sim guarantee: nothing the sound system does can perturb a checksum. The cues are `Local`-edge cosmetic systems in `app/audio.rs`, and the 12 `.wav` files are generated deterministically by `scripts/generate_audio.py` — so even the *content* is reproducible, not a binary blob in the repo's determinism path.

Haptics (`app/haptics.rs`) follow the same logic: a vibrator is a device, exactly like the audio sink and the window. The Android JNI `Vibrator` call lives in `app` and compiles to a no-op on every other target, so the edge-detector systems are identical across platforms and only the leaf call differs. Device-touching code lives where the device lives; the determinism core stays pure.

## Why CPU particles instead of `bevy_hanabi`

`bevy_hanabi` is a GPU compute particle system — wonderful for million-particle effects on PC. Our aesthetic wants ~50-300 chunky pixel particles per effect. The CPU side handles that with no ceremony. `bevy_hanabi` would add a substantial dependency, GPU compute requirements that some older Android devices lack, and complexity around shader compatibility. Aesthetic alignment + dependency minimalism = roll our own.

## Why postcard, not bincode

Both serialize Rust types compactly. `postcard` is no_std-friendly, smaller wire format, embedded-Rust ecosystem default, and has a more stable schema story. We don't need no_std today, but we might if we ever do anything with hardware (Daisy Seed integration, anyone?), and the embedded ecosystem alignment is the kind of "future you will thank present you" decision that costs nothing now.

## Why `cordic`, knowing it's frozen

`cordic` 0.1.5 hasn't shipped a release since May 2021. We use it anyway because the algorithm itself is mathematically stable, its API (`sin`/`cos`/`sin_cos`/`atan2`/`sqrt`) is exactly what `fixed_math` needs, and the cross-platform bit-identical determinism we need from trig is a property of the CORDIC algorithm, not the maintainer's release cadence. Pin the version, don't chase updates. If `cordic` ever breaks against a `fixed`-major bump or a Rust-edition transition, migrate to `fixed-trig` — actively maintained, overlapping API surface. Don't preemptively switch: the determinism property tests landing in Phase 1 will surface any real defect, and switching today means re-validating bit-identical trig results across the cross-platform CI matrix for no current benefit.

## Why level signals only in the wire format

If we encoded "just_pressed" / "just_released" bits and sent them, then under rollback resimulation we'd lose them — the bits would have already been consumed when we predicted. Edges have to be derived inside the sim, from the diff against `PreviousInputs`, which is rolled back. This is the same architectural shape fighting games use. It's tighter, smaller wire format, and inherently rollback-correct.

## Why rematch is driven by an input edge, not a button

A finished match (`MatchOver`) restarts on a THROW rising edge from *either* player, derived in the sim from the rolled-back `InputHistory` (`apply_rematch`). The obvious alternative — a UI "Play Again" button that mutates the `World` directly (reset score, respawn, flip state) — would never resimulate under rollback and would desync netplay peers the instant a frame rolled back across the restart. Routing the restart through the same level-signal input every other gameplay transition already uses keeps it rollback-correct and lockstep *for free*, and reuses `THROW_DOWN` so there's no wire-format change at all. This is the "level signals only" principle applied to the match lifecycle, not just to per-tick actions.

## Why the title screen is the absence of a ggrs `Session`

`AppScreen` has `Title` and `InMatch`, but the sim's idle-vs-running behavior is *not* gated on that flag — it falls out of whether a ggrs `Session` resource exists. With no `Session`, `bevy_ggrs` simply idles `GgrsSchedule`, so the Title screen is literally the no-session state: the sim isn't paused, it has nothing to advance (it sits at frame 0). `start_match` inserts a fresh SyncTest `Session`; `back_to_lobby` removes it (`remove_resource::<Session<GgrsCfg>>()`). `AppScreen` only drives entity spawn/despawn and non-sim UI systems. The payoff is zero special-case pause logic — there's no "is the sim allowed to tick right now" branch to get wrong, and you *can't* accidentally tick the sim from a menu because there's no session to tick it. The arena picker lives in `Title` and only mutates `SelectedArena` *before* the session is built, so it's a safe pre-session local change.

## What the no-session Title screen costs, and why `RollbackClockPlugin` exists

The design above has a bill attached, and it took two crash reports to read it. `bevy_ggrs` resets `RollbackFrameCount` to 0 on any tick where no `Session` exists, which is the mechanism that makes "Title == frame 0" true for free. But `GgrsTimePlugin` derives `Time<GgrsTime>` from that same counter and advances it with `Time::advance_to`, which asserts you never move time backwards. bevy_ggrs resets the counter and not the clock. So the counter says 0 and the clock still says 4.98s, and the first tick of the *second* session in a process tries to rewind time to zero and aborts the process.

The probe printed it in three lines:

```
after match 1: rollback_frame=299 ggrs_elapsed=4.983333333s
on the menu:   rollback_frame=0   ggrs_elapsed=4.983333333s
--- installing second session ---
panicked at bevy_time-0.18.1/src/time.rs:245: tried to move time backwards
```

This reached me as "replay is still crashing," and I chased the theater for a while on that description. Wrong lead. Watching a tape is just the most natural way to start a second session, and I had only ever tested tapes from a fresh launch, which is the one path that cannot fail. A second *match* crashes identically with no replay anywhere near it. `theater::tests` now pins all three cases, and the fresh-launch test is there specifically because it passing while the other two failed is what located the bug.

The fix mirrors bevy_ggrs's own reset condition (no session on this tick, zero the clock) instead of hooking each teardown site. I'd rather the two resets share one trigger than keep an inventory of every path that drops a session: `despawn_match`, `netplay`'s peer drop, forfeit. An inventory is a thing you forget to add to.

Upstream should reset the clock where it resets the counter. Worth a PR against `bevy_ggrs` if this survives contact.

## Why a configurable deadzone is determinism-safe

`Settings.stick_deadzone` is player-local and persisted to disk, which normally screams desync risk — per-machine state feeding the sim is exactly how rollback games break. It's safe here because it acts strictly *pre-wire*: it shapes the analog stick magnitude *before* quantization into the 4-byte `PlayerInput`. Two peers with different deadzones still exchange byte-identical quantized wire inputs and resimulate identically; the deadzone only changes how each player's raw touch maps into those bytes, exactly like controller sensitivity. The rule it illustrates: anything *before* wire-quantization may be local and non-deterministic; anything *consuming* the wire input must be identical everywhere. `StickDeadzone` (in `input_touch`) is read by the touch sampler before it emits `PlayerInput`, and `Settings` just mirrors the persisted value into it. (The volume, music, and haptics settings are cosmetic — they never touch the sim at all.)

## Why a 6-frame forgiveness window

3-10 frames is the typical fighting game range. 3 frames feels strict, 10 feels mushy. Mobile touch is noisier than buttons (less mechanically precise, fingers occlude the screen), so we lean toward the lenient side without going full mush. 6 frames is 100ms — long enough to forgive an honest mistime, short enough that a buffered input doesn't fire surprisingly late.

This is tunable. If playtesting shows it's too forgiving (causes accidental actions) or not enough (players feel inputs eaten), adjust per-action — different windows for throw vs. dash if needed.

## Why no diagonal snap

Snap-to-8 is a stability crutch that caps the skill ceiling. It feels good in early playtest because nothing is jittery, but at competitive levels it removes 5-10 degree precision aiming, which is exactly where high-level players differentiate. Smooth full-analog with the radial deadzone curve we specified will feel right; if some players struggle, snap is an opt-in accessibility setting later. The default must be uncompromised.

## Why movement locks during aim

This was a feel-driven call confirming Boomerang Fu's mental model: committing to a precise throw means committing your position. It creates a *risk* dimension to aiming — you make yourself a stationary target while you line up the shot. That's skill expression. Letting players move while aiming would dilute the throw to "always optimal aim, no risk," which is less interesting.

## Why strict version matching for replays

Auto-migration is a tar pit. Every sim change becomes a versioning project. Old replays slowly accumulate as a maintenance burden. Strict matching is honest: "this replay is from a previous version, here's how to view it (archived binaries via git tags)." It also forces version discipline — if I'm changing something that breaks replays, I know it immediately.

Concretely (M6): `sim::SIM_VERSION` is the value stamped into every replay header. Pre-release `main` carried the `u32::MAX` dev sentinel (`replay::DEV_SIM_VERSION`); for v1.0.0-rc1 it became `1`. The committed canonical demo is the special case — it's the one replay actually encoded to a file and verified through the *real* strict-match gate (`decode_for_sim_version` against `sim::SIM_VERSION` in `replay_sync`/`replay_viewer`), so it must stamp the real version, not the dev sentinel that the in-process struct-fed test/fuzz replays use (those never strict-decode). That's why every `SIM_VERSION` bump requires regenerating the canonical `.bmrg` (`gen_canonical --write`): forget it, and the determinism matrix rejects its own demo as a version mismatch.

## Rejected alternatives, briefly

- **4-player FFA** — rejected for MVP. The chaos hides bad balance, the rollback CPU budget gets tight with 4 players' inputs, and the matchmaking complexity isn't worth it for a portfolio piece. 1v1 is sharper and demos better.
- **Crypto/staked matches** — separate concern, separate product. Don't entangle.
- **Snap-to-8 stick** — cap on skill ceiling. Optional accessibility later.
- **Q32.32** — overkill, doubles snapshot size, slows ARM multiply. Q16.16 is correct.
- **Fixed virtual stick** (not floating) — forces players to look at their thumb. Floating is universally better on phones.
- **Auto-aim throw** — kills the skill expression. Swipe-to-throw is the right answer for aim.
- **`bevy_hanabi`** — overkill for our aesthetic and adds GPU compute requirements.
- **`bevy_roll_safe::RollbackAudioPlugin`** — unnecessary: audio is cosmetic, handled by `Local`-edge cues in `app`, not a rollback concern.
- **`bevy_transform_interpolation`** — f32-based, assumes different sim model.
- **bincode over postcard** — postcard is smaller, no_std-friendly, future-proofs hardware integration.
- **Float positions with seahash to "make it deterministic"** — fundamentally wrong. The hash is portable; the floats aren't. Still desyncs.

## Future you should remember

- The fuzzed soak harness is potentially extractable as `bevy_ggrs_fuzzharness` — a Rust ecosystem contribution with your name on it. After the game ships, factor it out.
- The replay-as-file model is the foundation for: spectator mode, anti-cheat, tournament infrastructure, "match of the week" content, server-side dispute resolution. None of those are MVP. All of them are unlocked.
- Lionel Williams's network is the spread vector for replay sharing. A clutch comeback is a 5-50KB file, not a screen recording. That's the build-in-public moment when the game is solid.
- The DigiTech connection is a separate track. Nothing about this game serves that goal directly. The reverb VST is its own project. Don't conflate.