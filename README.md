# 2-Top

A 1v1 mobile rollback brawler in Rust on Bevy + bevy_ggrs + Matchbox WebRTC. Portrait orientation, iOS / Android / web (PWA). Boomerang Fu mechanics with provable cross-platform deterministic simulation.

> **Naming:** the displayed name is **2-Top**. The repo and codebase identifiers stay as `two-top` (directory) / `two_top` (Rust crate) / `twotop` (Android bundle suffix) because Java package segments and Rust crate names can't start with a digit.

## Design docs

The four documents in repo root are the source of truth:

- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — what is being built (tech stack, workspace layout, determinism rules, schedules, sim/render boundary, replay format, CI strategy).
- [`BUILD_PLAN.md`](./BUILD_PLAN.md) — order of operations (18 phases, each with explicit produces / exit criteria).
- [`CONVENTIONS.md`](./CONVENTIONS.md) — hard rules (determinism invariants, component-registration rules, module boundaries).
- [`MORGAN_NOTES.md`](./MORGAN_NOTES.md) — the *why* (decision rationale, rejected alternatives).

## Status

Phases 0–15 complete — full gameplay (movement, dash, boomerang throw/ricochet/recall/catch, hits/death/respawn, round flow first-to-5), replay codec + viewer with scrub, touch input, net crate with lobby FSM, sprite animation + particles + 16-color palette. 275 tests green, cross-platform determinism matrix green. See `docs/plans/COMPLETION_PLAN.md` for remaining work (art overhaul, arenas, pickups, netplay wiring, polish, release).

## Build

```sh
# Full gate:
cargo nextest run --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p sync_test -- --frames 600 --check-distance 7

# Run the game:
cargo run -p app

# Run the replay viewer:
cargo run -p replay_viewer -- tests/demos/canonical/match_v1.bmrg
```
