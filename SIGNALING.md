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

Two datachannels ride that one peer connection: channel 0 is the
unreliable/unordered ggrs input stream, and channel 1 is a **reliable
side-channel** carrying the small postcard messages that are not sim
input — the identity handshake (`NetMsg::Profile`: install-id + dialed
name, which feeds the per-opponent grudge ledger), rematch consent
(`RematchWant`), and a clean goodbye (`Bye`, so the peer's screen flips
immediately instead of waiting out the disconnect grace).

---

## NAT traversal: STUN, and when you need TURN

WebRTC finds the direct path with STUN (the default config points at
Google's public STUN servers). That works for most home-wifi pairs and
many phone pairs — but **carrier-grade NAT (very common on cellular)
regularly defeats STUN-only traversal**. The failure is silent: both
phones reach the signaling server, see each other, and never complete
the datachannel. On screen that reads as an eternal `AWAITING A
CHALLENGER` (the summoning overlay names this after ~15 s).

The fix is a TURN relay — a fallback server both peers can reach that
relays the datachannel when no direct path exists. Configure it the
same way as the room URL (runtime env for desktop, compile-time bake
for the APK):

```bash
TWOTOP_TURN_URL="turn:turn.example.net:3478" \
TWOTOP_TURN_USER="twotop" \
TWOTOP_TURN_PASS="<secret>" \
  cargo apk run -p app --lib --target aarch64-linux-android
```

All three unset ⇒ STUN-only, exactly the pre-TURN behavior. The STUN
urls stay in the ICE list either way; WebRTC applies the credentials
only to the `turn:` entry.

**The baked env vars are for LOCAL testing only.** Anything compiled
into a distributed APK is one `strings` away from public, and a leaked
TURN credential spends your relay bandwidth. Public builds use the
ephemeral vendor below instead; the APK CI workflow refuses to carry
`TWOTOP_TURN_*` by design.

### Ephemeral credentials: the ice_vendor service

`crates/ice_vendor` is a tiny HTTP service that holds the relay secret
server-side and answers `GET /ice` with a THROWAWAY credential pair the
TURN server refuses once its embedded timestamp expires (the TURN REST
API convention, draft-uberti-behave-turn-rest). The app fetches it at
match entry when `TWOTOP_ICE_URL` is set — a URL, not a secret, so it
is safe to bake into public builds — with a 2.5 s timeout falling back
to the baked/STUN-only config. The fetch rides the SUMMONING wait, so
it costs nothing visible.

Two backends, picked by env:

* **Cloudflare TURN** (recommended): set `CF_TURN_KEY_ID` +
  `CF_TURN_API_TOKEN` (create a TURN key in the Cloudflare dashboard
  under Realtime). The vendor proxies Cloudflare's short-lived
  credential generator; the relay is their anycast network, with a
  large free monthly tier (verify current terms). Override the
  endpoint with `CF_TURN_API_URL` if their API moves.
* **Self-hosted coturn**: run coturn with
  `--use-auth-secret --static-auth-secret=<secret>` on a VPS (TURN
  wants UDP ingress — Railway's HTTP proxy can't carry it) and give
  the vendor `ICE_STATIC_AUTH_SECRET=<same secret>` +
  `ICE_TURN_URLS=turn:relay.example.net:3478?transport=udp`. The
  vendor computes `expiry:twotop` / base64(HMAC-SHA1) pairs coturn
  verifies offline.

Neither set ⇒ the vendor answers STUN-only, which still exercises the
whole fetch path. `ICE_TTL_SECS` tunes the credential lifetime
(default 4 h). Deploy the vendor anywhere HTTPS lives — a second
Railway service off this repo with start command
`cargo run -p ice_vendor --release` works — then bake its URL:

```bash
TWOTOP_ICE_URL="https://<vendor-host>/ice" scripts/phone.sh
# or set the TWOTOP_ICE_URL repo variable so CI bakes it into apk-latest
```

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
signaling connection from `OnEnter(InMatch)`. The arena pick happens on the
Title's roster screen before connecting, and it becomes part of the room
name — `two-top-<arena>?next=2`, or `two-top-<CODE>-<arena>?next=2` for a
private room — so two peers in one room have structurally agreed on the
table; there is no arena handshake to get wrong. `TWOTOP_ARENA=<name|id>`
still seeds the pick for desktop automation.

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
(`interval = 30`, ~2 checks/sec), and a **9 s disconnect timeout** —
the away grace that lets a phone survive a notification-shade peek or
a short call banner and rejoin the live match (ggrs replays the missed
ticks on resume). During an interruption the healthy side's overlay
shows `<NAME> AWAY` after ~1 s of silence; at 9 s ggrs declares the
peer gone and the match forfeits (net's own silence FSM backstops at
10 s). A `DesyncDetected` event logs a loud `ERROR` on
`target: two_top::net`; a `Disconnected` event (or a `Bye` on the
side-channel — the clean-leave path) forfeits the match (sets
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

* **Rematch (RUN IT BACK)**: after `MatchOver`, an ONLINE rematch is a
  two-sided handshake. A local THROW press becomes consent
  (`RematchWant` on the side-channel) instead of an instant restart;
  the summary shows `waiting on <NAME>...` / `<NAME> WANTS TO RUN IT
  BACK`. Once both sides consent, each client emits the real THROW
  input and `sim::apply_rematch` restarts the match in lockstep —
  still input-driven and rollback-safe (the gate only shapes when the
  local input is emitted, `netplay::gate_rematch_inputs`). Couch and
  practice keep the classic either-player-instant rematch. Verify both
  devices restart together — score back to 0-0, fresh countdown, no
  desync.
* **Leaving**: online at `MatchOver`, a top-band tap or ESC leaves
  cleanly — `Bye` on the side-channel (peer's screen flips to
  `<NAME> FLED` immediately), socket + session torn down, back to
  Title. Re-tapping FIND OPPONENT opens a fresh socket.

### Gate 2: brief disconnection blip (the away grace)

* Launch two peers, get them connected, start a round.
* Briefly toggle airplane mode on one device for ~2-4 seconds — or
  just pull down the notification shade / take a fake call: the whole
  point of the 9 s grace is surviving normal phone life.
* The healthy side shows `<NAME> AWAY` once the silence threshold
  (`DISCONNECT_AFTER_FRAMES = 60`, ~1 s) is crossed; its sim stalls
  (prediction window full) under the overlay.
* Restore connectivity within the 9 s ggrs window.
* Both sides should return to `connected`, the round resumes (ggrs
  replays the missed ticks), and the post-blip simulation stays
  bit-identical.

### Gate 3: forfeit on long disconnection

* Same setup, but kill connectivity on one device for 12+ seconds.
* The surviving device transitions `<NAME> AWAY → <NAME> FLED - the
  field is yours` at ~9 s of silence (ggrs `Disconnected`;
  `FORFEIT_AFTER_FRAMES = 600` is the 10 s fallback), records a career
  WIN, and offers the top-band LEAVE.
* `sim::MatchState::MatchOver` fires (the round flow ends; HUD shows
  match-over state).
* The forfeiting peer, on coming back, sees `MATCH ABANDONED — you
  left the duel` (its own absence is tracked, so the copy doesn't
  gaslight) and records the LOSS.
* Bonus check: instead of killing connectivity, tap LEAVE on one
  device — the other flips to `<NAME> FLED` *immediately* (the `Bye`
  message), no 9 s wait.

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

* ~~Room-code-based pairing~~ — done: the title's room pad dials 7⁴
  private rooms (`crates/app/src/room_code.rs`).
* ~~Player identity~~ — done: install-id + dialed name exchanged on the
  reliable side-channel; the grudge ledger files per-opponent records.
* TLS configuration default in the signaling URL builder.
* Auth / matchmaking filters — not in V1 scope.
* Spectator slot (room joins beyond 2 are queued; the on-device replay
  theater covers after-the-fact viewing).
* Settings UI for signaling URL (currently edit-and-rebuild only).
* The `Disconnected → Connected` reconnect path currently treats the
  reconnect as a fresh peer-connection edge (firing `PendingP2PSwap`
  again). If the existing P2PSession should be kept alive across
  blips, refine `should_swap_to_p2p` to suppress reconnect re-swaps.
  (Post-swap, ggrs interruptions no longer touch this path — the
  lobby's `Disconnected` now comes from honest silence aging and
  recovers without a re-swap.)
* Emotes: one `NetMsg` variant away — the side-channel is the hard
  part and it's in. Waiting on a design for where they live on screen.
