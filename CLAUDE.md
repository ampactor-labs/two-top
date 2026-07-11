# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository status

Phases 0–18 substantially complete. The workspace has 11 crates (`fixed_math`, `sim`, `render`, `input_touch`, `input_desktop`, `net`, `replay`, `replay_sync`, `sync_test`, `replay_viewer`, `app`) with 466 passing tests, clippy-clean, and cross-platform determinism matrix green. Full gameplay is implemented: movement, dash + i-frames, boomerang throw/ricochet/recall/catch, hits/death/respawn, round flow (first-to-5), replay codec + viewer with scrub, touch input, **desktop keyboard input (dev/testing only)**, **live Matchbox WebRTC netplay (P2P session swap, desync detection, forfeit — loopback-verified)**, sprite animation + particles + 16-color palette, the **art 2.0 overhaul** (3-direction sprite atlas: side/back/front), **three arenas (Anchor/Crossing/Reliquary)**, **pickups + perfect catch (all six modifier behaviors)**, the **game-feel layer** (screen shake, kill flash, kill-cam, synthesized audio, Android haptics, deterministic rematch), the **depth-duel camera** (WORLD_TILT_Y=0.75 tabletop perspective, Y-axis spawns, per-client PerspectiveFlip for online), a Title/lobby screen with arena picker, persisted JSON settings, the **social layer** (reliable side-channel: install-id + dialed CURSTAG names, per-opponent grudge ledger, consent-gated RUN IT BACK rematch, clean leave, away-grace with honest forfeit blame), **TURN relay config** for carrier-NAT traversal, the **on-device replay theater** (REPLAYS screen: list, play, scrub, speed through the live presentation), the **gauntlet** (persistent practice-bot ladder), the pip-slam score beat + victory pose, and a **southpaw** touch layout. `sim::SIM_VERSION` is `9`. The product is a **phone-vs-phone online fighter** — each player runs the APK on their Android device, taps Play, and the signaling server pairs them for a P2P match. The desktop build (`cargo run -p app`) is a dev/testing tool (local couch-versus with split keyboard). See `PLAYBOOK.md` for the end-to-end phone setup.

The displayed name is **2-Top**. Identifiers stay textual because Java package segments and Rust crate names can't start with a digit: the repo directory is `two-top` (hyphen), Rust crate names use `two_top` (underscore), and the Android bundle suffix is `twotop`. Treat "2-Top" as the user-facing name (READMEs, Android app label, marketing) and `two-top`/`two_top`/`twotop` as identifiers (paths, imports, manifests).

## Canonical docs — read order

These four files are the source of truth. Read all of them before proposing any non-trivial change. They are tightly cross-referenced and contradicting them silently is the failure mode to avoid.

- **`ARCHITECTURE.md`** — what is being built. Project overview, tech stack, workspace layout, determinism rules, fixed-math API, component model, schedules, sim/render boundary, input wire format, replay format, CI strategy.
- **`BUILD_PLAN.md`** — order of operations. 18 phases from empty workspace to polish. Each phase has explicit produces/exit-criteria gates. Do not skip phases or merge them; phase boundaries exist to keep the cross-platform determinism guarantee verifiable at every step.
- **`CONVENTIONS.md`** — hard rules. Determinism invariants, component/resource registration rules, math conventions, render-layer rules, input rules, module boundaries. Treat as enforced; CI will eventually fail any violation.
- **`MORGAN_NOTES.md`** — the *why*. Decision rationale (Q16.16 over Q32.32, 1cm units, interpolation over extrapolation, postcard over bincode, level-signal-only inputs, etc.) and rejected alternatives. Consult before re-litigating any design choice.

## Project shape (one-paragraph summary)

**2-Top** is a 1v1 mobile rollback brawler (iOS/Android/web-PWA) in Rust on Bevy + `bevy_ggrs` + Matchbox WebRTC. Boomerang Fu mechanics: throw-and-recall, dash with i-frames, one-hit kills, 30s rounds, best-of-N. Fairness, determinism, and frame-stable visuals are non-negotiable. The whole architecture exists to make cross-platform bit-identical simulation provable, not just hopeful.

## Load-bearing invariants (the things easiest to get wrong)

`CONVENTIONS.md` is the full list. These are the ones whose violations cause the most expensive, hardest-to-detect bugs:

