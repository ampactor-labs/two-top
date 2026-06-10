# 2-Top Completion Plan — Art Overhaul + Phases 16–18 + Netplay + Release

**Goal:** Take 2-Top from its current state (BUILD_PLAN phases 0–15 complete, 275 tests green, cross-platform determinism matrix green) to a release-candidate mobile game.

**Architecture:** Rust workspace on Bevy 0.18.1 + bevy_ggrs rollback + Matchbox WebRTC. All gameplay in the fixed-point sim crate (Q16.16 Fix/Vec2F); render layer is float-side and cosmetic-only.

---

## Milestone Map

| # | Milestone | Produces | Sim-affecting? |
|---|-----------|----------|----------------|
| M0 | Ground truth | Un-staled docs, verified baseline | No |
| M1 | Art 2.0 | New art spec + generator rewrite + AnimState v2 | Yes (AnimState) |
| M2 | Phase 16: arenas | BonePyre re-land, Crossing, Reliquary, arena select | Yes |
| M3 | Phase 17: pickups + perfect catch | 6 pickups, spawn system, perfect-catch | Yes |
| M4 | Phase 12 completion | Real matchbox wiring, P2P swap, loopback verified | No (net/app) |
| M5 | Phase 18: polish | Shake, kill-cam, audio, haptics, UI, perf pass | Mostly render |
| M6 | Release readiness | SIM_VERSION=1, tag, operator checklist | Yes (version) |

---

## Milestone 0 — Ground Truth

- [x] Task 0.1: Baseline verification — 275 tests pass, clippy clean, sync_test exit 0
- [x] Task 0.2: Un-stale docs — CLAUDE.md, README.md, BUILD_PLAN.md updated, plan copied

## Milestone 1 — Art 2.0

- [ ] Task 1.1: Rewrite ART_DIRECTION.md to v2
- [ ] Task 1.2: AnimState v2 in sim (+RUN, +CATCH)
- [ ] Task 1.3: Generator infrastructure + both player sheets
- [ ] Task 1.4: Wire the new atlas contract
- [ ] Task 1.5: 🛑 OPERATOR VISUAL GATE
- [ ] Task 1.6: Remaining asset production

## Milestone 2 — Phase 16: Arenas

- [ ] Task 2.1: Re-land arena infrastructure (BonePyre)
- [ ] Task 2.2: Pyre art + shatter feedback
- [ ] Task 2.3: Crossing arena
- [ ] Task 2.4: Reliquary arena
- [ ] Task 2.5: Arena selection + replay wiring

## Milestone 3 — Phase 17: Pickups + Perfect Catch

- [ ] Task 3.1: Perfect catch mechanic
- [ ] Task 3.2: Pickup plumbing
- [ ] Task 3.3: Six modifier behaviors
- [ ] Task 3.4: Pickup presentation

## Milestone 4 — Phase 12 Completion: Real Netplay

- [ ] Task 4.1: Matchbox driver in app
- [ ] Task 4.2: Desktop loopback verification

## Milestone 5 — Phase 18: Game Feel, Audio, UI

- [ ] Task 5.1: Screen shake + kill flash
- [ ] Task 5.2: Kill-cam beat
- [ ] Task 5.3: Synthesized audio + wiring
- [ ] Task 5.4: Haptics (Android)
- [ ] Task 5.5: Match summary + settings
- [ ] Task 5.6: Performance pass

## Milestone 6 — Release Readiness

- [ ] Task 6.1: Version + demos + tag
- [ ] Task 6.2: Operator checklist
