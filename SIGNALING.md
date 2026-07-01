# Netplay signaling — operator runbook

Phase 12 introduced the WebRTC peer-to-peer transport layer
(`crates/net`). The actual peer-discovery handshake happens through a
matchbox-protocol signaling server that brokers the WebRTC offer/answer
exchange before peers establish a direct datachannel. This file is the
operator-facing recipe for getting two real devices on different
networks to find each other and play a deterministic 2-Top match.

CI cannot verify these flows — there's no real network in a sandbox.
Phase 12's exit criteria (per `BUILD_PLAN.md`) are operator-verified
against actual devices.

---

## Architecture, in one diagram

```
   ┌──────────┐         WebSocket          ┌──────────────────┐         WebSocket         ┌──────────┐
   │ device A │ ───────────────────────►   │ matchbox_server  │  ◄───────────────────────  │ device B │
   │ (2-Top)  │ ◄───────────────────────   │ (signaling only) │  ───────────────────────►  │ (2-Top)  │
   └──────────┘                            └──────────────────┘                            └──────────┘
        ▲                                                                                       ▲
        │                                          WebRTC datachannel (P2P)                     │
        └───────────────────────────────────────────────────────────────────────────────────────┘
                            (sim inputs flow here once handshake completes)
```

The signaling server **only brokers the introduction**. Once both peers
have exchanged ICE candidates and built a working datachannel, the sim
inputs (4 bytes per player per tick) flow peer-to-peer with no server
in the path — that's the property the 2-Top determinism stack is
designed for.

---

## Choosing a signaling server

You have three options for the matchbox-protocol signaling server.
Pick based on your testing situation:

### Option 1: Public test server (fastest to start)

`johanhelsing/matchbox` ships a public signaling deployment for casual
testing. Configure the room URL in your build to point at it. **Do not
ship a public-server-pointed build to actual users** — it has no SLA,
no privacy guarantees, and shared rooms can collide.

### Option 2: Self-hosted on a VPS (recommended for real testing)

Spin up `matchbox_server` on any small VPS (1 CPU / 512 MB is plenty;
the server does no heavy work). Standard Cargo install:

```bash
cargo install matchbox_server
matchbox_server --host 0.0.0.0 --port 3536
```

Then on each device, configure the room URL to:

```
ws://<your-vps-public-ip>:3536/two-top?next=2
```

The `?next=2` query parameter tells matchbox to pair peers in groups
of two — first-pair queue semantics. The room name (`two-top`)
namespaces your traffic so multiple games / dev branches can share
one server without collision.

For TLS: front the server with caddy or nginx and point peers at
`wss://your.host/two-top?next=2`.

### Option 3: Self-hosted as a library (operator-customizable)

If you want to add auth, room codes, queue-fairness, or matchmaking
filters, the `matchbox_signaling` library lets you build a custom
server with the same protocol. See johanhelsing/matchbox README for
the trait surface. Out of scope for V1 of 2-Top.

---

## Pairing model

V1 uses **first-pair queue**: the first two players to join a room
become each other's peers, in arrival order. The third player to
join blocks until one of the first two leaves, then pairs with the
fourth, and so on.

Room codes (manual pairing) are not yet implemented in `crates/net` —
add by parameterizing the room URL with a per-match code. The Title
screen already has a Play gesture that starts the connection; room-code
entry and richer matchmaking UI are future work. The top-right
`lobby_overlay` remains a read-only `Text2d` `LobbyState` status label.

---

## Configuring the build

On desktop the room URL is a **runtime argument**, not a build-time
constant — the live matchbox driver landed in `crates/app/src/netplay.rs`
(M4). On **Android** there is no argv and no settable process env on a
tapped-icon launch, so the APK additionally reads a **compile-time**
`TWOTOP_ROOM` value baked at build time (precedence:
`--room` > `MATCHBOX_ROOM` > compiled `TWOTOP_ROOM`). Online play is
opt-in; the default build (none of the three set) is unchanged (local
SyncTest couch-versus).

Online builds still boot to the Title screen. When a room URL is present,
the title copy changes to `TAP TO FIND OPPONENT`; tapping the lower half
or pressing Start enters `InMatch`, and `MatchboxPlugin` opens the
signaling connection from `OnEnter(InMatch)`. Arena selection works on
that Title screen before connecting. `TWOTOP_ARENA=anchor|crossing|reliquary`
is still a useful startup default for desktop automation, but it is no
longer the only online arena selector.

1. Start a signaling server (local dev): `matchbox_server` (defaults to
   `0.0.0.0:3536`). For a real deployment see "Choosing a signaling
   server > Option 2: Self-hosted on a VPS" above.
2. Launch the game pointed at a room:

   ```bash
   # Desktop:
   cargo run -p app -- --room ws://<host>:3536/two-top?next=2
   # or, equivalently:
   MATCHBOX_ROOM=ws://<host>:3536/two-top?next=2 cargo run -p app

   # Android (room URL baked in at build time):
   TWOTOP_ROOM=ws://<host>:3536/two-top?next=2 \
     cargo apk run -p app --lib --target aarch64-linux-android
   ```

   `--room <url>` takes precedence over `MATCHBOX_ROOM`, which takes
   precedence over the compiled-in `TWOTOP_ROOM`. Absent all three,
   the build runs the local SyncTest session (no network). For a full
   device-by-device test walkthrough see [`PLAYBOOK.md`](./PLAYBOOK.md).
