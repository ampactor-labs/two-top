# 2-Top Completion Plan — Art Overhaul + Phases 16–18 + Netplay + Release

**Goal:** Take 2-Top from its current state (BUILD_PLAN phases 0–15 complete, 275 tests green, cross-platform determinism matrix green) to a release-candidate mobile game.

**Architecture:** Rust workspace on Bevy 0.18.1 + bevy_ggrs rollback + Matchbox WebRTC. All gameplay in the fixed-point sim crate (Q16.16 Fix/Vec2F); render layer is float-side and cosmetic-only.

---

## Milestone Map

| # | Milestone | Produces | Sim-affecting? | Status |
|---|-----------|----------|----------------|--------|
| M0 | Ground truth | Un-staled docs, verified baseline | No | ✅ done |
| M1 | Art 2.0 | New art spec + generator rewrite + AnimState v2 | Yes (AnimState) | ✅ done |
| M2 | Phase 16: arenas | BonePyre re-land, Crossing, Reliquary, arena select | Yes | ✅ done |
| M3 | Phase 17: pickups + perfect catch | 6 pickups, spawn system, perfect-catch | Yes | 🔶 3.1–3.3 done; 3.4 (art) left |
| **MP** | **PC + Web platform track** | **Desktop input, local couch versus, desktop/web builds, cross-play** | **No (input/app/build)** | ⬜ planned |
| M4 | Phase 12 completion | Real matchbox wiring, P2P swap, loopback verified | No (net/app) | ⬜ planned |
| M5 | Phase 18: polish | Shake, kill-cam, audio, haptics, UI, perf pass | Mostly render | ⬜ planned |
| M6 | Release readiness | SIM_VERSION=1, tag, operator checklist | Yes (version) | ⬜ planned |

**Sequencing note (MP):** the architecture is already cross-platform — the determinism matrix proves `x86_64-linux`, `aarch64-linux`, `aarch64-apple-darwin`, and `aarch64-linux-android` are bit-identical, and the fixed-point sim sidesteps the WASM float-nondeterminism problem outright. **MP's local-couch-versus piece (P.1 + P.2) is independent of M4 netplay** and is the fastest path to playtesting with friends on one PC, so it is prioritized to run right after M3. Online PC play (P.4) and Web (P.5–P.6) depend on M4/M5 landing first.

---

## Milestone 0 — Ground Truth

- [x] Task 0.1: Baseline verification — 275 tests pass, clippy clean, sync_test exit 0
- [x] Task 0.2: Un-stale docs — CLAUDE.md, README.md, BUILD_PLAN.md updated, plan copied

## Milestone 1 — Art 2.0

- [x] Task 1.1: Rewrite ART_DIRECTION.md to v2
- [x] Task 1.2: AnimState v2 in sim (+RUN, +CATCH)
- [x] Task 1.3: Generator infrastructure + both player sheets
- [x] Task 1.4: Wire the new atlas contract
- [x] Task 1.5: 🛑 OPERATOR VISUAL GATE — direction signed off
- [x] Task 1.6: Remaining asset production

## Milestone 2 — Phase 16: Arenas

- [x] Task 2.1: Re-land arena infrastructure (BonePyre)
- [x] Task 2.2: Pyre art + shatter feedback
- [x] Task 2.3: Crossing arena
- [x] Task 2.4: Reliquary arena
- [x] Task 2.5: Arena selection + replay wiring

## Milestone 3 — Phase 17: Pickups + Perfect Catch

- [x] Task 3.1: Perfect catch mechanic
- [x] Task 3.2: Pickup plumbing
- [x] Task 3.3: Six modifier behaviors (Fire/Heavy/Bouncy/Curve/Multishot/Phantom) — fuzz-clean across all arenas
- [ ] Task 3.4: Pickup presentation (icons, HUD chip, per-kind boomerang tint)

## Milestone P — PC + Web Platform Track

Why this is cheap: the sim is deterministic in fixed-point, so it already runs bit-identically off-phone; the only per-platform work is *input*, *windowing*, and *build pipeline*. Each task: test-first where sim-adjacent, full gate, commit.

- [ ] Task P.1: **Desktop input source** — new `input_desktop` crate mirroring `input_touch`'s `ReadInputs` shape. Reads keyboard + gamepad (`bevy::input`), maps each device to a player handle, emits per-handle `LocalInputs<GgrsCfg>` in the existing 4-byte wire format (reuse `quantize`/deadzone; **never** add edge bits to the wire — level signals only, per CONVENTIONS). Pure mapping fns unit-tested Bevy-free.
- [ ] Task P.2: **Local couch versus (one PC, no network)** — feed *distinct* per-handle inputs (P0 = keyboard or pad A, P1 = pad B) into a 2-local-player session. Decide session type: the current `SyncTestSession` feeds both handles identically (a determinism harness, not playable) — switch the desktop play path to a local session that accepts independent inputs per handle. **Fastest "play with a friend" path; zero dependency on M4.** Verify: two input devices drive two duelists independently; a kill scores; round flow runs.
- [ ] Task P.3: **Desktop windowing + packaging** — resizable/fullscreen window; landscape/desktop camera + HUD scale (the layout is portrait-phone today); on-screen control-scheme hint; release artifacts for Linux/macOS/Windows. Add `x86_64-pc-windows-msvc` to the determinism matrix.
- [ ] Task P.4: **Desktop online netplay** — after M4 lands, the same Matchbox driver gives remote PC-vs-PC. Verify two desktop windows + local signaling server complete a match desync-free (extends M4's loopback gate).
- [ ] Task P.5: **Web / WASM (PWA)** — `wasm32-unknown-unknown` target, trunk/wasm-bindgen build, asset loading, web-audio path, mouse+touch+keyboard input, PWA manifest + service worker. Matchbox is browser-native, so netplay carries over unchanged. Add a wasm build (and, if feasible headless, a wasm sim-checksum check) to CI.
- [ ] Task P.6: **Cross-play verification** — assert the WASM sim checksums match the native determinism matrix (fixed-point should already guarantee it), then document phone ↔ PC ↔ browser cross-play as a supported configuration.

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
