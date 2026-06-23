# 2-Top

A 1v1 rollback brawler written in Rust on Bevy, `bevy_ggrs`, and Matchbox WebRTC, targeting iOS, Android, and web (PWA) in portrait orientation. The gameplay borrows from Boomerang Fu: throw-and-recall, a dash with invincibility frames, one-hit kills, 30-second rounds, best-of-N. The interesting part is underneath. Every simulation has to run bit-identically on every platform, because two phones on opposite ends of a connection must agree on the exact game state or the match desyncs. That one constraint shapes the whole codebase.

> **Naming.** The displayed name is **2-Top**. Code identifiers stay textual as `two-top` (directory), `two_top` (Rust crate), and `twotop` (Android bundle suffix), because Java package segments and Rust crate names can't start with a digit.

## What it demonstrates

A determinism-first systems project that happens to be a game.

- A custom Q16.16 fixed-point math crate (`fixed_math`) carries all simulation math. `f32`, `f64`, and `glam` are banned from the `sim` crate so results never depend on a platform's floating-point behavior. State lives in `BTreeMap` / `BTreeSet`, hashing uses a portable hasher, and gameplay randomness comes from one rolled-back RNG.
- Rollback netcode over peer-to-peer WebRTC, built on `bevy_ggrs` and Matchbox: input prediction, resimulation, desync detection, and forfeit on disconnect. Only level-signal inputs travel on the wire; edges are re-derived from rolled-back state so resimulation can't drop them.
- Determinism is verified in CI, not assumed. A SyncTest session catches single-machine nondeterminism on every job. A cross-platform replay matrix runs the same recorded match on linux-x64, linux-aarch64 under qemu, macOS on native ARM, and Android, then diffs per-frame, per-component checksums for byte-for-byte identity. Per-component logs make a desync diagnosable when one surfaces.
- Eleven crates, 371 tests, clippy-clean under `-D warnings`, a pinned toolchain, and a committed `Cargo.lock` enforced with `--locked`. Replays are strictly version-matched with no migration path, so any change that touches the simulation bumps `sim_version`.

The same concerns show up in any verifiable, competitive online system: reproducible state, agreement on shared state across a network, and fairness as a hard requirement rather than a feature.

## Design docs

The four documents at the repository root are the source of truth:

- [`ARCHITECTURE.md`](./ARCHITECTURE.md): what is being built. Tech stack, workspace layout, determinism rules, schedules, the sim/render boundary, replay format, CI strategy.
- [`BUILD_PLAN.md`](./BUILD_PLAN.md): order of operations. Eighteen phases, each with explicit produces and exit criteria.
- [`CONVENTIONS.md`](./CONVENTIONS.md): the hard rules. Determinism invariants, component-registration rules, module boundaries.
- [`MORGAN_NOTES.md`](./MORGAN_NOTES.md): the reasoning. Decision rationale and the alternatives that were rejected.

## Status

Phases 0 through 18 are complete, plus the M6 version bump, which puts the project at release-candidate. What's in the build:

- **Gameplay**: movement, dash with i-frames, the full boomerang loop (throw, ricochet, recall, catch), hits, death, respawn, first-to-5 rounds, and a deterministic input-driven rematch.
- **Modes and input**: local couch-versus on desktop keyboard, touch input on mobile, and live WebRTC netplay with P2P session swap, desync detection, and forfeit. Online is opt-in via `--room` / `MATCHBOX_ROOM`.
- **Content**: three arenas (Anchor, Crossing, Reliquary), pickups with a perfect-catch window, and six boomerang modifiers (Fire, Heavy, Bouncy, Curve, Multishot, Phantom). Sprite animation, particles, and a locked 16-color palette.
- **Feel and polish**: screen shake and a kill-cam, synthesized audio (12 cues), Android haptics, a title and lobby screen with an arena picker, and persisted settings.
- **Tooling**: a replay codec and a viewer with frame scrubbing.

371 tests pass and the cross-platform determinism matrix is green. Remaining work is tracked in [`docs/plans/COMPLETION_PLAN.md`](./docs/plans/COMPLETION_PLAN.md): desktop packaging (P.3b), the web/WASM PWA build (P.5 and P.6), and the release tag.

## Build

```sh
# Full gate:
cargo nextest run --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p sync_test -- --frames 600 --check-distance 7

# Run the game (default: local couch-versus, boots to the title/lobby screen):
cargo run -p app

# Online play is opt-in (needs a signaling server, see SIGNALING.md):
cargo run -p app -- --room <url>   # or set MATCHBOX_ROOM

# Android sideloading: see SIDELOAD.md.

# Replay viewer:
cargo run -p replay_viewer -- tests/demos/canonical/match_v1.bmrg
```
