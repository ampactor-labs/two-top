# Architecture

## Project Overview

**2-Top** is a 1v1 mobile rollback brawler in portrait orientation. Mechanics are derived from Boomerang Fu: each player has one thrown-and-recalled boomerang, dash with i-frames, one-hit kills, 30-second rounds, best-of-N format. The aesthetic is demonic-Duke pixel art executed with Hyper Light Drifter discipline.

Targets: iOS, Android, and web (PWA over WebRTC). Same Rust codebase across all three.

Networking is GGRS-style rollback peer-to-peer with WebRTC transport via Matchbox. Fairness, determinism, and frame-stable visuals are non-negotiable.

## Tech Stack

| Layer | Crate / Tool |
|---|---|
| Engine | `bevy` |
| Rollback netcode | `bevy_ggrs` (built on `ggrs`) |
| Transport | `matchbox_socket` + `bevy_matchbox` (Bevy resource pump) + signaling server |
| Fixed-point math | `fixed` (Q16.16 via `FixedI32<U16>`) |
| Trig / sqrt | `cordic` |
| Deterministic RNG | hand-rolled `SimRng` (xorshift64*, single `u64` state — sim has no `rand_xoshiro` dep; `rand_xoshiro` is a `replay_sync`-only fuzzer dep) |
| Rollback states | Plain `Resource` enums (rolled back through bevy_ggrs directly — see MORGAN_NOTES § "Why we cut bevy_roll_safe") |
| Serialization | `postcard` |
| Logging | `tracing` + `tracing-subscriber` |
| Property testing | `proptest` |
| Cross-target build | `cross` / `taiki-e/setup-cross-toolchain-action` |
| Wire-format POD | `bytemuck` (`Pod` / `Zeroable` derives for `PlayerInput`) |
| Audio | `bevy_audio` — *not* a default; the `app` crate sets `default-features = false` and explicitly enables `bevy_audio` + `wav` (app-only). Pulls `cpal`/`alsa-sys`. |