3. On the Title screen, tap the lower half or press Start. The lobby overlay
   (top-right, yellow text) then cycles `connecting → waiting peer →
   connected`. The online build installs **no** session up front — the sim
   idles session-less at frame 0 until the peer connects, at which point
   `perform_swap` *inserts* a live `P2PSession`. (Only the local/couch build
   runs a SyncTest session, and that one is created on match-start in
   `screen::spawn_match`, not here.)

### Handle assignment (deterministic, peer-agreed)

Both peers must agree on which is player 0. The rule: **the lower
`PeerId` is handle 0**, the higher is handle 1. Each peer learns both
ids from the signaling handshake and computes the same assignment
locally — no negotiation round-trip. Player *entities* are spawned
identically on both sides (same `setup`), so only "which handle reads
local input" differs; the deterministic sim guarantees the rest.

The ggrs session is built with `input_delay = 2`, desync detection on
(`interval = 30`, ~2 checks/sec), and a 3 s disconnect timeout. A
`DesyncDetected` event logs a loud `ERROR` on `target: two_top::net`;
a `Disconnected` event forfeits the match (sets
`sim::MatchState::MatchOver`).

### Loopback verification (Gate 0 — done, automatable)

Verified on a single box: `matchbox_server` + two `app --room
ws://127.0.0.1:3536/two-top?next=2` instances. Both reach `connected`,
swap to `P2P` with agreed handles, complete the 5-step sync handshake,
and run a live match with **zero `DesyncDetected` events**. Killing one
instance forfeits the survivor (`peer disconnected → MatchOver →
Forfeited`) within ~2.5 s. A transient stall (CPU contention) surfaced
as `NetworkInterrupted` and cleanly `NetworkResumed` with no desync.
The three gates below additionally require **real devices / real
networks** and remain operator-executed.

---

## Exit-criteria test plan

These are the operator-verified gates per `BUILD_PLAN.md` § Phase 12.

### Gate 1: full match across networks

Two devices, on different networks (say a phone on cellular and a
laptop on home wifi), launch the build pointed at the same room URL.

* Within ~2 seconds, both lobby overlays should reach `connected`.
* The 3-2-1 countdown begins on both devices.
* Players move, throw, recall, kill, respawn — all in lockstep on
  both screens.
* MatchScore + MatchState diverge by zero across both devices for the
  full 30-second round (and any subsequent rounds).
* The match ends naturally (first to 5 kills → MatchOver).

If either side desyncs visibly, take both `.bmrg.log` files and run
`scripts/diagnose_desync.sh` against the per-frame checksum logs.

* **Rematch**: after `MatchOver`, *either* player pressing THROW
  restarts the match in lockstep (`sim::apply_rematch`, which runs in
  `GgrsSchedule` right before `tick_match_state` and is input-driven,
  so it's rollback/netplay-safe). Verify both devices restart
  together — score back to 0-0, fresh countdown, no desync. Note the
  couch-only ESC back-to-lobby does **not** apply online:
  `screen::back_to_lobby` early-returns when a room URL is set (the
  lobby FSM owns teardown).

### Gate 2: brief disconnection blip

* Launch two peers, get them connected, start a round.
* Briefly toggle airplane mode on one device for ~1.5 seconds.
* Lobby overlay on both sides should show `reconnecting…` once the
  silence threshold (`DISCONNECT_AFTER_FRAMES = 60`, ~1 s) is crossed.
* Restore connectivity within the forfeit window
  (`FORFEIT_AFTER_FRAMES = 180`, ~3 s total silence).
* Both sides should return to `connected`, the round resumes, and
  the post-blip simulation stays bit-identical.

### Gate 3: forfeit on long disconnection

* Same setup, but kill connectivity on one device for 5+ seconds.
* The surviving device's lobby overlay transitions
  `connected → reconnecting… → FORFEIT` within ~3 seconds of silence.
* `sim::MatchState::MatchOver` fires (the round flow ends; HUD shows
  match-over state).
* The forfeiting peer, on regaining connectivity, sees
  `reconnecting…` then a permanent `FORFEIT` (terminal — no recovery
  from `Forfeited`, by design).

---

## Debug helpers

* **Lobby overlay**: top-right corner of every running `app` build,
  yellow text, single line of current `LobbyState`. Visible without
  any debug build flag.
* **Determinism diagnosis**: per-frame checksum TSV produced by the
  cross-platform CI matrix is also reproducible locally via
  `replay_sync`. If two real devices desync, save their `.bmrg.log`
  files and use `scripts/diagnose_desync.sh` on the resulting TSVs.
* **Network logs**: run with
  `RUST_LOG=two_top::net=debug,matchbox_socket=debug` to see the
  matchbox handshake step-by-step. The app's net logs use the
  `two_top::net*` target, not bare `net`. Most are `info`/`warn`/`error`
  (already visible at the default filter); the `debug!`/`trace!` lines
  are compiled out in release builds.

---

## Known follow-ups

These are tracked but not blocking Phase 12's CI gate:

* Room-code-based pairing (current impl is first-pair queue only).
* TLS configuration default in the signaling URL builder.
* Auth / username / matchmaking filters — not in V1 scope.
* Spectator slot (room joins beyond 2 are queued; spectator mode lands
  with Phase 14's replay viewer).
* Settings UI for signaling URL (currently edit-and-rebuild only).
* The `Disconnected → Connected` reconnect path currently treats the
  reconnect as a fresh peer-connection edge (firing `PendingP2PSwap`
  again). If the existing P2PSession should be kept alive across
  blips, refine `should_swap_to_p2p` to suppress reconnect re-swaps.
