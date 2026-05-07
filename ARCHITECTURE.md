# Architecture

## Project Overview

`two-top` is a 1v1 mobile rollback brawler in portrait orientation. Mechanics are derived from Boomerang Fu: each player has one thrown-and-recalled boomerang, dash with i-frames, one-hit kills, 60-second rounds, best-of-N format. The aesthetic is demonic-Duke pixel art executed with Hyper Light Drifter discipline.

Targets: iOS, Android, and web (PWA over WebRTC). Same Rust codebase across all three.

Networking is GGRS-style rollback peer-to-peer with WebRTC transport via Matchbox. Fairness, determinism, and frame-stable visuals are non-negotiable.

## Tech Stack

| Layer | Crate / Tool |
|---|---|
| Engine | `bevy` |
| Rollback netcode | `bevy_ggrs` (built on `ggrs`) |
| Transport | `matchbox_socket` + signaling server |
| Fixed-point math | `fixed` (Q16.16 via `FixedI32<U16>`) |
| Trig / sqrt | `cordic` |
| Deterministic RNG | `rand_xoshiro` (`Xoshiro256StarStar`) |
| Rollback states | `bevy_roll_safe` (states only — not audio) |
| Serialization | `postcard` |
| Logging | `tracing` + `tracing-subscriber` |
| Property testing | `proptest` |
| Cross-target build | `cross` / `taiki-e/setup-cross-toolchain-action` |