## Workspace Layout
(Use `two_top` for Rust crate names since hyphens aren't valid there. Filesystem path stays `two-top`; the digit-form `2-Top` is the display name only.)

```
two-top/
├── Cargo.toml                     # workspace root
├── rust-toolchain.toml            # pinned stable version
├── crates/
│   ├── fixed_math/                # Q16.16 vocabulary, Vec2F, trig
│   ├── sim/                       # deterministic simulation; no bevy_render
│   ├── net/                       # matchbox + GGRS session plumbing
│   ├── replay/                    # postcard codec, demo loader, playback
│   ├── render/                    # sprites, particles, transform sync, effect sprites, screen-shake state
│   ├── input_touch/               # touch -> PlayerInput quantization
│   ├── input_desktop/             # PC keyboard/gamepad ReadInputs source
│   ├── app/                       # main game binary
│   ├── sync_test/                 # SyncTestSession harness binary
│   ├── replay_sync/               # headless replay-and-checksum binary (CI)
│   └── replay_viewer/             # scrubbing replay playback app
├── tests/
│   └── demos/
│       ├── canonical/             # hand-recorded match demos
│       ├── stress/                # edge-case demos
│       └── regressions/           # auto-committed by fuzzer
├── scripts/
│   └── diagnose_desync.sh
└── .github/workflows/
    ├── ci.yml
    └── determinism.yml
```

The `sim` crate has zero `bevy_render` dependency. The `render` crate depends on `sim` read-only.

The `app` crate is split into focused modules (`crates/app/src/`):

- `audio` — `GameAudioPlugin`: synthesized WAV cues + ambient bed (cosmetic; see § Audio).
- `camera` — `CameraFollowPlugin`: base follow + kill-cam zoom + screen-shake rig.
- `haptics` — Android JNI `Vibrator` (Settings-gated; no-op off-Android).
- `screen` — `AppScreen` Title ↔ InMatch state machine (see § App Screen Lifecycle).
- `settings` — persisted JSON `Settings` (see § Settings).
- `netplay` — matchbox session swap + `LocalPlayerHandle`.
- `logging` — `tracing-subscriber` setup + match-lifecycle log edges.
- `lobby_overlay` / `debug_overlay` — on-screen netplay-status and dev overlays.

## Determinism Rules

The seven sources of non-determinism and how each is eliminated:

| Source | Rule |
|---|---|
| Floating-point | No `f32`/`f64` in `sim` crate. Fixed-point (`Fix`, `Vec2F`) only. |
| System execution order | All sim systems explicitly ordered with `.before()`/`.after()` in `GgrsSchedule`. |
| Entity iteration order | Queries that depend on order sort by `RollbackId` first. |
| PRNG | One seeded xorshift64* `SimRng` resource, rolled back. Visual RNG is separate, not rolled back. |
| Time | `Time<GgrsTime>` and `FrameCount(u32)` resource only. Never `Instant::now()` or `Time<Real>` inside `GgrsSchedule`. |
| Allocator | `BTreeMap` or `Vec<(K,V)>` in sim. Never `HashMap` with random hasher. |
| External I/O | Hard wall: sim is `(state, inputs) -> state'`. Audio, rendering, networking happen outside `GgrsSchedule`. |

## Fixed Math Layer

```rust
// crates/fixed_math/src/lib.rs

pub type Fix = fixed::types::I16F16;
pub type FixWide = fixed::types::I32F32;  // intermediates only

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Vec2F { pub x: Fix, pub y: Fix }

impl Vec2F {
    pub const ZERO: Vec2F;
    pub const fn new(x: Fix, y: Fix) -> Self;
    pub fn from_cm(x: i32, y: i32) -> Self;

    pub fn length_sq(self) -> Fix;       // wide_mul; saturates at Fix::MAX (~181cm magnitude)
    pub fn length_sq_wide(self) -> FixWide;  // for arena-scale ranking, no saturation
    pub fn length(self) -> Fix;          // cordic::sqrt via FixWide intermediate
    pub fn normalize(self) -> Vec2F;     // returns ZERO on zero input
    pub fn dot(self, other: Vec2F) -> Fix;
    pub fn cross(self, other: Vec2F) -> Fix;  // 2D scalar cross
    pub fn rotate(self, radians: Fix) -> Vec2F;
    pub fn angle(self) -> Fix;           // atan2

    pub fn to_f32(self) -> (f32, f32);   // RENDER ONLY
}

pub fn sin(x: Fix) -> Fix;
pub fn cos(x: Fix) -> Fix;
pub fn sin_cos(x: Fix) -> (Fix, Fix);
pub fn atan2(y: Fix, x: Fix) -> Fix;
pub fn sqrt(x: Fix) -> Fix;

pub const PI: Fix;
pub const TWO_PI: Fix;
pub const HALF_PI: Fix;
```

**Unit convention:** 1 unit = 1 centimeter. Arena dimensions, velocities, hitbox radii all in cm.

**No `From<f32> for Fix`.** Constants must be built at compile time via `Fix::const_from_int(integer_literal)` (or `Fix::lit("…")` for non-integer constants). The `const_fixed_from_int!` macro was deprecated in `fixed` 1.20 and is slated for removal.

**`length_sq` is the default for distance comparisons.** `length()` only when the magnitude is genuinely needed.

## Component Model

All rollback components derive:
```rust
#[derive(Component, Clone, Copy, Hash, PartialEq, Eq, Debug)]
#[require(Rollback)]
```

Position / motion:
- `PositionF(Vec2F)`
- `PreviousPositionF(Vec2F)` — set by `snapshot_previous` at start of `GgrsSchedule`
- `VelocityF(Vec2F)`

Identity / structure:
- `Player { handle: usize }`
- `Boomerang { owner_handle: usize, state: BoomerangState }` — `BoomerangState` is *also* registered as its own rollback component
- `Wall { kind: WallKind, rect: RectF }` — static arena geometry, *not* a `Rollback` component

State:
- `AnimState { anim_id: u8, ticks: u16 }`
- `StunFrames(u32)` — drives hit-stop and i-frames
- `DashState` — `Idle` / `Dashing { frames_remaining, dir }` / `Cooldown { frames_remaining }`
- `Dead { respawn_at_frame: Option<u32> }`
- `Empowered(bool)` — set by a perfect catch; consumed by the next throw
- `HeldModifier(Option<PickupKind>)` — the pickup a player is carrying
- `BoomerangMods { modifier, is_secondary, despawn_at_frame }` — per-boomerang pickup/multishot state
- `Pickup { kind, slot, despawn_at_frame }` — a pickup waiting on the floor
- `FireTrailCell { owner_handle, expires_at_frame }` — a burning floor cell laid by a Fire boomerang
- `BonePyre { rect, shattered, chain_group }` — Reliquary's shatterable arena prop

`InputHistory` is a rolled-back *resource* (a `BTreeMap<usize, [PlayerInput; 8]>`), not a component — see the resources list.

Markers:
- `NoInterpolate` (render-side flag, but rolled back if on a sim entity)

Rolled-back resources:
- `FrameCount(u32)`
- `InputHistory` — per-player ring buffer; `advance_input_history` pushes the tick's inputs at end of `GgrsSchedule`
- `SimRng { state: u64 }` — hand-rolled xorshift64*
- `MatchState` — plain `Resource` enum (data-carrying: `Countdown { digit, expires_at_frame }` / `InRound { expires_at_frame }` / `RoundOver { expires_at_frame }` / `MatchOver`), rolled back via `rollback_resource_with_copy::<MatchState>()`
- `MatchScore { p0: u8, p1: u8 }`
- `BridgeState`, `DoorCooldown` — Crossing/Reliquary arena state
- `PickupSpawnTimer` — earliest frame the next pickup may appear

Registration in `sim::SimPlugin::build` (NOT app setup):
```rust
app
    .rollback_component_with_copy::<PositionF>()
    .rollback_component_with_copy::<PreviousPositionF>()
    .rollback_component_with_copy::<VelocityF>()
    .rollback_component_with_copy::<Player>()
    .rollback_component_with_copy::<NoInterpolate>()
    .rollback_component_with_copy::<DashState>()
    .rollback_component_with_copy::<StunFrames>()
    .rollback_component_with_copy::<Dead>()
    .rollback_component_with_copy::<Boomerang>()
    .rollback_component_with_copy::<BoomerangState>()
    .rollback_component_with_copy::<AnimState>()
    .rollback_component_with_copy::<Empowered>()
    .rollback_component_with_copy::<HeldModifier>()
    .rollback_component_with_copy::<BoomerangMods>()
    .rollback_component_with_copy::<Pickup>()
    .rollback_component_with_copy::<FireTrailCell>()
    .rollback_component_with_copy::<BonePyre>()

    .rollback_resource_with_copy::<FrameCount>()
    .rollback_resource_with_copy::<MatchScore>()
    .rollback_resource_with_copy::<MatchState>()
    .rollback_resource_with_copy::<BridgeState>()
    .rollback_resource_with_copy::<DoorCooldown>()
    .rollback_resource_with_copy::<SimRng>()
    .rollback_resource_with_copy::<PickupSpawnTimer>()
    .rollback_resource_with_clone::<InputHistory>()

    .checksum_component_with_hash::<PositionF>()
    .checksum_component_with_hash::<PreviousPositionF>()
    .checksum_component_with_hash::<VelocityF>()
    .checksum_component_with_hash::<DashState>()
    .checksum_component_with_hash::<StunFrames>()
    .checksum_component_with_hash::<Dead>()
    .checksum_component_with_hash::<Boomerang>()
    .checksum_component_with_hash::<AnimState>()
    .checksum_component_with_hash::<Empowered>()
    .checksum_component_with_hash::<HeldModifier>()
    .checksum_component_with_hash::<BoomerangMods>()
    .checksum_component_with_hash::<Pickup>()
    .checksum_component_with_hash::<FireTrailCell>()
    .checksum_component_with_hash::<BonePyre>()
    .checksum_resource_with_hash::<FrameCount>()
    .checksum_resource_with_hash::<MatchScore>()
    .checksum_resource_with_hash::<MatchState>()
    .checksum_resource_with_hash::<BridgeState>()
    .checksum_resource_with_hash::<DoorCooldown>()
    .checksum_resource_with_hash::<SimRng>()
    .checksum_resource_with_hash::<PickupSpawnTimer>()
    .checksum_resource_with_hash::<InputHistory>();
```

`Wall` is *not* registered for rollback (static geometry); `BoomerangState` is registered even though the boomerang's live state is read through the `Boomerang.state` field.

**Every component on a rollback entity must be registered, including markers.** Unregistered markers cause silent desyncs after entity respawn.

## Schedules

`GgrsSchedule` (rollback, deterministic) — one explicit `.chain()` (see `sim::SimPlugin::build`):

1. `snapshot_previous` — copy `PositionF` → `PreviousPositionF`
2. `tick_respawn` — revive any player whose `Dead.respawn_at_frame` has arrived
3. `apply_rematch` — a THROW rising edge during `MatchOver` restarts the match (input-driven; see § App Screen Lifecycle and below)
4. `tick_match_state` — drive the round/match state machine forward (countdown digits, round/match transitions)
5. `start_dash` — commit dash on a DASH_DOWN edge
6. `player_movement` — apply movement input to `VelocityF` and `PositionF`
7. `wall_collision` — players vs walls
8. Boomerang / arena cluster (its own inner `.chain()`): `recall_boomerangs`, `curve_boomerangs`, `boomerang_physics`, `boomerang_wall_collision`, `expire_secondary_boomerangs`, `drop_fire_trail`, `boomerang_pyre_collision`, `chain_ignition` (Reliquary, `run_if`), `boomerang_sigil_collision` (Crossing, `run_if`), `hit_boomerang_player`, `fire_trail_kills`, `chasm_kills` (Crossing, `run_if`), `sigil_door_teleport` (Reliquary, `run_if`), `catch_boomerangs`, `throw_boomerangs`
9. `pickup_spawner` — spawn at most one floor pickup on a randomized timer
10. `collect_pickups` — a player walking over a pickup collects it
11. `expire_pickups` — despawn pickups that sat uncollected too long
12. `expire_fire_trail` — despawn burned-out fire cells
13. `tick_player_timers` — count down `DashState` / `StunFrames`
14. `advance_animation` — advance `AnimState.ticks`, transition anims
15. `advance_frame_count` — bump `FrameCount`
16. `advance_input_history` — push this tick's inputs into `InputHistory` for next tick's edge diffing
- `record_last_tick_time` — runs in the `AdvanceWorld` schedule under `AdvanceWorldSystems::Last` (not `GgrsSchedule`), writes `LastSimTickTime`

The inner boomerang/arena cluster is its own nested `.chain()` so the outer chain stays under Bevy's 20-element `.chain()` arity limit. Arena-specific systems are gated with `run_if(arena_is(_))` rather than split into separate schedules.

`apply_rematch` is the one escape from the terminal `MatchOver` state: a THROW rising edge (diffed against the rolled-back `InputHistory`, no wire-format change) from either player resets score to 0-0, returns `MatchState` to the top of the countdown, and wipes the arena. It is input-driven so it resimulates identically under rollback and stays lockstep across netplay peers — never an out-of-band `World` reset.

`Update` (render, f32 allowed):

- `sync_transforms_from_sim` — interpolated `Transform` from `PositionF`/`PreviousPositionF` (render crate)
- camera rig — the app-side `CameraFollowPlugin` (`crates/app/src/camera.rs`): `update_camera_base` + `update_kill_cam`, then `compose_camera`, which reads render's `ScreenShake`/`LastKillPos` resources to compose base follow + kill-cam zoom + shake offset
- effect sprites / particles — CPU-side, render-only `CosmeticRng` (capped; see § Particles)
- audio cues — the app-side `GameAudioPlugin` edge-detectors (see § Audio)
- `ui` — HUD, scrub bar, menus

## Simulation/Render Boundary

**Module rule:** `sim` crate forbids `bevy_render`, `bevy::transform`, and `glam` imports. `render` crate has read-only access to `sim` types.

**Interpolation:**
```rust
let tick_dt = 1.0 / tick_rate as f32;
let alpha = ((Time<Real>.elapsed_secs() - LastSimTickTime.0) / tick_dt).clamp(0.0, 1.0);
// Lerp Vec2F via to_f32() conversions
// Angles: shortest-path lerp through ±π
```

`NoInterpolate` marker disables interpolation per entity.

`snap_position(pos, prev_pos, new_pos)` sets `prev_pos.0 = new_pos` to kill lerp on teleports (respawns, etc.).

**Animation does not interpolate.** Pixel art snaps frame-to-frame at 60Hz tick rate. `AnimState.frame` is sim state; render reads it and picks the sprite.

**Audio:** cosmetic, and lives entirely in the `app` crate (`crates/app/src/audio.rs`, `GameAudioPlugin`) — *not* in `sim` or `render`. There is no `RollbackSound` component, no `SoundKind`, and nothing audio-related in `GgrsSchedule`. Twelve assets (eleven one-shot WAV cues + a looping ambient bed) are generated by `scripts/generate_audio.py` into `assets/audio/*.wav`. Cues fire from `Update` edge-detector systems that diff the *current* sim state against a `Local` prev-state map (the same edge pattern the render effect spawners use — never `Added`/`Changed`, which rollback re-fires). `AudioPlayer` entities are never rolled back. `bevy_audio`/`cpal` are kept in `app` alone so the crates in the determinism matrix (`sim`/`render`/etc.) stay device-free.

**Particles:** CPU-side custom system. `EffectSprite` entities, spawned by render-side systems reading sim transitions, advanced and faded each frame. Capped at `EFFECT_SPRITE_CAP = 500` active — `cull_excess_effects` / `select_effect_culls` drop the lowest-priority excess per frame whenever the count exceeds the cap.

**Render-side M5 types** (`crates/render/src/lib.rs`): `ScreenShake` / `LastKillPos` resources (read by the app camera rig), the `ArenaProp` marker (so `OnExit(InMatch)` can tear down arena props in one query), `EffectSprite` with `EFFECT_SPRITE_CAP`, `FloorStain` (blood decals), and `CosmeticRng` (the non-rolled-back cosmetic RNG).

## App Screen Lifecycle

The `app` crate wraps the match in a small bevy `States` machine (`crates/app/src/screen.rs`; the `app` crate enables the `bevy_state` feature):

```rust
#[derive(States, ... Default)]
pub enum AppScreen { #[default] Title, InMatch }
```

- **`Title`** — session-less. With no ggrs `Session` inserted, bevy_ggrs idles `GgrsSchedule` and the sim sits at frame 0. The title overlay carries the arena picker (1/2/3 keys, or a tap in the upper-half cycles arenas) and the settings keys (see § Settings). Pressing start (Space/Enter, or a tap in the lower half) transitions to `InMatch`.
- **`InMatch`** — `OnEnter(InMatch)` spawns the two players, arena walls, and the selected arena's props; in couch (local SyncTest) mode it then inserts a *fresh* `SyncTestSession` via `build_synctest_session`, so the rollback frame count restarts at 0 every match. `OnExit(InMatch)` despawns every match/arena/play-spawned entity and removes the `Session<GgrsCfg>` so bevy_ggrs idles the sim back at frame 0.

Online also boots to `Title`. With a room URL configured, the title copy becomes "TAP TO FIND OPPONENT"; pressing start enters `InMatch`, starts the matchbox connection, and `perform_swap` inserts the P2P session once the peer connects. Arena selection still happens on the Title screen before the online connection starts.

## Settings

Persisted player settings live in `crates/app/src/settings.rs`:

```rust
#[derive(Resource, Serialize, Deserialize, ...)]
#[serde(default)]
pub struct Settings {
    pub stick_deadzone: f32,
    pub haptics: bool,
    pub sfx_volume: f32,
    pub music_volume: f32,
}
```

Stored as JSON at `dirs::config_dir()/two-top/settings.json` (via `serde`/`serde_json`/`dirs`). `#[serde(default)]` keeps old files forward-compatible, and `Settings::clamped()` pins every field into range (non-finite falls back to the default). Adjusted only on the `Title` screen (H toggles haptics, `-`/`=` SFX, `[`/`]` music, `,`/`.` deadzone), saved on every change.

The deadzone is mirrored into `input_touch::StickDeadzone`, which shapes the virtual stick *before* quantization to the wire format — a legal pre-wire input change, never post-wire. `DEADZONE_DEFAULT = 0.12` is the baseline; `DEADZONE_MAX = 0.40` the upper bound.

## Input Model

Wire format, exactly 4 bytes per player per frame:
```rust
#[repr(C)]
#[derive(Default, Clone, Copy, Hash, PartialEq, Eq, Debug, Pod, Zeroable)]
pub struct PlayerInput {
    pub stick_x: i8,    // -127..127
    pub stick_y: i8,    // -127..127
    pub aim_angle: u8,  // 0..255 maps to 0..2π
    pub buttons: u8,
}

impl PlayerInput {
    pub const THROW_DOWN: u8 = 0b0000_0001;
    pub const AIM_ACTIVE: u8 = 0b0000_0010;
    pub const DASH_DOWN:  u8 = 0b0000_0100;
    pub const TAUNT_DOWN: u8 = 0b0000_1000;
    // bits 4-7 reserved
}
```

**Level signals only — no edges in wire format.** Edges derived in sim by diffing against the rolled-back `InputHistory`.

**While `AIM_ACTIVE` is set, `stick_x`/`stick_y` encode aim drag direction and power.** Movement is locked. Tip-magnitude is power, direction is angle. The `aim_angle` byte is redundant when AIM_ACTIVE but kept for fast lookup and for the sticky-on-release frame.

**Touch layer (`PreUpdate`, not rolled back):**
- `TouchState` resource updated from Bevy's `Touches`
- Floating virtual stick: the screen is split down the center — the first touch in the left half becomes the move stick, origin at first-touch position; the right half (minus the bottom-right dash corner) drives throw/aim the same way
- Radial deadzone curve: 12% inner deadzone (the `DEADZONE_DEFAULT` of the configurable `stick_deadzone`, clamped to `DEADZONE_MAX = 0.40`; see § Settings), 75% saturation
- Three throw interactions (the right half is a throw BUTTON; the left stick aims, mirroring the desktop throw-key + d-pad model):
  - **Tap** with the left stick centered: instant throw in facing direction at default power
  - **Hold + deflect the left stick**: `AIM_ACTIVE` set, stick bytes encode the aim (sim roots the character while charging, so the stick is free)
  - **Release while aiming**: throw fires with last-frame aim values; the aim is sticky for one frame across release, even if both thumbs lift at once

**Forgiveness buffer:**
- `InputHistory` resource (per-handle ring buffer of 8 frames, ~133ms)
- Default forgiveness window: 6 frames (~100ms)
- API: `pressed_within(mask, frames)`, `released_within(mask, frames)`, `consume(mask)` to clear after action triggers

## Replay Format

`.bmrg` files, postcard-encoded:
```rust
pub struct Replay {
    pub header: ReplayHeader,
    pub inputs: Vec<FrameInputs>,  // FrameInputs = [PlayerInput; num_players]
}

pub struct ReplayHeader {
    pub magic: [u8; 4],            // b"BMRG"
    pub format_version: u16,
    pub sim_version: u32,           // sim::SIM_VERSION (1 for v1.0.0-rc1; u32::MAX only on un-tagged dev builds via DEV_SIM_VERSION)
    pub seed: u64,
    pub num_players: u8,
    pub frame_rate: u8,
    pub frame_count: u32,
    pub recorded_at: u64,
    pub winner: Option<u8>,
    pub player_handles: [Option<String>; 2],
    pub arena_id: u8,
}
```

Strict version matching: replays only load if `sim_version` matches binary. Old replays viewed via archived git-tagged binaries. The committed canonical demo (`tests/demos/canonical/match_v1.bmrg`) stamps the real `sim::SIM_VERSION` (via `replay_sync::canonical_replay`) and must be regenerated on any `SIM_VERSION` bump; the in-process struct-fed test/fuzz replays keep using `DEV_SIM_VERSION` (`u32::MAX`) since they never strict-decode.

## Logging

`tracing` crate. `Cargo.toml`:
```toml
tracing = { version = "0.1", features = ["release_max_level_info"] }
```

Categories:
- **Lifecycle** (INFO, release): match start/end, score, round transitions
- **Network** (WARN/INFO, release): desync detected, latency, peer events
- **Error** (ERROR, release): file errors, state corruption
- **SimDebug** (DEBUG, dev only): per-frame events
- **Performance** (TRACE, opt-in): timing, rollback stats

Diagnostic events written to `.bmrg.log` companion file (JSON Lines).

## CI Strategy

Three-layer determinism defense:

**Layer 1: SyncTestSession.** Single-machine. Runs every CI job via `cargo nextest run --workspace --locked` in `ci.yml`, which executes `crates/sim/tests/determinism.rs::determinism_locked_600_frame_synctest` — a 600-frame SyncTest with `check_distance: 7`, `input_delay: 2`. Catches intra-machine non-determinism. (The live app + `replay_sync` use `check_distance: 2` to keep the per-frame resimulation cost down; the test bumps it to 7 for stricter coverage.)

**Layer 2: Cross-platform replay matrix.** `replay_sync` binary runs `tests/demos/canonical/match_v1.bmrg` headlessly on each platform, dumps per-frame per-component checksum log. Diff job compares all logs against linux-x64 baseline. Platforms: linux-x64, linux-aarch64 (qemu), macos-14 (native ARM), aarch64-linux-android (now `--workspace --exclude app --tests`: build-only, no replay-sync run — see `.github/workflows/determinism.yml`). `app` is excluded on every non-native cross target because its `bevy_audio` → `cpal` → `alsa-sys` and `bevy_winit` → `wgpu` chains need target-side system libs the cross-toolchains don't ship.

**Layer 3: Per-component checksum dump for diagnostics.** When the diff job finds a divergence, the log already says which component column differs at which frame. `scripts/diagnose_desync.sh` re-runs `replay_sync --dump-state-at <frame>` on both platforms and prints a side-by-side comparison.

Soak schedule (current; the four-tier ladder below is the eventual target):
- **Per-PR + per-main**: `ci.yml` runs `cargo nextest run --workspace --locked` (covers Layer 1's 600-frame SyncTest) + `clippy --workspace --all-targets --locked -- -D warnings`. `determinism.yml` runs Layer 2's cross-platform replay matrix on the canonical demo.
- **Nightly**: `fuzz_soak.yml` runs N seeds × 1800 frames (default 100 seeds via cron `0 4 * * *`).
- **Aspirational**: per-main 18000-frame stress demos + weekly 10hr fuzzed soak. Not currently scheduled — wire when the project has hardened enough that the existing tiers stop catching novel bugs.

Fuzzer auto-commits divergent seeds as `tests/demos/regressions/<seed>.bmrg`.

`Cargo.lock` committed. `--locked` enforced everywhere. `rust-toolchain.toml` pinned.

---
