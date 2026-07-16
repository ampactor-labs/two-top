# 2-Top testing playbook

The single hands-on reference for testing 2-Top across your devices. Work
top to bottom: each rung proves something the next rung depends on, so when
something breaks you know it was introduced by the step you just did.

This doc is self-contained for the common flows. The deeper "why" lives in
[`SIDELOAD.md`](./SIDELOAD.md) (Android toolchain) and
[`SIGNALING.md`](./SIGNALING.md) (netplay transport); you only need them if a
step here misbehaves.

> **Your kit.** One Linux laptop (Fedora) as the dev box/signaling broker, and
> two Android phones as the players. Commands prefixed with nothing run on the
> **laptop**; phone actions are called out explicitly.

---

## The ladder at a glance

| Rung | What you prove | Needs a phone? | Needs the network? |
| --- | --- | --- | --- |
| 0 | The game runs (couch versus on the laptop) | no | no |
| 1 | The build is healthy (tests + lint + determinism) | no | no |
| 2 | The netplay stack works (loopback, two processes) | no | local only |
| 3 | The game runs **on the phone** (sideload, solo touch) | yes | no |
| 4 | **Phone vs phone, same Wi-Fi** | yes | LAN |
| 5 | **Two devices on different networks** | yes | public internet |

Rungs 0–3 work today with no caveats. Rung 4 is the first real cross-device
match and the recommended milestone. Rung 5 is the hard one; do it last.

---

## Rung 0 — laptop couch versus

The fastest "is it alive" check. Two players share one keyboard.

```sh
cargo run -p app
```

- Boots to the **Title / lobby** screen with an arena picker. Seven arenas
  (2026-07-16 roster): Anchor, Crossing, Reliquary, **The Pit** (walled-in —
  no void, the boundary ricochets fangs, no storm), **The Vigil** (the storm
  never comes; a killless round expires scoreless), **The Gallery** (dense
  corridor maze), **The Forest** (bone trees block and ricochet; two chips
  fell one, a Heavy fells in one — and FIRE spreads tree to tree, burning
  the cover down for the rest of the match). Keys 1-3 pick the classics;
  tapping/cycling reaches all seven. Online rooms hash across the roster.
- Start a match. **Player 0 = WASD**, **Player 1 = arrow keys**.
- Throw is the boomerang button (see the on-screen prompts); dash gives
  i-frames; one hit kills; first to 5 wins the match.
- `F11` toggles borderless fullscreen (handy on a TV).

**Check:** movement, dash, throw → ricochet → recall → catch, a kill +
respawn, the round timer, and the score climbing to 5 → match-over. After
match-over, pressing **Throw** restarts (the input-driven rematch).

---

## Rung 1 — build health (run before any device session)

If these are green, the simulation is deterministic on this machine and the
code is CI-clean. Run them after every pull or code change.

```sh
# Tests across the whole workspace
cargo nextest run --workspace --locked

# Lint gate (this is the exact CI gate — note --all-targets)
cargo clippy --workspace --all-targets --locked -- -D warnings

# Single-machine determinism (SyncTest, 600 frames, rollback distance 7)
cargo run -p sync_test -- --frames 600 --check-distance 7
```

**Check:** all tests pass, clippy emits nothing, and SyncTest finishes with
no `SyncTestMismatch`. A mismatch means a real determinism violation, not a
flake. Cross-platform determinism is a CI-only gate (linux-x64/aarch64,
macOS-ARM, Android); you can't reproduce the full matrix locally, and that's
expected.

---

## Rung 2 — loopback netplay (no phone yet)

Proves the WebRTC + signaling + rollback path works on your box before you
add the variables of a second device and a real network. This is two app
processes talking through a local signaling server.

You need the matchbox signaling server once:

```sh
cargo install matchbox_server
```

Then three terminals:

```sh
# Terminal 1 — signaling server (brokers the handshake only)
matchbox_server --host 0.0.0.0 --port 3536

# Terminal 2 — peer A
cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2

# Terminal 3 — peer B
cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2
```

The `?next=2` pairs peers two at a time. The room name (`two-top`) namespaces
your traffic.

**What changes online:** an online build boots to the **Title screen** like
local — tap Play to connect. The connection to the signaling server starts
when you tap Play, not at app launch. Arena selection works on the Title
screen for both modes:

```sh
cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2
```

**Check (top-right yellow lobby overlay):** after tapping Play, both windows
cycle `connecting → waiting peer → connected`, then play in lockstep with
**zero desync**. Kill one process: the survivor forfeits (lobby shows
`FORFEIT`, match ends) within a few seconds.

