# Build Plan

Phased execution roadmap. Do not skip phases. Each phase produces verifiable artifacts and must pass its exit criteria before the next phase begins.

Reference: `ARCHITECTURE.md` for all design decisions, `CONVENTIONS.md` for invariants.

---

## Phase 0 — Workspace Setup

**Goal:** Empty Cargo workspace with all crates declared, CI green on a no-op.

**Produces:**
- `Cargo.toml` (workspace root) listing all crates from the layout in ARCHITECTURE.md
- `rust-toolchain.toml` pinning latest stable
- Empty `lib.rs` in each crate
- `.github/workflows/ci.yml` running `cargo check --workspace --locked`
- `.gitignore`, `README.md`

**Exit criteria:**
- `cargo check --workspace --locked` passes locally and in CI
- All crate directories exist and compile

---

## Phase 1 — Fixed Math Layer

**Goal:** `fixed_math` crate complete with property tests passing on all CI platforms.

**Produces:**
- `fixed_math::Fix`, `Vec2F`, all ops per ARCHITECTURE.md
- `fixed_math::sin`/`cos`/`sin_cos`/`atan2`/`sqrt` wrapping `cordic`
- `PI`, `TWO_PI`, `HALF_PI` constants
- `Vec2F::to_f32` (the only sim→render conversion)
- `proptest`-driven properties: roundtrip add/sub, rotate-by-2π identity, sqrt-of-square, sin²+cos²=1
- `clippy.toml` denying `f32`/`f64` literals in this crate's body except in `to_f32`

**Exit criteria:**
- `cargo nextest run -p fixed_math` passes locally
- Property tests pass on linux-x64, macos-14 (ARM), aarch64-linux-android in CI
- `clippy -D warnings` clean

---

## Phase 2 — Cross-Platform Determinism CI

**Goal:** CI matrix runs property tests on all four target platforms with a locked toolchain.

**Produces:**
- `.github/workflows/determinism.yml` matrix: linux-x64, linux-aarch64, macos-14, aarch64-linux-android
- `taiki-e/setup-cross-toolchain-action` integration for ARM Linux and Android
- `Swatinem/rust-cache@v2` for caching
- A trivial test that hashes a known fixed-point computation (e.g. 1000 rotations of a vector by 0.01 radians) and asserts the hash is identical across all platforms

**Exit criteria:**
- All four platforms produce bit-identical hashes for the same fixed-point computation
- Workflow runs in under 10 minutes on first cache miss

---

## Phase 3 — bevy_ggrs Hello World

**Goal:** A single deterministic entity moves under input control. SyncTestSession passes with `check_distance: 7`.

**Produces:**
- `sim` crate skeleton with Bevy app builder
- `PlayerInput` wire format struct (4 bytes, `Pod`/`Zeroable`)
- `GgrsConfig` impl
- `PositionF`, `VelocityF` registered for rollback + checksum
- `Player { handle: usize }` marker registered
- `LastSimTickTime` resource + `record_last_tick_time` system in `AdvanceWorldSystems::Last`
- `sync_test` binary running `SyncTestSession` with synthesized inputs
- A single quad spawned at origin that moves left/right based on `stick_x`

**Exit criteria:**
- `cargo run -p sync_test -- --frames 600 --check-distance 7` runs without panic
- The quad position at frame 600 is deterministic across multiple runs locally
- The same hash check from Phase 2 extended to include `PositionF` end state matches across all CI platforms

---

## Phase 4 — Replay Format and Codec

**Goal:** `.bmrg` files round-trip cleanly through postcard. A demo recording can be saved and replayed.

**Produces:**
- `replay` crate: `Replay`, `ReplayHeader` structs with `serde` derives
- `replay::encode(replay) -> Vec<u8>` and `replay::decode(&[u8]) -> Result<Replay>`
- Magic byte and version validation in `decode`
- `record_input` and `playback_input` plugins for `sim` apps
- A roundtrip test: encode → decode → assert equal
- A small CLI tool `replay_sync` (skeleton, full impl in Phase 5)

**Exit criteria:**
- Roundtrip test passes
- A 60-frame demo recorded from `sync_test` can be replayed and produces an identical end-state hash

---

## Phase 5 — SyncTest CI Harness with Per-Component Checksums

**Goal:** `replay_sync` binary runs a canonical demo headlessly and produces a per-frame, per-component checksum TSV. CI matrix runs it cross-platform and diffs the outputs.

