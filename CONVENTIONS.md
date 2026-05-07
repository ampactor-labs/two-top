# Conventions

Hard rules. Violations cause subtle bugs that are expensive to find. The CI and code review enforce these.

## Determinism Invariants

- **No `f32` or `f64` in the `sim` crate.** Period. Use `Fix` and `Vec2F` from `fixed_math`. The only exception is `Vec2F::to_f32` for explicit sim→render conversion, called only in render systems.
- **No `bevy::transform`, `bevy::render`, or `glam` imports in the `sim` crate.**
- **No `Instant::now()`, `SystemTime`, or `Time<Real>` inside `GgrsSchedule`.** Use `FrameCount` resource or `Time<GgrsTime>`.
- **No `HashMap` or `HashSet` in sim code.** Use `BTreeMap`, `BTreeSet`, or `Vec<(K,V)>`. If you absolutely must use `HashMap`, configure with `ahash::AHasher` seeded with a constant.
- **No `rand::thread_rng()` or `rand::random()` in sim.** Use the `SimRng` resource (which is rolled back).
- **No `println!` or `eprintln!`.** Use `tracing` macros (`debug!`, `info!`, `warn!`, `error!`).
- **System ordering is explicit.** Every sim system uses `.before(...)` / `.after(...)` or is part of a labeled `SystemSet`. Never rely on Bevy's default scheduling.

## Component / Resource Rules

- **Every rollback component derives `Component, Clone, Copy, Hash, PartialEq, Eq, Debug`** and uses `#[require(Rollback)]`.
- **Every component on a rollback entity must be registered** with `rollback_component_with_copy::<T>()`, including markers. Unregistered markers cause silent desyncs after entity respawn.
- **Use `bevy_ggrs::checksum_hasher`, never `std::hash::DefaultHasher`.** The default hasher is not portable across machines.
- **Use `with_copy` over `with_clone`** wherever possible. Our types are `Copy`.
- **Hash components that affect gameplay** with `checksum_component_with_hash::<T>()`. Markers don't need this; their presence/absence is checked elsewhere.

## Math and Geometry

- **`length_sq` for distance comparisons, `length` only when the magnitude is needed.** Avoiding sqrt is performance and precision win. **For arena-scale comparisons (vector magnitudes potentially > 181 cm) use `length_sq_wide`** — plain `length_sq` saturates at `Fix::MAX` and silently returns wrong rankings for distant pairs.
- **Squared values use `wide_mul` internally.** `length_sq` does this; if you write your own squared-magnitude code, use `wide_mul`.
- **Always normalize angles to ±π before trig calls.** Wrap any direct trig in helper functions in `fixed_math`.
- **No `From<f32> for Fix`.** Constants only via `Fix::const_from_int(integer_literal)` (or `Fix::lit("…")` for non-integer constants). `const_fixed_from_int!` is deprecated in `fixed` 1.20.

## Render Layer Rules

- **Render systems only read sim components.** Never write to `PositionF`, `VelocityF`, etc. from a render system.
- **`Transform` is never queried in `GgrsSchedule`.** Only `sync_transforms_from_sim` writes to it, only in `Update`.
- **Animation does not interpolate.** Pixel art frames snap. The render reads `AnimState` and picks the sprite.
- **Camera, screen shake, particles, bloom, color grading, audio playback are render-only.**
- **Visual RNG is separate.** Use a different RNG for cosmetic randomness (sparks, ambient particles); never read from `SimRng`.

## Input Rules

- **Wire format is level signals only.** No `just_pressed` or `just_released` bits. Edges are derived in sim by diffing against `PreviousInputs`.
- **`PlayerInput` is exactly 4 bytes.** Don't bloat the wire format. Reserved bits are reserved.
- **Touch state is local-only.** `TouchState` is never rolled back, never serialized.
- **Quantization happens at the input boundary.** `f32` pixel coordinates → `i8`/`u8` wire format → `Fix` in sim. The sim never sees floats.
- **Movement locks during aim.** When `AIM_ACTIVE` is set, stick bytes encode aim direction and power, not movement.

## Replay and Logging

- **Strict version matching for replays.** Don't write migration code. Bump `sim_version` on any sim-affecting change.
- **Dev builds use `sim_version = u32::MAX`.** Tagged "🚧 dev replay" in the viewer.
- **`tracing` features pinned in `Cargo.toml`** with `release_max_level_info`.
- **Performance category is opt-in.** Don't leave `TRACE!` calls active in shipped builds.

## Build / Tooling

- **`Cargo.lock` is committed.**
- **`--locked` is used in CI.**
- **`rust-toolchain.toml` pins exact stable version.** Toolchain drift is itself a determinism risk.
- **Bevy is pinned with `=` to a single patch version** (e.g. `bevy = "=0.18.1"`). Do not bump until `bevy_ggrs`, `bevy_roll_safe`, and `bevy_matchbox` have all shipped releases compatible with the new Bevy. Downstream rollback crates lag main `bevy` releases — chasing latest first guarantees broken determinism CI.
- **`clippy -D warnings` is enforced** in the `ci` workflow.
- **No `unsafe` in `sim` or `fixed_math` crates** without an `// SAFETY:` comment block explaining why.
- **All public types in `sim` and `fixed_math` derive `Debug`.**

## Module Boundaries

- `fixed_math` depends on `fixed`, `cordic`, `serde`. Nothing else.
- `sim` depends on `fixed_math`, `bevy` (no `bevy_render`), `bevy_ggrs`, `bytemuck`, `serde` (and once landed: `bevy_roll_safe`, `rand_xoshiro`, `tracing`).
- `render` depends on `sim`, `bevy` (full), `bevy_audio`. No reverse dependency.
- `net` depends on `sim`, `matchbox_socket`, `bevy_ggrs`.
- `replay` depends on `sim`, `bevy` (no `bevy_render`), `bevy_ggrs`, `postcard`, `serde`. The codec (`encode`/`decode`) is pure; the `RecordPlugin` / `ReplayPlaybackPlugin` are why `bevy` and `bevy_ggrs` are direct deps.
- `app` depends on everything.
- `sync_test` depends on `sim`, `fixed_math`, `bevy` (no `bevy_render`), `bevy_ggrs`. (No `replay` dep — capture is in `replay_sync`.)
- `replay_sync` depends on `sim`, `replay`, `fixed_math`, `bevy` (no `bevy_render`), `bevy_ggrs`.
- `replay_viewer` depends on `sim`, `replay`, `render` (and Bevy+ggrs transitively).

If you find yourself wanting to break one of these rules, stop and re-read `MORGAN_NOTES.md` first.

---