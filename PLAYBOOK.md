# 2-Top testing playbook

The single hands-on reference for testing 2-Top across your devices. Work
top to bottom: each rung proves something the next rung depends on, so when
something breaks you know it was introduced by the step you just did.

This doc is self-contained for the common flows. The deeper "why" lives in
[`SIDELOAD.md`](./SIDELOAD.md) (Android toolchain) and
[`SIGNALING.md`](./SIGNALING.md) (netplay transport); you only need them if a
step here misbehaves.

> **Your kit.** One Linux laptop (Fedora) that doubles as the dev box and
> Player 1, and one old Samsung Android phone as Player 2. Commands prefixed
> with nothing run on the **laptop**; phone actions are called out explicitly.

---

## The ladder at a glance

| Rung | What you prove | Needs a phone? | Needs the network? |
| --- | --- | --- | --- |
| 0 | The game runs (couch versus on the laptop) | no | no |
| 1 | The build is healthy (tests + lint + determinism) | no | no |
| 2 | The netplay stack works (loopback, two processes) | no | local only |
| 3 | The game runs **on the phone** (sideload, solo touch) | yes | no |
| 4 | **Laptop vs phone, same Wi-Fi** | yes | LAN |
| 5 | **Phone on cellular vs laptop on Wi-Fi** | yes | public internet |

Rungs 0–3 work today with no caveats. Rung 4 is the first real cross-device
match and the recommended milestone. Rung 5 is the hard one; do it last.

---

## Rung 0 — laptop couch versus

The fastest "is it alive" check. Two players share one keyboard.

```sh
cargo run -p app
```

- Boots to the **Title / lobby** screen with an arena picker.
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

**What changes online:** an online build **skips the Title screen** and boots
straight into the match. Arena selection online is via an env var, since the
picker lives on the Title screen the online build never shows:

```sh
TWOTOP_ARENA=crossing cargo run -p app -- --room ws://127.0.0.1:3536/two-top?next=2
# values: anchor (default) | crossing | reliquary
```

**Check (top-right yellow lobby overlay):** both windows cycle
`idle → connecting → waiting peer → connected`, then play in lockstep with
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
cargo apk run -p app --target aarch64-linux-android
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

## Rung 4 — laptop vs phone, same Wi-Fi (the milestone)

Both devices on the same home Wi-Fi. The laptop runs the signaling server;
the phone and the laptop both join the same room by the laptop's LAN address.
On one subnet, WebRTC usually connects directly on host candidates, so no
STUN/TURN is needed.

### 4a. Find the laptop's LAN IP

```sh
hostname -I | awk '{print $1}'      # quick: first address
# or, to see the Wi-Fi interface explicitly:
ip -4 addr show
```

Call this `<LAPTOP_IP>` below (e.g. `192.168.1.42`).

### 4b. Open the signaling port through the firewall

Fedora's firewalld blocks inbound by default, so the phone can't reach the
server until you open it. For a test session:

```sh
# Open the signaling WebSocket port
sudo firewall-cmd --add-port=3536/tcp

# The WebRTC datachannel negotiates ephemeral UDP ports. The simplest thing
# for a trusted home LAN test is to mark your Wi-Fi interface trusted for the
# session (replace wlan0 with your interface from `ip -4 addr show`):
sudo firewall-cmd --zone=trusted --add-interface=wlan0
```

These are **not** persistent (no `--permanent`), so a reboot or
`firewall-cmd --reload` reverts them. That's deliberate for a test box.

### 4c. Start the signaling server (laptop)

```sh
matchbox_server --host 0.0.0.0 --port 3536
```

### 4d. Bake the room URL into the phone build, install, launch

The phone has no command line and no settable env var, so the room URL is
baked in at **build time** via `TWOTOP_ROOM`:

```sh
TWOTOP_ROOM=ws://<LAPTOP_IP>:3536/two-top?next=2 \
  cargo apk run -p app --target aarch64-linux-android
```

(To pick the arena for the phone, also prefix `TWOTOP_ARENA=crossing`.)

### 4e. Join from the laptop

```sh
cargo run -p app -- --room ws://<LAPTOP_IP>:3536/two-top?next=2
```

### 4f. What to verify (Phase 12, Gate 1)

- Both lobby overlays reach `connected` within ~2 seconds.
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

## Rung 5 — phone on cellular vs laptop on Wi-Fi (the hard one)

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
with `TWOTOP_ROOM=wss://signal.example.com/two-top?next=2`. Verification is
the same Gate 1–3 checklist as Rung 4.

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

# Laptop joins a room
cargo run -p app -- --room ws://<HOST>:3536/two-top?next=2

# Phone: install solo
cargo apk run -p app --target aarch64-linux-android

# Phone: install pointed at a room (room URL baked at build time)
TWOTOP_ROOM=ws://<HOST>:3536/two-top?next=2 \
  cargo apk run -p app --target aarch64-linux-android

# Laptop LAN IP
hostname -I | awk '{print $1}'

# Open firewall for a LAN test (non-persistent)
sudo firewall-cmd --add-port=3536/tcp
sudo firewall-cmd --zone=trusted --add-interface=wlan0

# Phone logs
adb logcat --pid=$(adb shell pidof com.ampactorlabs.twotop)

# Verbose netplay handshake (desktop)
RUST_LOG=two_top::net=debug,matchbox_socket=debug cargo run -p app -- --room <URL>

# Vulkan capability check
adb shell pm list features | grep vulkan
```

---

## Known gaps (so you don't chase non-bugs)

- **No in-app way to enter a room URL.** The lobby overlay is read-only.
  Desktop uses `--room`/`MATCHBOX_ROOM`; the phone uses the compile-time
  `TWOTOP_ROOM` bake (Rung 4d). An in-app lobby text field is tracked future
  work.
- **First-pair queue only.** The first two peers in a room get matched, in
  arrival order. There are no room codes / private match codes yet, so don't
  share a room name with anyone else mid-test.
- **STUN/TURN isn't a build knob yet** (blocks Rung 5 cross-network play).
- **Cross-platform determinism is CI-only.** You verify single-machine
  determinism locally (SyncTest); the four-platform byte-identical matrix
  runs in CI.