1. **No `f32` / `f64` / `glam` / `bevy::transform` / `bevy::render` in the `sim` crate.** All sim math is `Fix` (`I16F16`) and `Vec2F` from `fixed_math`. The only sim→render bridge is `Vec2F::to_f32`, called only from render systems.
2. **Every component on a rollback entity must be registered with `rollback_component_with_copy::<T>()`, including markers.** Unregistered markers cause silent desyncs after entity respawn — the worst kind of bug, because it surfaces hours later in soak tests.
3. **Sim system ordering is explicit.** Every system has `.before()` / `.after()` or a labeled `SystemSet`. Never rely on Bevy's default scheduling inside `GgrsSchedule`.
4. **Wire-format inputs are level signals only.** Edges (`just_pressed` etc.) are derived in sim by diffing against the rolled-back `PreviousInputs`, never sent on the wire — otherwise rollback resimulation loses them.
5. **`SimRng` (rolled back) for gameplay randomness; a separate non-rolled-back RNG for cosmetics.** Never `thread_rng()` / `rand::random()` in sim.
6. **`BTreeMap` / `BTreeSet` / `Vec<(K,V)>` only in sim — no `HashMap` with random hasher.** Same reason: random hasher state is non-portable.
7. **`bevy_ggrs::checksum_hasher`, never `std::hash::DefaultHasher`** (not portable).
8. **Strict replay version matching, no migrations.** Bump `sim_version` on any sim-affecting change. Old replays viewed via archived git-tagged binaries.

When tempted to break one of these, re-read `MORGAN_NOTES.md` first — most of the rationale lives there.

## Determinism is verified by CI, not local runs

Three-layer defense (`ARCHITECTURE.md` § CI Strategy):

1. `SyncTestSession` (`crates/sync_test`) — every CI job, `check_distance: 7`, catches single-machine non-determinism.
2. **Cross-platform replay matrix** — `replay_sync` runs canonical demos headlessly on linux-x64, linux-aarch64 (qemu), macos-14 (native ARM), aarch64-linux-android. All must produce byte-identical per-frame per-component checksum TSVs. This is the gate that matters.
3. Per-component checksum logs + `scripts/diagnose_desync.sh` for diagnosis.

A change that passes locally on one platform but hasn't been through the cross-platform matrix is *unverified*. Don't claim determinism work is done until the matrix has run.

## Common commands (forward-looking)

These are the commands `BUILD_PLAN.md` defines for each phase. They will be the daily-driver commands once Phase 0 lands. Listed here so future sessions don't have to rediscover them.

```bash
# Workspace gate (Phase 0):
cargo check --workspace --locked

# Per-crate tests (Phase 1+):
cargo nextest run -p fixed_math
cargo nextest run -p sim
# etc.

# Full lint gate (CI-equivalent):
cargo clippy --workspace --locked -- -D warnings

# SyncTest harness (Phase 3+):
cargo run -p sync_test -- --frames 600 --check-distance 7

# Cross-platform replay determinism (Phase 5+):
cargo run -p replay_sync -- --demo tests/demos/canonical/match_v1.bmrg --output checksums.tsv
cargo run -p replay_sync -- --dump-state-at <frame> --demo <path>

# Fuzzed soak (Phase 6+):
cargo run -p replay_sync -- --fuzz <seed>
```

`Cargo.lock` is committed and `--locked` is enforced everywhere. `rust-toolchain.toml` pins the exact stable version — toolchain drift is itself a determinism risk, so don't loosen the pin without reason.

## Operator runbooks

- **`PLAYBOOK.md`** — the primary hands-on reference. Covers the full ladder from laptop couch-versus to phone-vs-phone cross-network play, including signaling server setup, APK build, sideloading, and verification gates.
- **`SIDELOAD.md`** — detailed Android toolchain setup (NDK, SDK, cargo-apk prerequisites).
- **`SIGNALING.md`** — deep dive on the matchbox signaling protocol, server options, and debug helpers.

## Tooling conventions

This repo is opted in to the AI-assisted dev protocol (`~/.claude/protocol/PROTOCOL.md`). Per-repo state lives in `.protocol/`, git hooks (commit-msg + pre-commit) are active. Disclosure conventions are in `CONTRIBUTING.md`; the PR self-review checklist is at `.github/PR_SELF_REVIEW.md`.

## Phased-build discipline

`BUILD_PLAN.md` is sequenced for a reason: each phase's exit criteria depend on the prior phase's verifiable artifacts. The cross-platform determinism guarantee is built incrementally and would be much harder to retrofit. Don't propose work that crosses phase boundaries (e.g., starting boomerang physics in Phase 9 before Phase 7's sim/render boundary lands) without an explicit case for why the order should change.

When in doubt about which phase a piece of work belongs to, check `BUILD_PLAN.md` § Produces / Exit criteria for each phase — work tends to belong to whichever phase first lists the artifact under "Produces."