If you want to watch the handshake:

```sh
RUST_LOG=two_top::net=debug,matchbox_socket=debug \
  cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2
```

---

## Rung 3 — get the game onto the phone (one-time setup + solo run)

This is the biggest one-time cost. Do the toolchain once; after that, install
is a single command. Full detail in [`SIDELOAD.md`](./SIDELOAD.md); the
condensed path:

### 3a. Check the phone *can* run it (before investing in the toolchain)

2-Top renders through Vulkan. An old phone may not qualify, and you'd rather
know now. With the phone connected and `adb` working (see 3c):

```sh
adb shell pm list features | grep vulkan
```

You want to see `android.hardware.vulkan.version` (Vulkan 1.1+). If there's
no Vulkan feature line, the app will launch to a **black screen** and this
phone can't be Player 2.

### 3b. Host toolchain (one-time)

```sh
# Rust target for arm64 Android
rustup target add aarch64-linux-android

# Android SDK command-line tools + build-tools + platform + NDK
# (lean, no Android Studio — see SIDELOAD.md §3 for the full sdkmanager recipe)
export ANDROID_HOME="$HOME/Android/Sdk"
export ANDROID_NDK_ROOT="$ANDROID_HOME/ndk/26.3.11579264"

# The APK packager
cargo install cargo-apk
```

Persist `ANDROID_HOME`, `ANDROID_NDK_ROOT`, and `$ANDROID_HOME/platform-tools`
on `PATH` in your `~/.zshrc` so you don't re-export each session.

### 3c. Phone in developer mode

1. **Settings → About phone → tap "Build number" seven times.**
2. **Settings → System → Developer options → enable USB debugging.**
3. Plug into the laptop with a **data-capable** USB cable.
4. Accept the **"Allow USB debugging from this computer?"** prompt (check
   "Always allow").
5. Samsung-specific: if install later fails with `INSTALL_FAILED_USER_RESTRICTED`,
   enable **Settings → Apps → Special access → Install unknown apps** for your
   file manager / the ADB source.

Confirm the host sees the phone (status must read `device`, not
`unauthorized`/`offline`):

```sh
adb devices
```

### 3d. Build, install, run (solo)

```sh
cargo apk run -p app --lib --target aarch64-linux-android
```

This cross-compiles, packages, installs, and launches. A launcher icon
labeled **2-Top** appears on the phone.

**Check on the phone:** the Title screen renders (sprites not blank), audio
plays, touch controls move a player, and a throw/kill **buzzes** (haptics
need `VIBRATE` granted — it's in the manifest — and haptics enabled in the
in-game settings). A blank or silent build means the asset bundle didn't
package; a black screen means the Vulkan check in 3a failed.