**Produces:**
- `replay_sync` binary: takes `--demo`, `--frames`, `--output` args. Runs sim with demo as input, writes TSV with columns `frame total_checksum positionf_part velocityf_part anglef_part ...`
- `tests/demos/canonical/match_v1.bmrg` — hand-recorded 30-second demo (use `sync_test` interactively to capture, or generate synthetically and commit)
- Updated `determinism.yml` to run `replay_sync` on all platforms and a final job that diffs all logs against linux-x64
- `scripts/diagnose_desync.sh` — takes two TSV files, finds first divergent frame and column, prints both
- `--dump-state-at <frame>` flag for `replay_sync` that pretty-prints the full sim state at that frame

**Exit criteria:**
- All four platforms produce byte-identical TSV logs from the canonical demo
- `diagnose_desync.sh` correctly identifies a planted divergence (test by temporarily injecting an `f32` op)

---

## Phase 6 — Fuzzed Soak Harness

**Goal:** A fuzzer generates random valid input streams, runs them through `replay_sync`, and either passes or commits the seed as a regression demo.

**Produces:**
- `crates/replay_sync/src/fuzz.rs` — generates random `PlayerInput` streams from a seed
- `--fuzz <seed>` flag for `replay_sync`
- Nightly workflow that runs 100 fuzzed seeds for 1 hour total
- Fuzzer that, on divergence, automatically writes the seed and inputs to `tests/demos/regressions/<seed>.bmrg` and opens an issue

**Exit criteria:**
- Nightly job runs to completion
- A planted bug surfaces as a regression demo within 10 fuzzed seeds

---

## Phase 7 — Simulation/Render Boundary

**Goal:** Visual layer interpolates between sim ticks; render runs at any framerate cleanly.

**Produces:**
- `render` crate
- `PreviousPositionF`, `PreviousAngleF` components, registered for rollback
- `snapshot_previous` system as the first system in `GgrsSchedule`
- `sync_transforms_from_sim` in `Update`: interpolates Transform from sim state using `LastSimTickTime` + tick rate
- `NoInterpolate` marker
- `snap_position(pos, prev, new)` helper for teleports
- `app` binary linking sim + render and showing a rendered moving quad

**Exit criteria:**
- Quad moves smoothly at 144Hz display, 30Hz display, and at variable framerates
- SyncTest still passes with all the new components and systems
- Manually setting `NoInterpolate` produces a snappy non-interpolated render

---

## Phase 7.5 — Android Sideload Unblock (between-phases)

**Goal:** Compile sim/replay/render/sync_test/replay_sync for `aarch64-linux-android` in CI and produce a sideloadable APK locally so dev devices ($40 Samsungs etc.) can run the build before Phase 8 lands real input.

This is not a numbered phase — it's the smallest viable cross-platform-build unblock. It does not advance gameplay; it just expands what the cross-platform determinism matrix can prove and gives the operator a way to put the binary on a phone.

