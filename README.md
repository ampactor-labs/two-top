# 2-Top

[![CI](https://github.com/ampactor-labs/two-top/actions/workflows/ci.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/ci.yml)
[![Determinism](https://github.com/ampactor-labs/two-top/actions/workflows/determinism.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/determinism.yml)
[![Fuzz Soak](https://github.com/ampactor-labs/two-top/actions/workflows/fuzz_soak.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/fuzz_soak.yml)
[![APK](https://github.com/ampactor-labs/two-top/actions/workflows/apk.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/apk.yml)

A phone-vs-phone rollback brawler written in Rust on Bevy, `bevy_ggrs`, and
Matchbox WebRTC. The gameplay borrows from Boomerang Fu: throw-and-recall,
a dash with invincibility frames, one-hit kills, 30-second rounds, first to
five. The interesting part is underneath. Every simulation tick has to
produce bit-identical state on every platform, because two phones on
opposite ends of a connection either agree exactly or the match desyncs.
That one constraint shapes the whole codebase.

Android is the product platform today; iOS and a web PWA are on the
roadmap. The desktop build is a dev tool: couch versus on one keyboard,
plus the capture and loopback harnesses.

> **Naming.** The displayed name is **2-Top**. Code identifiers stay
> textual as `two-top` (directory), `two_top` (Rust crate), and `twotop`
> (Android bundle suffix), because Java package segments and Rust crate
> names can't start with a digit.

## Play it

Every push to main builds a sideloadable APK. The newest one is always
here:

```
https://github.com/ampactor-labs/two-top/releases/download/apk-latest/two-top.apk
```

Install it on two Android phones ([`SIDELOAD.md`](./SIDELOAD.md)), tap
FIND OPPONENT on both, and the public room pairs you. Dial the same
four-glyph code on both phones for a private duel. The public build
carries no relay secrets: it fetches throwaway TURN credentials from a
small credential service at match entry, so cross-carrier matches relay
through Cloudflare and a leaked credential dies within hours.

## What it demonstrates

A determinism-first systems project that happens to be a game.

- A custom Q16.16 fixed-point math crate (`fixed_math`) carries all
  simulation math. `f32`, `f64`, and `glam` are banned from the `sim`
  crate so results never depend on a platform's floating-point behavior.
  State lives in `BTreeMap` / `BTreeSet`, hashing uses a portable hasher,
  and gameplay randomness comes from one rolled-back RNG.
- Rollback netcode over peer-to-peer WebRTC, built on `bevy_ggrs` and
  Matchbox: input prediction, resimulation, desync detection every 30
  ticks, and forfeit on disconnect. Only level-signal inputs travel on the
  wire; edges are re-derived from rolled-back state so resimulation can't
  drop them.
- Determinism is verified in CI, not assumed. A SyncTest session catches
  single-machine nondeterminism on every job. A cross-platform replay
  matrix runs the same recorded match on linux-x64, linux-aarch64 under
  qemu, macOS on native ARM, and Android, then diffs per-frame,
  per-component checksums for byte-for-byte identity. A nightly fuzz soak
  replays randomized matches across all seven arenas, because the walk-only
  golden checksum structurally cannot catch a boomerang-clash desync.
- Twelve crates, 498 tests, clippy-clean under `-D warnings`, a pinned
  toolchain, and a committed `Cargo.lock` enforced with `--locked`.
  Replays are strictly version-matched with no migration path, so any
  change that touches the simulation bumps `sim_version` (currently 11).

The same problems turn up in any competitive online system that has to
stay honest: reproducing state exactly, and getting two machines to agree
on it across a network.

## What's in the game

Seven arenas, each built around one rule. Anchor is the neutral box with a
central pyre. Crossing has a blood chasm and a bridge you raise by hitting
an altar sigil. Reliquary has teleporter doors and chain-linked pyres. The
Pit is walled in: no void, and the boundary ricochets your fang back into
play. The Vigil never shrinks; a round with no kill just expires. The
Gallery is a corridor maze. The Forest is twelve bone trees that block
movement, fall after two hits, and burn: one fire fang can take a whole
cluster, and the sightlines it opens stay open for the rest of the match.

Seven pickup modifiers (Fire, Heavy, Bouncy, Curve, Multishot, Phantom,
Swap), each telegraphed by a colored halo on the floor that matches the
tint the fang will fly with. Perfect catches build a speed ladder. A taunt
roots you in public view and pays a streak tier if you survive it.
Rounds end in a sudden-death floor crumble where the storm exists, and
every decided match writes a replay tape you can watch on the phone,
through the live presentation, with scrubbing and playback speeds.

Online has an identity layer: a dialed four-glyph name, a per-opponent
rivalry record ("4TH MEETING, you lead 2-1"), a consent-gated RUN IT BACK
rematch, and honest forfeit blame: whoever walked away owns the loss, and
quitting a live duel from the in-match QUIT chip is scored the same way.
Offline there is a practice gauntlet against a bot that sharpens every
time you beat it and resets when it beats you.

## Design docs

The four documents at the repository root are the source of truth:

- [`ARCHITECTURE.md`](./ARCHITECTURE.md): what is being built. Tech stack,
  workspace layout, determinism rules, schedules, the sim/render boundary,
  replay format, CI strategy.
- [`BUILD_PLAN.md`](./BUILD_PLAN.md): order of operations. Eighteen
  phases, each with explicit produces and exit criteria.
- [`CONVENTIONS.md`](./CONVENTIONS.md): the hard rules. Determinism
  invariants, component-registration rules, module boundaries.
- [`MORGAN_NOTES.md`](./MORGAN_NOTES.md): the reasoning. Decision
  rationale and the alternatives that were rejected.

[`PLAYBOOK.md`](./PLAYBOOK.md) is the operator's ladder from a laptop
couch match to two phones on different carriers. [`SIGNALING.md`](./SIGNALING.md)
covers the signaling server and NAT traversal, including the ephemeral
TURN credential service in `crates/ice_vendor`.

## Status

The full gameplay loop, netplay, seven arenas, the social layer, the
on-device replay theater, and the CI/APK pipelines are built and green.
Open items: the two-phone cross-carrier field test (the relay path is
deployed and verified server-side; the parking-lot test remains), desktop
packaging, and the web/WASM PWA milestone, which also means adding wasm32
to the determinism matrix.

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

# Build + install the phone build (needs the Android NDK, see SIDELOAD.md):
scripts/phone.sh

# Replay viewer (desktop; phones use the built-in REPLAYS screen):
cargo run -p replay_viewer -- tests/demos/canonical/match_v1.bmrg
```
