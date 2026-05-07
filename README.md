# two-top

A 1v1 mobile rollback brawler in Rust on Bevy + bevy_ggrs + Matchbox WebRTC. Portrait orientation, iOS / Android / web (PWA). Boomerang Fu mechanics with provable cross-platform deterministic simulation.

## Design docs

The four documents in repo root are the source of truth:

- [`ARCHITECTURE.md`](./ARCHITECTURE.md) — what is being built (tech stack, workspace layout, determinism rules, schedules, sim/render boundary, replay format, CI strategy).
- [`BUILD_PLAN.md`](./BUILD_PLAN.md) — order of operations (18 phases, each with explicit produces / exit criteria).
- [`CONVENTIONS.md`](./CONVENTIONS.md) — hard rules (determinism invariants, component-registration rules, module boundaries).
- [`MORGAN_NOTES.md`](./MORGAN_NOTES.md) — the *why* (decision rationale, rejected alternatives).

## Status

Phase 0 — workspace skeleton. No gameplay yet.

## Build

```sh
cargo check --workspace --locked
```
