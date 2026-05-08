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
add by parameterizing the room URL with a per-match code. The lobby
UI scaffold in `crates/app/src/lobby_overlay.rs` shows where the
room-code text-entry field would go.

---

## Configuring the build

The room URL is currently hard-coded in the operator's local edits to
`crates/app/src/lib.rs` — Phase 12 does not surface a settings menu
for it yet. To point the build at your signaling server:

1. Edit the matchbox driver call site (TBD: lands when the operator
   adds the actual `WebRtcSocket::new_unreliable_with_room_url(...)`
   call to `app::run`).
2. Rebuild: `cargo run -p app --release`.
3. Confirm the lobby overlay (top-right corner, yellow text) cycles
   through `idle → connecting → waiting peer → connected` as expected.

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
* **Network logs**: run with `RUST_LOG=net=debug,matchbox_socket=debug`
  to see the matchbox handshake step-by-step.

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