Logs (the run command doesn't pipe stdout back):

```sh
adb logcat --pid=$(adb shell pidof com.ampactorlabs.twotop)
```

---

## Rung 4 — two phones, same Wi-Fi (the milestone)

Both phones are on the same home Wi-Fi. The laptop only runs the signaling
server; it is not a player. Both phones join the same room by the laptop's LAN
address, then the WebRTC datachannel goes phone-to-phone. On one subnet, this
usually connects directly on host candidates, so no STUN/TURN is needed.

### 4a. Find the laptop's LAN IP

```sh
hostname -I | awk '{print $1}'      # quick: first address
# or, to see the Wi-Fi interface explicitly:
ip -4 addr show
```

Call this `<LAPTOP_IP>` below (e.g. `192.168.1.42`).

### 4b. Open the signaling port through the firewall

Fedora's firewalld blocks inbound by default, so the phones can't reach the
signaling server until you open it. For a test session:

```sh
sudo firewall-cmd --add-port=3536/tcp
```

This is **not** persistent (no `--permanent`), so a reboot or
`firewall-cmd --reload` reverts it. That's deliberate for a test box.

Only add the broader trusted-interface rule if the **laptop itself** is one of
the peers in a laptop-vs-phone test, because then WebRTC UDP also has to reach
the laptop:

```sh
# Optional laptop-peer rule; replace wlan0 with your Wi-Fi interface.
sudo firewall-cmd --zone=trusted --add-interface=wlan0
```

That rule is unnecessary for the normal two-phone setup.

### 4c. Start the signaling server (laptop)

```sh
matchbox_server --host 0.0.0.0 --port 3536
```

### 4d. Bake the room URL into one APK

Phones launched from the Android icon have no command line and no settable env
var, so the room URL is baked in at **build time** via `TWOTOP_ROOM`. For a
small, optimized playtest APK, sign the release build with your local Android
debug keystore:

```sh
CARGO_APK_RELEASE_KEYSTORE="$HOME/.android/debug.keystore" \
CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android \
TWOTOP_ROOM=ws://<LAPTOP_IP>:3536/two-top?next=2 \
  cargo apk build -p app --lib --target aarch64-linux-android --release
```

If `~/.android/debug.keystore` does not exist yet, run
`cargo apk build -p app --lib --target aarch64-linux-android` once; cargo-apk
will generate it. That debug APK is huge, so use the release APK below for
sharing.

The APK is:

```sh
target/release/apk/app.apk
```

Install it on each phone. With USB debugging, plug in one phone at a time and
run:

```sh
adb install -r target/release/apk/app.apk
```

For the second phone, either repeat the ADB install or send that same
`app.apk` via Drive/Bluetooth/email/USB transfer and tap it on the phone.

### 4e. Launch both phones

Both phones boot to the Title screen with **"TAP TO FIND OPPONENT"**. Pick the
arena first if you want (tap the top area to cycle arenas), then both players
tap the bottom half to connect.

### 4f. What to verify (Phase 12, Gate 1)

- Both phone lobby overlays reach `connected` within ~2 seconds.
- A 3-2-1 countdown starts on **both** screens together.
- Move, throw, recall, kill, respawn — all in lockstep on both screens.
- Score and match state stay identical on both devices for the full round
  and any subsequent rounds, ending naturally at first-to-5 → match-over.
- **Rematch:** after match-over, **either** player pressing Throw restarts
  both devices together (score back to 0-0, fresh countdown, no desync).
  Note: the couch-only "ESC back to lobby" does not apply online.

### 4g. Resilience gates (optional, same setup)

- **Gate 2 (blip):** toggle airplane mode on the phone for ~1.5s. Both sides
  show `reconnecting…`, then resume bit-identically when connectivity returns
  within the ~3s forfeit window.
- **Gate 3 (forfeit):** kill connectivity for 5+ seconds. The survivor goes
  `connected → reconnecting… → FORFEIT` and the match ends. Forfeit is
  terminal by design — the dropped peer can't rejoin that match.

If anything desyncs visibly, grab the `.bmrg.log` files from both devices and
run `scripts/diagnose_desync.sh` on the per-frame checksum logs.

---

## Rung 5 — two devices on different networks (the hard one)

This is real internet netplay, and it is materially harder than Rung 4 for
two reasons. Do it only after Rung 4 is solid.

1. **The signaling server must be publicly reachable.** Your laptop behind
   home NAT is not. Either run `matchbox_server` on a small public VPS
   (1 CPU / 512 MB is plenty) or expose the local one through a tunnel
   (`cloudflared` / `ngrok`). Front it with TLS (caddy/nginx → `wss://`) if
   you tunnel. See [`SIGNALING.md`](./SIGNALING.md) § "Self-hosted on a VPS".

2. **The datachannel needs NAT traversal.** Carrier mobile data is usually
   behind CGNAT, often symmetric, which plain STUN can't punch. You will very
   likely need a **TURN** relay (self-hosted `coturn`, or a hosted TURN
   provider) in addition to STUN.

Once the server is public at, say, `wss://signal.example.com`, the device
config is the same shape as Rung 4: laptop joins with
`--room wss://signal.example.com/two-top?next=2`, and the phone APK is built
with `TWOTOP_ROOM=wss://signal.example.com/two-top?next=2`. Use the same
debug-keystore-signed release flow as Rung 4 for local sideload playtests.
Verification is the same Gate 1–3 checklist as Rung 4.

> TURN/STUN wiring for the WebRTC layer is not yet a configurable build knob
> in `crates/net`. Expect this rung to need a code change before it works; it
> is genuinely future work, not a one-command setup.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| `adb: no devices/emulators found` | USB debugging off, or charge-only cable | Re-check the dev-mode toggle, accept the host fingerprint, swap cable |
| `INSTALL_FAILED_USER_RESTRICTED` | Samsung blocks sideload | Settings → Apps → Special access → Install unknown apps → enable the source |
| App launches to a **black screen** | No Vulkan-capable GPU | `adb shell pm list features \| grep vulkan` — pre-2018 devices may not qualify |
| App installs but is **silent / untextured** | Asset bundle not packaged | Confirm `assets = "../../assets"` in `[package.metadata.android]` |
| App **immediately exits** | Rust panic | `adb logcat` — any `SyncTestMismatch` is a real determinism bug, not a build issue |
| Throw/kill **don't vibrate** | `VIBRATE` missing or haptics off | `VIBRATE` is declared; enable haptics in the in-game settings |
| Lobby stuck at `connecting` (Rung 4) | Phone can't reach the signaling server | Open `3536/tcp` in firewalld; check both devices are on the same Wi-Fi; some routers have "AP isolation" — disable it |
| Lobby reaches `connected` then immediately drops, or stuck at `waiting peer` | Only one peer joined, or mismatched room URL | Both sides must use the **exact** same `ws://<LAPTOP_IP>:3536/two-top?next=2` |
| Connects on LAN but **never** on cellular (Rung 5) | NAT traversal failing | You need STUN and almost certainly TURN; see Rung 5 notes |
| Game runs but they **desync** | Determinism violation | Save both `.bmrg.log`, run `scripts/diagnose_desync.sh` |

---

## Quick command reference

```sh
# Laptop couch versus
cargo run -p app

# Build health (run before device sessions)
cargo nextest run --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p sync_test -- --frames 600 --check-distance 7

# Signaling server
matchbox_server --host 0.0.0.0 --port 3536

# Laptop joins a room (optional laptop peer / loopback testing)
cargo run -p app -- --room ws://<HOST>:3536/two-top?next=2

# Phone: install solo
cargo apk run -p app --lib --target aarch64-linux-android

# Phone: install pointed at a room (room URL baked at build time)
TWOTOP_ROOM=ws://<HOST>:3536/two-top?next=2 \
  cargo apk run -p app --lib --target aarch64-linux-android

# Phone APK to share/install on two phones
CARGO_APK_RELEASE_KEYSTORE="$HOME/.android/debug.keystore" \
CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android \
TWOTOP_ROOM=ws://<HOST>:3536/two-top?next=2 \
  cargo apk build -p app --lib --target aarch64-linux-android --release

# Laptop LAN IP
hostname -I | awk '{print $1}'

# Open firewall for the laptop-hosted signaling server (non-persistent)
sudo firewall-cmd --add-port=3536/tcp

# Optional only when the laptop is also a WebRTC peer
sudo firewall-cmd --zone=trusted --add-interface=wlan0

# Phone logs
adb logcat --pid=$(adb shell pidof com.ampactorlabs.twotop)

# Verbose netplay handshake (desktop)
RUST_LOG=two_top::net=debug,matchbox_socket=debug cargo run -p app -- --room <URL>

# Vulkan capability check
adb shell pm list features | grep vulkan
```

---

## Passing the APK to a friend

Build a signed release APK with the signaling URL baked in:

```sh
CARGO_APK_RELEASE_KEYSTORE="$HOME/.android/debug.keystore" \
CARGO_APK_RELEASE_KEYSTORE_PASSWORD=android \
TWOTOP_ROOM=ws://<HOST>:3536/two-top?next=2 \
  cargo apk build -p app --lib --target aarch64-linux-android --release
```

The APK is at `target/release/apk/app.apk`. Send it via Google Drive,
Bluetooth, email, USB transfer — whatever's easiest. Your friend enables
"Install unknown apps" for the source app in their Android settings, taps the
APK to install, and launches. Both players tap Play on the Title screen → the
signaling server pairs you → you fight.

This uses your local debug keystore to sign a release build for sideload
playtesting. For store distribution, use a real upload/release key instead.

For same-wifi: the signaling server runs on your machine (Rung 4 setup).
For cross-network: deploy `matchbox_server` to a public host (see Rung 5).

## Solo practice, private rooms, replays, identity

What every install carries, no server or second phone needed:

**The gauntlet (practice vs the bot).** Tap PRACTICE VS BOT on the Title
(P on desktop), then PLAY. The match runs the normal local session with the
bot supplying player 2's inputs — it keeps range, plants visibly before it
throws, dashes through your fangs, and steers its recalls. Beat it and your
GAUNTLET TIER climbs (the button label carries the number); every tier the
bot opens sharper — harder throws, earlier dodges, tighter aim, up to a
beatable ceiling. Lose once and the tier resets to zero (best tier is
remembered in `career.json`). A fresh install's bot starts as a passive
dummy and sharpens one notch per kill you land, so the first match is the
tutorial. Practice results never touch the online career W-L.
`TWOTOP_PRACTICE=1` env arms it for desktop automation.

**Private rooms.** On the online Title, the glyph row under the menu dials a
room code: tap a glyph to cycle it (letters of CUR + STAG, 2401 combinations),
tap QUICK to go back to public matchmaking. Two phones dialed to the same four
glyphs only ever meet each other. The code persists across launches. Desktop:
keys 1-4 cycle the slots, 0 resets to quick.

**Your name.** The online Title's NAME row dials a 4-glyph name from the
same alphabet (desktop: keys 5-8). A fresh install gets one dealt from its
install identity, so every phone is named before its owner touches the pad.
The name rides the identity handshake to your opponent: it shows on their
summary, in their grudge ledger ("4TH MEETING with TAGC — you lead 2-1"),
and in the replay tape header.

**Match replays + the theater.** Every decided match writes a `.bmrg` input
tape: `~/Downloads/two-top/replays/` on desktop,
`Android/data/<pkg>/files/replays/` on the phone (reachable with any Files
app). The file is a few KB and reproduces the whole match bit-for-bit.
Watch them ON THE DEVICE: the REPLAYS button on the Title lists the saved
tapes (V on desktop); tap one and it plays back through the live game's own
presentation — HUD, kill-cam, audio, the works — because the deterministic
sim just replays the inputs. Tap to pause, drag the bottom strip to scrub,
tap a speed (0.5x-4x), tap the top edge to exit. A tape from another sim
version is honestly absent from the list rather than desyncing; keep the
tagged binary around for old tapes. (Desktop `replay_viewer` still works.)

**Online etiquette, enforced.** A finished online match offers RUN IT BACK:
your THROW press asks for the rematch (the opponent's summary shows who's
in) and the match restarts only when both sides consent. A top-band tap (or
Esc) LEAVES cleanly — the opponent's screen flips to `<NAME> FLED`
immediately instead of waiting out the grace. If a phone goes away
mid-match (call, notification shade), the other side sees `<NAME> AWAY`
and the match survives interruptions up to ~9 s; past that it forfeits,
the survivor records the win, and the leaver's own screen says MATCH
ABANDONED.

**Quitting mid-match.** The QUIT chip lives in the top-right corner during
live play (top-left for southpaw; Esc on desktop). First tap arms it
(`SURE?`), a second tap within 2.5 s quits. Walking out of a live online
duel records the loss on your ledger — same honesty as the away-grace
forfeit. While waiting alone in an online room the chip reads CANCEL and
needs no confirmation; it's also the way out of a stuck SUMMONING wait.

Settings (haptics, sfx, music, deadzone, southpaw) are the five tappable
rows on the Title — left half of a row lowers/toggles, right half raises.
**Southpaw** mirrors the whole touch layout left-for-right: move stick on
the right half, throw on the left, dash bottom-LEFT.

## Known gaps (so you don't chase non-bugs)

- **No in-app way to enter a room URL.** The lobby overlay is read-only.
  Desktop uses `--room`/`MATCHBOX_ROOM`; the phone uses the compile-time
  `TWOTOP_ROOM` bake (Rung 4d). An in-app lobby text field is future work.
  (Private ROOMS within the baked server are in — the glyph pad above.)
- **STUN-only traversal fails behind carrier NAT.** For Rung 5
  cross-network play through restrictive carrier NATs, bake a TURN relay:
  `TWOTOP_TURN_URL` / `TWOTOP_TURN_USER` / `TWOTOP_TURN_PASS` (see
  SIGNALING.md § NAT traversal). All unset ⇒ STUN-only, as before.
- **Cross-platform determinism is CI-only.** You verify single-machine
  determinism locally (SyncTest); the four-platform byte-identical matrix
  runs in CI.
- **Replay tapes survive frame hitches now.** The recorder captures both
  players' inputs per sim tick inside the rollback schedule (corrected on
  resimulation), so match-start shader-compile stalls no longer poison the
  tape — every decided match saves its `.bmrg`. The old harvest design
  lost the whole tape to any >8-tick hitch, which on desktop was nearly
  every autostart match.
- **Taunt is live on every input path.** Touch: tap the top strip of the
  screen (top 24%, minus the QUIT corner top-right — mirrored for
  southpaw). Desktop: `T` (P0) / `Enter` (P1); gamepad: North. The
  flex roots you for 0.7 s, cancels on dash or throw with no reward, and
  completing it feeds the perfect-catch streak one tier. The bot taunts
  your corpse once it has sharpened up — punish it on your next life.