## Workspace Layout
(Use two_top for Rust crate names since hyphens aren't valid there.)

```
two-top/
├── Cargo.toml                     # workspace root
├── rust-toolchain.toml            # pinned stable version
├── crates/
│   ├── fixed_math/                # Q16.16 vocabulary, Vec2F, trig
│   ├── sim/                       # deterministic simulation; no bevy_render
│   ├── net/                       # matchbox + GGRS session plumbing
│   ├── replay/                    # postcard codec, demo loader, playback
│   ├── render/                    # sprites, particles, transform sync, audio
│   ├── input_touch/               # touch -> PlayerInput quantization
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

## Determinism Rules

The seven sources of non-determinism and how each is eliminated:

| Source | Rule |
|---|---|
| Floating-point | No `f32`/`f64` in `sim` crate. Fixed-point (`Fix`, `Vec2F`) only. |
| System execution order | All sim systems explicitly ordered with `.before()`/`.after()` in `GgrsSchedule`. |
| Entity iteration order | Queries that depend on order sort by `RollbackId` first. |
| PRNG | One seeded `Xoshiro256StarStar` resource, rolled back. Visual RNG is separate, not rolled back. |
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

    pub fn length_sq(self) -> Fix;       // wide_mul internally
    pub fn length(self) -> Fix;          // cordic::sqrt
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

**No `From<f32> for Fix`.** Constants must be built at compile time via `const_fixed_from_int!` or `Fix::from_num(integer_literal)`.

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
- `AngleF(Fix)`
- `PreviousAngleF(Fix)`

Identity / structure:
- `Player { handle: usize }`
- `Boomerang { owner: usize, state: BoomerangState }`
- `Wall { kind: WallKind }`

State:
- `AnimState { anim_id: u8, frame: u16 }`
- `StunFrames(u16)` — drives hit-stop and i-frames
- `InputHistory { buffer: [PlayerInput; 8], head: u8 }`

Markers:
- `NoInterpolate` (render-side flag, but rolled back if on a sim entity)

Rolled-back resources:
- `FrameCount(u32)`
- `PreviousInputs { per_player: [PlayerInput; 2] }`
- `SimRng(Xoshiro256StarStar)`
- `MatchState` (via `bevy_roll_safe::init_ggrs_state`)
- `MatchScore { p0: u8, p1: u8 }`

Registration in app setup:
```rust
app
    .rollback_component_with_copy::<PositionF>()
    .rollback_component_with_copy::<PreviousPositionF>()
    .rollback_component_with_copy::<VelocityF>()
    .rollback_component_with_copy::<AngleF>()
    .rollback_component_with_copy::<PreviousAngleF>()
    .rollback_component_with_copy::<Player>()
    .rollback_component_with_copy::<Boomerang>()
    .rollback_component_with_copy::<Wall>()
    .rollback_component_with_copy::<AnimState>()
    .rollback_component_with_copy::<StunFrames>()
    .rollback_component_with_copy::<InputHistory>()
    .rollback_component_with_copy::<NoInterpolate>()

    .rollback_resource_with_copy::<FrameCount>()
    .rollback_resource_with_copy::<PreviousInputs>()
    .rollback_resource_with_copy::<SimRng>()
    .rollback_resource_with_copy::<MatchScore>()

    .checksum_component_with_hash::<PositionF>()
    .checksum_component_with_hash::<VelocityF>()
    .checksum_component_with_hash::<AngleF>()
    .checksum_component_with_hash::<AnimState>()
    .checksum_component_with_hash::<StunFrames>()
    .checksum_component_with_hash::<MatchScore>();
```

**Every component on a rollback entity must be registered, including markers.** Unregistered markers cause silent desyncs after entity respawn.

## Schedules

`GgrsSchedule` (rollback, deterministic) — explicit ordering required:

1. `snapshot_previous` — copy `PositionF` → `PreviousPositionF`, `AngleF` → `PreviousAngleF`
2. `input_diff` — derive edges from `(CurrentInputs, PreviousInputs)`, push to `InputHistory`
3. `match_state_advance` — countdown timers, round transitions
4. `player_movement` — apply movement input to `VelocityF` and `PositionF`
5. `boomerang_physics` — boomerang flight, ricochet, recall
6. `collision` — players vs walls, boomerangs vs walls, boomerangs vs players
7. `combat` — apply hits, stun, scoring, respawn
8. `animation_advance` — increment `AnimState.frame`, transition states
9. `audio_tag` — spawn `RollbackSound` entities for events that just occurred
10. `commit_inputs` — write `CurrentInputs` → `PreviousInputs` for next tick
11. `record_last_tick_time` — runs in `AdvanceWorldSystems::Last`, writes `LastSimTickTime`

`Update` (render, f32 allowed):

- `sync_transforms_from_sim` — interpolated `Transform` from `PositionF`/`PreviousPositionF`
- `camera_follow` — exponential damping toward midpoint of player Transforms
- `screen_shake` — read sim events via change detection, drive camera offset
- `particles_update` — CPU-side particle simulation (render-only RNG)
- `audio_reconcile` — read `RollbackSound` entities, play via `bevy_audio`
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

**Audio:** sim spawns `RollbackSound { kind: SoundKind, frame: u32 }` entities (rolled back). Render-side `audio_reconcile` reads which entities exist and plays them via `bevy_audio`. Entities removed by rollback never play. We do not use `bevy_roll_safe`'s audio plugin.

**Particles:** CPU-side custom system. `Particle { position: Vec2, velocity: Vec2, lifetime: f32, color: Color }`. Spawned by render-side systems reading sim events. Capped at ~500 active.

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

**Level signals only — no edges in wire format.** Edges derived in sim by diffing against `PreviousInputs`.

**While `AIM_ACTIVE` is set, `stick_x`/`stick_y` encode aim drag direction and power.** Movement is locked. Tip-magnitude is power, direction is angle. The `aim_angle` byte is redundant when AIM_ACTIVE but kept for fast lookup and for the sticky-on-release frame.

**Touch layer (`PreUpdate`, not rolled back):**
- `TouchState` resource updated from Bevy's `Touches`
- Floating virtual stick: first touch in lower-left quadrant becomes the stick, origin at first-touch position
- Radial deadzone curve: 12% inner deadzone, 75% saturation
- Three throw interactions:
  - **Tap** (touch + release within ~150ms, drag distance < 20px): instant throw in facing direction at default power
  - **Hold + drag**: enters aim mode, `AIM_ACTIVE` set, stick bytes encode aim
  - **Release while aiming**: throw fires with last-frame aim values; `aim_angle` is sticky for one frame across release

**Forgiveness buffer:**
- `InputHistory` component, ring buffer of 8 frames (~133ms)
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
    pub sim_version: u32,           // u32::MAX in dev builds
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

Strict version matching: replays only load if `sim_version` matches binary. Old replays viewed via archived git-tagged binaries.

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

**Layer 1: SyncTestSession.** Single-machine. Runs every CI job. `check_distance: 7`, `input_delay: 0`. Catches intra-machine non-determinism.

**Layer 2: Cross-platform replay matrix.** `replay_sync` binary runs `canonical_match.bin` headlessly on each platform, dumps per-frame per-component checksum log. Diff job compares all logs against linux-x64 baseline. Platforms: linux-x64, linux-aarch64 (qemu), macos-14 (native ARM), aarch64-linux-android.

**Layer 3: Per-component checksum dump for diagnostics.** When the diff job finds a divergence, the log already says which component column differs at which frame. `scripts/diagnose_desync.sh` re-runs `replay_sync --dump-state-at <frame>` on both platforms and prints a side-by-side comparison.

Soak schedule:
- **PR**: 30s canonical demo (1,800 frames)
- **Main**: 5min canonical + stress demos (18,000 frames)
- **Nightly**: 1hr fuzzed inputs, 100 seeds
- **Weekly**: 10hr fuzzed soak

Fuzzer auto-commits divergent seeds as `tests/demos/regressions/<seed>.bmrg`.

`Cargo.lock` committed. `--locked` enforced everywhere. `rust-toolchain.toml` pinned.

---