**Produces:**
- `crates/sim/Cargo.toml` target-gated `android-activity` dep with `native-activity` feature, so `bevy_android`'s downstream `compile_error!` stops firing for android cross-compiles
- `.github/workflows/determinism.yml` android matrix entry expanded from `-p fixed_math` to `--workspace --exclude app`, with a split run step (`cargo nextest run` for non-android, `cargo build --tests` for android since the runner can't execute aarch64 binaries)
- `crates/app/src/lib.rs` carrying the Bevy app body with a `#[bevy_main]` entry; `crates/app/src/main.rs` slimmed to call `app::run()`
- `crates/app/Cargo.toml` `[lib] crate-type = ["cdylib", "rlib"]` plus `[package.metadata.android]` block for cargo-apk (bundle id `com.ampactorlabs.twotop`, apk_label `2-Top`, portrait, min_sdk 24, target_sdk 34, INTERNET permission, touchscreen feature)
- `SIDELOAD.md` operator runbook (NDK setup, cargo-apk install, ADB/USB debugging, the `cargo apk run -p app --target aarch64-linux-android` loop, troubleshooting table)
- Display name "2-Top" introduced as the user-facing identity; codebase identifiers stay `two-top` / `two_top` / `twotop` (Java/Rust naming rules forbid leading digits)

**Exit criteria:**
- Determinism matrix's `aarch64-linux-android` job builds `--workspace --exclude app` cleanly in CI (no `compile_error!`, no missing-feature panics)
- Local `cargo apk run -p app --target aarch64-linux-android` produces an APK that installs on a tethered Android device and shows the Phase 7 visual smoke test
- `cargo nextest run --workspace --locked` and `cargo clippy --workspace --locked -- -D warnings` stay green on host

**Deliberately deferred:**
- GameActivity (better gamepad routing) — cargo-apk2/xbuild + AndroidX bundling, picked up when input phases want it
- Multi-arch APK (armv7, x86_64 emulator) — single-arch is enough to sideload to one phone today
- Release signing / Play Store submission — debug-cert APK is fine for tethered install
- Cross-compiling `app` itself in CI — wgpu+bevy_render through the NDK on a GitHub runner is its own packaging story

---

## Phase 8 — Input Layer (Touch)

**Goal:** Touch input drives `PlayerInput` correctly on iOS, Android, and desktop (mouse drag as touch substitute for desktop).

**Produces:**
- `input_touch` crate
- `TouchState` resource, populated from Bevy `Touches` in `PreUpdate`
- Floating virtual stick logic (first touch in lower-left quadrant)
- Radial deadzone curve (12% inner, 75% saturation)
- Throw interaction state machine (tap / hold-drag-aim / release)
- `read_local_inputs` system in GGRS's `ReadInputs` schedule that converts `TouchState` to `LocalInputs<GgrsConfig>`
- Mouse-drag fallback for desktop testing
- `InputHistory` component + `pressed_within` / `released_within` / `consume` API
- 6-frame default forgiveness window
- `PreviousInputs` resource registered for rollback
- A debug overlay showing current `PlayerInput` and `InputHistory` for tuning

**Exit criteria:**
- Stick movement feels smooth on a real phone (subjective, but document the tuning values used)
- SyncTest passes with two `sync_test`-replayed inputs producing identical results
- Forgiveness buffer correctly fires actions queued slightly before they become legal

---

## Phase 9 — Player Movement

**Goal:** Player moves around an empty arena with walls. Collides with walls correctly. Dash with i-frames.

**Produces:**
- Sim systems: `player_movement`, `wall_collision`
- `Wall { kind: WallKind }` component (rectangle and edge-aligned for simplicity v1)
- `StunFrames` component for i-frames
- Dash mechanic: short directional impulse, ~10 frames of i-frames, ~20-frame cooldown
- One arena defined as a tilemap or a list of wall rects
- Camera follow in `Update` (exponential damping)

**Exit criteria:**
- Player moves at the right speed (target: cross arena in ~2 seconds)
- Wall collisions feel solid (no tunneling at max speed; sweep-test if needed)
- Dash i-frames work and SyncTest still passes
- Movement and dash are deterministic across platforms (extend canonical demo)

---

## Phase 10 — Boomerang Throw and Recall

**Goal:** Player can throw boomerang (tap or aimed), recall it, catch it.

**Produces:**
- `Boomerang { owner, state: BoomerangState }` where state ∈ `Held`, `Flying`, `Returning`
- `boomerang_physics` system
- Throw on `released_within(THROW_DOWN, 6)`: spawn `Boomerang` entity with velocity from input
- Recall on second `pressed_within(THROW_DOWN, 6)` while boomerang is `Flying`: transition to `Returning`
- Catch on collision with owner while `Returning`: despawn, owner is `Held` again
- Wall ricochet on collision: reflect velocity, optional damping
- Visual: render the boomerang sprite (placeholder bone-fang)

**Exit criteria:**
- Throw, recall, catch loop works
- Ricochets feel right
- SyncTest passes including throw/ricochet/recall sequences in the canonical demo
- Cross-platform replay still byte-identical

---

## Phase 11 — Hits, Death, Respawn

**Goal:** Players can kill each other with the boomerang. Round flow works.

**Produces:**
- Boomerang vs player collision while `Flying` or `Returning` (and not on owner): trigger death
- Death: hide player, set respawn timer (~3 seconds), increment opponent score
- Respawn: `snap_position` to a respawn point, reset state
- Hit-stop: brief `StunFrames` increase on the killer's animation for impact feel (4-6 frames)
- `MatchState` as a plain rolled-back `Resource` enum: `Countdown(3,2,1)`, `InRound`, `RoundOver`, `MatchOver`. (See MORGAN_NOTES § "Why we cut bevy_roll_safe" — `bevy_roll_safe` 0.7 caps `bevy_ggrs` at `^0.20` and we're on `=0.21`.)
- `MatchScore` resource, rolled back
- Round timer: 30 seconds, transitions through states accordingly
- First to 5 round wins ends the match

**Exit criteria:**
- Full match playable end-to-end with two `sync_test` instances
- Respawn doesn't visually interpolate (test `snap_position`)
- All sim still deterministic across platforms

---

## Phase 12 — Networking (Matchbox + Lobby)

**Goal:** Two real devices on different networks can play a match.

**Produces:**
- `net` crate: matchbox signaling client integration
- A simple lobby UI: "Find Match" button, queue, "Found, connecting..." state
- A self-hosted matchbox signaling server (or use a public one for v1)
- WebRTC peer connection through `matchbox_socket`
- Switch from `SyncTestSession` to `P2PSession` once peer is connected
- Disconnection handling: 3-second grace period, then forfeit

**Exit criteria:**
- Two devices on different networks complete a full match without desync
- Reconnection across a brief network blip works
- Forfeit on long disconnection works

---

## Phase 13 — Diagnostic Logging

**Goal:** `tracing`-based logging in place across all categories. Bug reports include a `.bmrg.log`.

**Produces:**
- `tracing-subscriber` setup in `app/main.rs` with file rotation in release, stderr in dev
- Lifecycle, Network, Error spans/events instrumented at relevant points
- `.bmrg.log` companion file written alongside saved replays
- Cargo features `release_max_level_info` confirmed

**Exit criteria:**
- A complete match produces a meaningful `.bmrg.log`
- No measurable performance regression in release builds (verify with frame timing)

---

## Phase 14 — Replay Viewer

**Goal:** A standalone or in-game replay viewer with scrub bar and frame-step.

**Produces:**
- `replay_viewer` binary (or in-game mode of `app`)
- Loads `.bmrg`, runs sim deterministically forward
- Snapshots taken every 60 frames
- Scrub bar UI: drag to jump
- Frame-step buttons (forward, backward)
- Speed selector (0.25x, 0.5x, 1x, 2x, 4x)
- Toggle HUD overlay (hitboxes, velocity vectors)

**Exit criteria:**
- Loading a saved match plays it back identically
- Scrubbing to any frame works in <100ms
- Backward step works (uses snapshot + replay-forward internally)

---

## Phase 15 — Visuals (Sprites, Animations, Particles)

**Goal:** Game looks like the demonic-HLD aesthetic. Animations driven from sim.

**Produces:**
- `AnimState`-driven sprite selection in render layer
- 4-frame idle, 6-frame throw, 2-frame dash, hit/death animations per character
- CPU-side `Particle` system: hit-burst, boomerang trail, death-explosion, ambient embers
- 16-color palette enforcement
- One full character sprite sheet
- Boomerang sprite (12×12 bone fang with 3-frame trail)

**Exit criteria:**
- Visual quality matches the aesthetic spec
- Particles cap at ~500 active without dropped frames on a mid-range Android phone
- Animations snap correctly (no smoothing)

---

## Phase 16 — Arena Content

**Goal:** Three launch arenas with distinct character.

**Produces:**
- Arena 1: open box arena, no hazards (training)
- Arena 2: arena with a central pit hazard (instant death if knocked in)
- Arena 3: arena with conveyor belt or wind zone affecting boomerang trajectory
- Arena selection screen
- Per-arena ambient color wash and tile motifs

**Exit criteria:**
- All three arenas playable end-to-end
- Each feels mechanically distinct

**History:** Cycle 1 landed as commit `58fd4ab` (arena infrastructure + Anchor's central bone pyre), then was reverted in `5bf8c81` with no documented reason. Audit of the diff found the code determinism-clean (correct `rollback_component_with_copy` + `checksum_component_with_hash` registration, deterministic ordering, SyncTest-verified). Re-implemented in the Completion Plan's M2 using `58fd4ab` as the spec (not blind cherry-picked — commit `1204aa9` changed `SimSnapshot` afterward and would conflict).

---

## Phase 17 — Pickups

**Goal:** 4-6 pickup modifiers spawn during rounds and meaningfully change boomerang behavior.

**Produces:**
- Pickup spawning system: at fixed map positions, on a timer, in deterministic locations chosen by `SimRng`
- Pickup types: Fire (lingering trail damages), Ice (freeze on hit), Bouncy (extra ricochets), Multishot (3 boomerangs), Curve (stronger curve trajectory), Heavy (slower, larger hitbox)
- Pickups apply temporarily (10-15 seconds or 1 throw)
- Visual + audio cues for each pickup type

**Exit criteria:**
- Pickups feel impactful but balanced
- All deterministic and replay-clean

---

## Phase 18 — Polish

**Goal:** Game feel matches what shipping titles deliver.

**Produces:**
- Screen shake on hits and deaths (render-side only)
- Hit-stop tuning per attack type
- Camera zoom-in on kill cam (1-second slow-mo replay of the killing throw)
- Haptics: subtle on throw, harder on hit, sharp on death
- Audio: throw, recall, hit, death, ambient demonic background
- UI: score display, round timer, match summary screen
- Settings menu: deadzone tuning, haptics on/off, audio volumes

**Exit criteria:**
- Subjective: game feels good to play for 30+ minutes
- Performance: stable 60fps on iPhone 12 / Pixel 6 baseline; degrades gracefully on older hardware

---