# 2-Top

[![CI](https://github.com/ampactor-labs/two-top/actions/workflows/ci.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/ci.yml) [![Determinism](https://github.com/ampactor-labs/two-top/actions/workflows/determinism.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/determinism.yml) [![Fuzz Soak](https://github.com/ampactor-labs/two-top/actions/workflows/fuzz_soak.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/fuzz_soak.yml) [![APK](https://github.com/ampactor-labs/two-top/actions/workflows/apk.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/apk.yml)

A phone-vs-phone fighting game in Rust with rollback netcode over peer-to-peer WebRTC and a simulation that CI proves bit-identical on four platforms. The rules follow Boomerang Fu: throw and recall a boomerang, dash with a short invincibility window, one hit kills, first to five wins. The simulation uses fixed-point math instead of floats, which is what lets a per-frame state checksum match across platforms. Built on Bevy, the GGRS rollback library and Matchbox, it ships as an Android APK and a browser build.

**Status: working.** The two-phone test over Wi-Fi against mobile data passed at an earlier revision and has not been re-run since the netplay layer changed.

Live: https://ampactor.dev/two-top/

## Quick start

### Two Android phones

Download `two-top.apk` from the newest release at https://github.com/ampactor-labs/two-top/releases/tag/apk-latest and install it on both phones (Android asks you to allow installs from that source; [`SIDELOAD.md`](SIDELOAD.md) covers `adb install`). Tap FIND OPPONENT on both and the public room pairs them. For a private duel tap PRIVATE and dial the same four glyphs on both phones. Both phones must pick the same arena, because the pick is part of the room name.

### A browser

Open https://ampactor.dev/two-top/ in a phone or desktop browser. The deployed page has no signaling room URL baked in (the address where two peers meet), so it boots into local mode: the practice bot (PRACTICE VS BOT), couch versus on a keyboard, the replay theater, and any shared match by its link (`#watch=<id>`). On an iPhone, Share then Add to Home Screen installs it as a fullscreen icon (`web/manifest.webmanifest`).

### A desktop build, for development

Two players share one keyboard:

```sh
cargo run -p app
```

`rustup` installs the pinned toolchain (Rust 1.95.0, `rust-toolchain.toml`). On Linux the `app` crate links ALSA for audio, so install the ALSA development package first (`libasound2-dev` on Debian and Ubuntu, the package CI installs). The window boots to the Title screen with an arena picker. Player 0 moves with WASD, throws with Space and dashes with Left Shift; Player 1 uses the arrow keys, Right Shift and Right Control. Online from the desktop needs a signaling server, the small service that introduces two peers to each other ([`SIGNALING.md`](SIGNALING.md)):

```sh
cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2
```

With the Android NDK and `cargo-apk` installed, `scripts/phone.sh` builds a release APK with the public room baked in and installs it on the connected phone. [`PLAYBOOK.md`](PLAYBOOK.md) is the full ladder from a laptop couch match to two phones on different networks.

## How it works

The workspace has 13 crates. `sim` is the game: a Bevy entity-component-system (ECS) simulation that advances at 60 ticks per second and owns every piece of gameplay state. `fixed_math` gives it its numbers, `net` bridges Matchbox (the WebRTC socket crate) to GGRS, `replay` encodes match tapes, `render` draws interpolated sprites and particles from sim state, `input_touch` and `input_desktop` turn a thumb or a keyboard into the 4-byte wire input, and `app` is the Bevy application around all of it (screens, the netplay driver, identity, replays, sharing, the bot). `sync_test`, `replay_sync` and `replay_viewer` are tools; `ice_vendor` and `tape_drop` are the two small HTTP services the online build talks to.

Three terms carry the design. Determinism means the same sequence of inputs produces the same state, bit for bit, on every machine. Rollback netcode (GGRS, through `bevy_ggrs`) depends on it: each phone runs the simulation locally, predicts the other player's input when a packet has not arrived, and when the real input lands it rewinds to that frame and re-simulates forward, up to 16 frames deep online. Fixed-point math is how the simulation stays deterministic: every quantity is a Q16.16 number (a 32-bit integer with 16 fractional bits; 1 unit is 1 cm), so arithmetic is the same on every CPU, where floating point can round differently between platforms and compilers.

Each tick both players' inputs, 4 bytes each, go through GGRS into the sim; the render layer interpolates between the last two sim positions at whatever rate the display refreshes. Online, inputs travel phone to phone over a WebRTC data channel and a signaling server only pairs the peers. Every 30 ticks the peers compare a state checksum, so a divergence ends the match as a detected desync, and 9 seconds of silence from the other phone forfeits the match.

Decisions that shaped it, with the reasoning in [`MORGAN_NOTES.md`](MORGAN_NOTES.md):

- No `f32`, `f64` or `glam` in `sim`; clippy's `disallowed-types` denies the float types there. Sim collections are `BTreeMap`s, checksums use `bevy_ggrs`'s portable hasher, and gameplay randomness comes from one rolled-back RNG.
- The wire carries level signals only (stick, aim, buttons held), never press or release events. Presses are derived in the sim by diffing against the rolled-back input history, so a re-simulation cannot lose them.
- Replays are strictly version-matched: `sim::SIM_VERSION` (15 today) is stamped into every tape and rides the online room name, and any change to the simulation bumps it. There is no migration path; old tapes play on archived tagged builds.
- Online play is peer to peer, and the public APK carries no relay secret. Phones on mobile data usually sit behind carrier NAT (address sharing that blocks direct connections), which needs a TURN relay server to carry the traffic. When a build has the vendor's URL baked in, it fetches short-lived TURN credentials from `ice_vendor` at match entry, so the relay works and a leaked credential dies within hours.

### What is in the game

There are seven arenas, each built around one rule, from a neutral box with a central pyre to a forest of bone trees that burn down and stay burned for the rest of the match. Seven pickups each change the throw (Fire, Heavy, Bouncy, Curve, Multishot, Phantom, Swap); a perfect catch builds a streak, a taunt roots you in place and pays a streak tier if you finish it, and the floor crumbles in the last 8 seconds of a 30-second round. Every decided match writes a tape of about 14 KB that replays on the phone with scrubbing and playback speeds. Online there is a four-letter name over a durable install-id, a signing key that dual-signs decided matches, a per-opponent rivalry record, a consent-gated rematch, and forfeit rules that never hand a win to the survivor of a network drop. Offline there is a ten-tier practice bot, and a rival's tapes can be fitted onto it as a sparring "shade". The full description is in [`docs/GAME.md`](docs/GAME.md).

The displayed name is 2-Top. Identifiers stay textual (the `two-top` repository, the `com.ampactorlabs.twotop` Android package) because Java package segments and Rust crate names cannot start with a digit.

## Project layout

```
crates/
  fixed_math/     Q16.16 Fix and Vec2F, no floats
  sim/            the deterministic game simulation
  net/            Matchbox WebRTC to GGRS bridge, lobby state, side channel, signed results
  replay/         the .bmrg tape codec
  render/         interpolation, sprites, particles, screen shake
  input_touch/    floating virtual stick and the throw and dash touch layer
  input_desktop/  keyboard (and optional gamepad) input
  app/            the Bevy app: screens, netplay driver, identity, replays, share, bot
  sync_test/      600-frame SyncTest binary
  replay_sync/    checksum TSVs from a tape or a fuzz seed; --attest verifies signed results
  replay_viewer/  desktop tape player
  ice_vendor/     HTTP service vending short-lived TURN credentials
  tape_drop/      HTTP service holding shared tapes for about a week
assets/           generated sprites, arena floors and audio (scripts/generate_*.py)
web/              index.html, join.html, manifest and icons for the browser build
deploy/matchbox/  Dockerfile for the signaling server
tests/demos/      the canonical demo tape and its golden checksums
docs/             NORTH.md and its plan, the audits, GAME.md
```

The four documents at the root are the source of truth: `ARCHITECTURE.md` (what is built), `BUILD_PLAN.md` (the phased order, phases 0 to 18), `CONVENTIONS.md` (the hard rules) and `MORGAN_NOTES.md` (why). `PLAYBOOK.md`, `SIDELOAD.md` and `SIGNALING.md` are the operator runbooks. `docs/NORTH.md` is the finished form the four point at, with its execution record in `docs/plans/NORTH_PLAN.md`.

## Deploy

- **Web**: `.github/workflows/web.yml` builds the `app` crate for `wasm32-unknown-unknown` on every push to main, runs `wasm-bindgen` and `wasm-opt`, copies `web/` and `assets/` into the bundle and publishes it to GitHub Pages at https://ampactor.dev/two-top/. It bakes no `TWOTOP_ROOM`, so the page has no online mode.
- **APK**: `.github/workflows/apk.yml` builds a release APK with `cargo-apk` on every push to main, checks the 16 KB ELF page alignment that Android 15 phones require, prints the signing certificate, and replaces the rolling `apk-latest` release. The public APK makes direct connections only (STUN, no relay) unless the `TWOTOP_ICE_URL` repository variable points it at a credential vendor; TURN credentials are never compiled into it.
- **Services**, each built from this repository for Railway: the Matchbox signaling server (`deploy/matchbox/Dockerfile`), whose public room `wss://two-top-matchbox-production.up.railway.app/two-top?next=2` is the default that `scripts/phone.sh` and `apk.yml` bake in; `ice_vendor` (the root `Dockerfile`, health-checked at `/healthz` per `railway.json`); and `tape_drop` (`crates/tape_drop/Dockerfile`, deployed by CLI upload as `PLAYBOOK.md` describes). `SIGNALING.md` covers running your own signaling server and relay.

## Testing

```sh
cargo nextest run --workspace --locked   # or: cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
cargo run -p sync_test -- --frames 600 --check-distance 16
cargo run -p replay_sync -- --demo tests/demos/canonical/match_v1.bmrg --output checksums.tsv
cargo run -p replay_sync -- --fuzz 7 --output /dev/null
```

The workspace has 611 `#[test]` functions, counted with `grep -rho '#\[test\]' crates/ | wc -l` on 2026-09-27: 228 in `sim`, 149 in `app`, 71 in `input_touch`, the rest spread over the other crates. The `sim` integration tests drive a real GGRS `SyncTestSession`, which re-simulates every frame and panics on any divergence; `crates/sim/tests/determinism.rs` runs 600 frames at check distance 16, the live prediction window. `crates/replay_sync/tests/golden_checksums.rs` recomputes the canonical demo's 1,800 per-frame checksums and compares them byte for byte with the committed `tests/demos/canonical/match_v1.checksums.tsv`. `sync_test` is the same SyncTest as a standalone binary, `replay_sync --fuzz <seed>` runs a seeded random match under SyncTest with the arena chosen by the seed, and `scripts/diagnose_desync.sh` narrows a matrix divergence to a frame and a component.

CI runs seven workflows:

- **CI** (push and pull request): `cargo fmt --check`, `cargo check`, the nextest suite, clippy with `-D warnings` for the native target and for `wasm32-unknown-unknown`, the palette and silhouette art gates, and the Android manifest and platform-gate scripts.
- **Determinism** (push and pull request): tests the workspace for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (under qemu) and `aarch64-apple-darwin`, runs the canonical demo through `replay_sync` on each, and fails if any of the resulting checksum tables (TSV files) differs from the linux-x64 one. The `aarch64-linux-android` lane compiles the workspace and its tests only; nothing runs on Android in CI.
- **Wasm Determinism** (push and pull request): builds the game for wasm32, applies the same `wasm-opt` pass the deploy uses, and has headless Chrome recompute the 1,800 checksums against the committed golden. That is the fourth checksum lane.
- **Fuzz Soak** (nightly): 100 seeded random matches through `replay_sync --fuzz`, spread across all seven arenas, uploading any divergent seed's tape.
- **APK** and **Web** (push to main): build and publish the two artifacts described under Deploy.
- **Audit** (weekly): `cargo audit` against the pinned dependency graph.

Two things are not tested anywhere automated. Audio and haptics need a real device ([`docs/OPERATOR_CHECKLIST.md`](docs/OPERATOR_CHECKLIST.md) is the device checklist). Netplay against a real peer has no harness: `crates/net` and `crates/app` have no test directory for it, so [`docs/ROLEPLAY_AUDIT.md`](docs/ROLEPLAY_AUDIT.md) traced those mechanisms by reading them. The canonical demo runs 1,800 frames and never crosses a round boundary, so the matrix does not exercise the round-boundary code; the `sim` unit tests do.

## Limitations

The two-phone test over Wi-Fi against mobile data passed once, at an earlier revision, and nobody has re-run it since the netplay layer changed: the prediction window, the disconnect and forfeit rules, the packet guards and the browser relay path are all newer than that test. Until it runs again, cross-carrier play is a path that was verified and then drifted. Loopback on one machine and the CI matrix cover the simulation and say nothing about a real network.

- The deployed browser build has no online mode: the Pages build bakes no signaling room, so FIND OPPONENT never appears there. Nobody has played an online match from a browser build, and as of the last audit pass nobody had opened the page on a real iPhone (`docs/ROLEPLAY_AUDIT.md`, "Still unverified"); its browser-specific fixes have been verified only by the build gates and the checksum probe, with no one testing them on a phone.
- There is no native iOS app and no desktop packaging; `cargo run -p app` is the desktop build, and it is a dev tool.
- There is no in-app way to enter a room URL. Desktop uses `--room` or `MATCHBOX_ROOM`; the APK bakes `TWOTOP_ROOM` at build time.
- The public APK relays through TURN only if the credential vendor's URL was baked at build time. Without it, two phones behind carrier NAT find each other through the signaling server and then never connect (`SIGNALING.md`).
- A phone call still forfeits the match after 9 seconds of silence, with no reconnect and no wake lock.
- One ggrs assertion on the wire's `start_frame` remains reachable by a hostile peer mid-match; closing it needs a change in ggrs itself.
- Replays are strictly version-matched. A tape from another `SIM_VERSION` lists dimmed and does not play, and there is no migration path.
- Android needs Vulkan 1.1; a phone without it launches to a black screen (`PLAYBOOK.md`, Rung 3a). The sideload APK is signed with a debug key.

## Roadmap

1. Re-run the two-phone cross-carrier match on the current build. It needs two phones on different carriers and a person at each; CI cannot run it.
2. A two-peer in-process netplay harness, so the netplay mechanisms (forfeit, desync, rematch consent, the side channel) run under a test (`docs/ROLEPLAY_AUDIT.md`, "Next"). It is missing because `crates/net` and `crates/app` have no test directory and the only gate today is two processes on one machine.
3. Surviving a phone call: reconnecting inside the grace window and holding a wake lock. Today an interruption longer than 9 seconds is a forfeit.

## License

MIT OR Apache-2.0, as declared in `Cargo.toml` under `[workspace.package]`. The repository has no `LICENSE-MIT` or `LICENSE-APACHE` file yet, and the `ice_vendor` and `tape_drop` crates do not inherit the workspace field.
