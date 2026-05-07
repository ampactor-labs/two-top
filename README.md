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

Phase 0 — workspace skeleton. No gameplay yet.

## Build

```sh
cargo check --workspace --locked
```
