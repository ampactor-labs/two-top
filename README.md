# 2-Top

[![CI](https://github.com/ampactor-labs/two-top/actions/workflows/ci.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/ci.yml)
[![Determinism](https://github.com/ampactor-labs/two-top/actions/workflows/determinism.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/determinism.yml)
[![Fuzz Soak](https://github.com/ampactor-labs/two-top/actions/workflows/fuzz_soak.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/fuzz_soak.yml)
[![APK](https://github.com/ampactor-labs/two-top/actions/workflows/apk.yml/badge.svg)](https://github.com/ampactor-labs/two-top/actions/workflows/apk.yml)

A phone-versus-phone fighting game for Android, written in Rust on Bevy with rollback netcode over peer-to-peer WebRTC. Rollback means each phone predicts the opponent's input and rewinds to correct its guess when the real input arrives. Both phones must then compute bit-identical state, so the simulation uses Q16.16 fixed-point math, and CI compares per-frame checksums of a recorded match on four targets. The gameplay borrows from Boomerang Fu: a boomerang you throw and recall, a dash that dodges hits, one-hit kills and first to five.

**Status: working.** CI publishes the APK and the browser build on every push to main, but the two-phone match across mobile carriers last passed at an earlier revision and the browser build has no online play.

Live: https://ampactor.dev/two-top/ · Package: https://github.com/ampactor-labs/two-top/releases/tag/apk-latest

## Quick start

### On two Android phones

1. On both phones, download the newest APK, which CI rebuilds on every push to main:
   https://github.com/ampactor-labs/two-top/releases/download/apk-latest/two-top.apk
2. Allow installs from the app you downloaded it with, then open the APK. [`SIDELOAD.md`](SIDELOAD.md) covers installing and the Android toolchain.
3. Pick the same arena on both phones, because the arena is part of the room name. Then tap FIND OPPONENT on both; the public signaling room pairs them and the match starts.

For a private match, switch the toggle on the Title screen to PRIVATE and dial the same four-glyph code on both phones. With one phone, tap PRACTICE and then PLAY to fight the bot. [`docs/GAME.md`](docs/GAME.md) lists the arenas, pickups and modes.

### In a browser

Open [ampactor.dev/two-top](https://ampactor.dev/two-top/). It is the same `app` crate compiled to WebAssembly with WebGL2, with touch controls on a phone and the keyboard on a laptop. It offers practice against the bot, couch play on a keyboard and replays. It keeps its profile, settings, rivalry record, room code and tapes in `localStorage`. On an iPhone, Share and then Add to Home Screen gives it an icon and a fullscreen launch. A shared match opens straight from its link (`#watch=<id>`).

### From source, on a desktop

Install Rust with rustup; it reads the pinned 1.95.0 toolchain from `rust-toolchain.toml`. On Debian or Ubuntu, install `libasound2-dev` first, as CI does.

```sh
cargo run -p app
```

The window opens on the Title screen for couch play on one keyboard. One player uses WASD, Space to throw and Left Shift to dash; the other uses the arrow keys, Right Shift and Right Ctrl. The desktop build is a development tool. [`PLAYBOOK.md`](PLAYBOOK.md) walks from this step to two phones on different networks, including a local signaling server and `scripts/phone.sh`, which builds and installs the APK on a USB-connected phone.

## Usage

The workspace also builds these developer tools.

```sh
# Watch a tape in a desktop window (phones use the REPLAYS screen)
cargo run -p replay_viewer -- tests/demos/canonical/match_v1.bmrg

# Re-simulate a tape and write its per-frame, per-component checksums
cargo run -p replay_sync -- --demo tests/demos/canonical/match_v1.bmrg --output checksums.tsv

# Check a signed match result against its tape
cargo run -p replay_sync -- --demo <match>.bmrg --attest <match>.attest.json

# Run the rollback checker over 600 frames at the live 16-frame prediction depth
cargo run -p sync_test -- --frames 600 --check-distance 16
```

For the canonical tape, `checksums.tsv` must equal `tests/demos/canonical/match_v1.checksums.tsv` byte for byte. `replay_sync --fuzz <seed>` generates a random match from a seed, and `--dump-state-at <frame>` prints the simulation state at one frame. `scripts/diagnose_desync.sh <a.tsv> <b.tsv>` prints the first frame and column where two checksum files differ. Setting `TWOTOP_CAPTURE=<file.png>` makes the desktop app save one screenshot and exit, so a visual change can be checked without watching the window.

## How it works

Every frame, `input_touch` or `input_desktop` turns the controls into a 4-byte `PlayerInput` (stick x, stick y, aim angle, buttons). GGRS, the rollback library, sends it to the other phone, predicts the input it has not received yet and runs the `sim` systems for the frame. When a real input differs from the prediction, GGRS restores a saved snapshot and re-simulates the frames since. `render` then draws the latest state, interpolating between simulation frames.

- **Fixed-point math.** Floating-point results can differ between CPUs, compilers and instruction sets, and one differing bit ends the match in a desync. The `sim` crate therefore bans `f32`, `f64` and `glam`, and does all math in `fixed_math` (Q16.16, on the `fixed` crate). It also uses ordered maps (`BTreeMap`) only, a portable hasher and one rolled-back random number generator.
- **Level signals on the wire.** An input carries only which buttons are held on that frame. The simulation derives presses by comparing it with the previous frame's rolled-back input, so re-simulation reproduces them.
- **Strict replay versions.** A tape records `SIM_VERSION` (currently 15) and plays only in a build with the same version. There are no migrations, so any change to the simulation bumps it.

Online, `net` connects two peers through Matchbox: a signaling server introduces them, then inputs travel directly over a WebRTC data channel. The live session predicts up to 16 frames, delays local input by 3, compares checksums every 30 frames and forfeits after 9 seconds without contact. A second, reliable channel carries names, rematch consent and a goodbye. After a match decided on score, both phones sign the same result with ed25519 keys, and `replay_sync --attest` verifies it later. The room name includes the arena and `SIM_VERSION`, so only matching builds on the same arena meet. Carrier networks often put phones behind NAT (address translation) that blocks a direct connection; the match then needs a TURN relay server that both phones can reach. The app fetches short-lived relay credentials (4 hours by default) from the `ice_vendor` service when a match starts, so no relay secret ships in the APK. `ice_vendor` can proxy Cloudflare's TURN service or sign credentials for a self-hosted coturn server. [`SIGNALING.md`](SIGNALING.md) covers the signaling and relay setup.

### Design documents

- [`ARCHITECTURE.md`](ARCHITECTURE.md): the tech stack, workspace layout, determinism rules, schedules, replay format and CI strategy.
- [`BUILD_PLAN.md`](BUILD_PLAN.md): the 18 build phases, each with what it produces and its exit criteria.
- [`CONVENTIONS.md`](CONVENTIONS.md): the hard rules, such as determinism invariants and module boundaries.
- [`MORGAN_NOTES.md`](MORGAN_NOTES.md): the reasons behind each decision and the alternatives I rejected.
- [`docs/NORTH.md`](docs/NORTH.md): the finished form of the game, with the execution plan in [`docs/plans/NORTH_PLAN.md`](docs/plans/NORTH_PLAN.md).

## Project layout

```
crates/
  fixed_math/     Q16.16 numbers and vectors, the only math the sim uses
  sim/            game rules as Bevy systems, rolled back by GGRS
  input_touch/    touch controls to PlayerInput
  input_desktop/  keyboard and gamepad to PlayerInput
  net/            Matchbox-to-GGRS bridge, side channel, signed results
  render/         sprite drawing, effects, interpolation of sim state
  app/            the game for Android, desktop and the browser
  replay/         the .bmrg tape format
  replay_sync/    headless re-simulation, checksums, fuzzing, --attest
  replay_viewer/  desktop tape viewer
  sync_test/      standalone rollback checker
  ice_vendor/     service that issues short-lived TURN credentials
  tape_drop/      service that stores shared tapes for watch links
tests/demos/canonical/  the canonical tape and its committed checksums
web/            browser page, join page, PWA manifest and icons
scripts/        phone build, CI gates, asset generators, desync diagnosis
deploy/matchbox/  Dockerfile for the signaling server
```

The displayed name is 2-Top. Identifiers spell it `two-top` (the directory), `two_top` (Rust) and `twotop` (the Android package suffix), because Rust crate names and Java package segments cannot start with a digit.

## Deploy

- **Android.** `apk.yml` runs on every push to main. It builds a release APK for `aarch64-linux-android` with cargo-apk, bakes in the public signaling room plus the relay-credential, tape-drop and watch-page URLs from repository variables, checks the 16 KB ELF page alignment that Android 15 needs, prints the signing certificate, and replaces the `apk-latest` prerelease. The APK is signed with a debug key.
- **Browser.** `web.yml` runs on every push to main. It builds `app` for `wasm32-unknown-unknown`, runs wasm-bindgen and `wasm-opt`, and deploys to GitHub Pages at https://ampactor.dev/two-top/.
- **Services.** The Matchbox signaling server (`deploy/matchbox/Dockerfile`), `ice_vendor` (root `Dockerfile`) and `tape_drop` (`crates/tape_drop/Dockerfile`) run on Railway. No workflow deploys them; [`PLAYBOOK.md`](PLAYBOOK.md) and [`SIGNALING.md`](SIGNALING.md) have the steps.

## Testing

```sh
cargo fmt --all --check
cargo nextest run --workspace --locked   # or: cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 scripts/check_android_manifest.py
python3 scripts/check_platform_gates.py
python3 scripts/check_palette.py      # needs Pillow
python3 scripts/check_silhouettes.py  # needs Pillow
```

The workspace has 611 `#[test]` functions, counted with `grep -rE '^\s*#\[test\]' crates`. The largest groups are `sim` (228, one file per mechanic in `crates/sim/tests`), `app` (149) and `input_touch` (71). `fixed_math` has property tests. `replay_sync` tests the canonical tape against its committed checksums, the verification of signed results and the fuzzer. In 19 of the 23 sim test files, the game runs inside a SyncTest session, in which GGRS rolls back and re-simulates recent frames on every step and fails on any checksum mismatch; the 600-frame SyncTest in `determinism.rs` rolls back 16 frames, the live prediction window.

| Workflow         | Trigger                    | What it checks                                                                                                                                                                                                  |
| ---------------- | -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CI               | push and PR to main        | fmt, `cargo check`, nextest, clippy on all targets, clippy of `app` for wasm32, the palette and silhouette gates, the Android manifest and platform-gate checks                                                |
| Determinism      | push and PR to main        | nextest and the canonical replay on Linux x86-64, Linux ARM64 (under QEMU) and macOS ARM64, then a byte diff of the three checksum files; the ARM lanes skip `app` and `ice_vendor`, and the Android lane only compiles the tests |
| Wasm Determinism | push and PR to main        | the canonical replay inside headless Chrome, which must print `CHECKSUMS-OK 1800 frames`                                                                                                                       |
| Fuzz Soak        | nightly, or by hand        | 100 seeded random matches through SyncTest, spread over all seven arenas                                                                                                                                       |
| APK, Web         | push to main               | the release builds (see Deploy)                                                                                                                                                                                |
| Audit            | weekly                     | `cargo audit` against RustSec advisories                                                                                                                                                                       |

Not tested: no test runs two peers against each other, so forfeits, rematch consent and the side channel are unit-tested in pieces and checked end to end by hand ([`PLAYBOOK.md`](PLAYBOOK.md), rungs 2 to 5). The canonical tape runs 1800 frames on one arena, with both players sending the same inputs; it ends before the first round boundary and never taunts. Nothing runs on a real phone in CI.

## Limitations

The online path is the least proven part. Two phones on different networks (Wi-Fi against mobile data) passed the field test once, at an earlier revision, and the netplay code has changed a lot since without that test being repeated. The browser build has no online play, because its deploy workflow bakes in no signaling room, so an iPhone can practise and watch replays but cannot duel. The repository records no test of the browser build on a real iPhone.

- If a phone goes quiet for 9 seconds (a phone call, or switching apps), its player forfeits. There is no reconnect.
- A hostile peer can still crash the other player's app through an assertion inside `ggrs` that checks a frame number the remote sends. The fix belongs in `ggrs` itself.
- `tape_drop` runs on `tiny_http` 0.12, which cannot set a read deadline, so a slow client can hold a request thread.
- There is no native iOS app and no Play Store listing. The APK is a debug-signed sideload build.
- The desktop build is laid out for a portrait phone and has no packaged release for Linux, macOS or Windows.
- A tape plays only in a build with the same `SIM_VERSION`. Older tapes need the matching tagged build.

[`docs/ROLEPLAY_AUDIT.md`](docs/ROLEPLAY_AUDIT.md) records each open finding with its evidence.

## Roadmap

1. Repeat the two-phone match across carriers on the current build. It needs two phones on different networks and a person at each, so it cannot run in CI.
2. Online play in the browser: bake the signaling room and relay-credential URLs into `web.yml`, then play a browser against a phone. Neither URL is set in that workflow, and browser netplay has never been verified.
3. A two-peer test harness in one process, so forfeit, rematch and side-channel handling run in CI. It does not exist yet; the audit lists it as the next piece of test infrastructure.

## License

The workspace manifest (`Cargo.toml`) declares `MIT OR Apache-2.0`, and 11 of the 13 crates inherit it; `ice_vendor` and `tape_drop` declare no license. The repository has no LICENSE files yet.
