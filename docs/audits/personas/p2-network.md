# Persona 2 — "Ray, on the train"
## Netplay reliability audit of 2-Top

> I got on at Embankment with a full four bars. Tapped PLAY, got paired,
> and for about ninety seconds it was the best thing on my phone. Then we
> went into the tunnel under the river and the picture just… stopped.
> Not a spinner. Not "opponent away." The exact frame I was on, held,
> like the phone had died. I tapped it. I tapped it again. Nine seconds
> later it said **STAG FLED — the field is yours**, and gave me a win I
> hadn't earned, for a disconnect that was *mine*.
>
> When I came out the other side I checked RIVALS. It said I was up 4-2.
> He texted me a photo of his screen. His said he was up 4-2.
>
> Both of us think we won the same match. That's the part I can't get
> past. Everything else in this game is honest — there's a whole signed-
> attestation thing, there's a tape, there's a ledger. And the one number
> two people would actually argue about is the one nobody can prove.

---

## Findings

| # | Finding | Sev | Proof |
|---|---|---|---|
| 1 | Rollback config is LAN-tuned: ggrs's default 8-frame prediction ceiling, never raised, and `WaitRecommendation` is thrown away | 🔴 | `netplay.rs:552-566`, `:634-636`; ggrs `builder.rs:21` |
| 2 | The whole away-grace FSM (OPPONENT AWAY, forfeit grace) is unreachable online — its clock freezes during the exact stall it detects | 🔴 | `net:739-760`, `netplay.rs:669-672`, `sim:2883`, `sim:3358` |
| 3 | Both phones record a WIN for the same dropped match; forfeit blame is a local-only guess that never fires on a network drop | 🔴 | `grudge.rs:258-268`, `:303-305`; `netplay.rs:796-810` |
| 4 | Forfeit/goodbye writes `sim::MatchState` out-of-band into a **rolled-back** resource — CONVENTIONS forbids it; a rollback un-ends the match | 🔴 | `netplay.rs:678`, `:746`; `net:791`; `sim:3360`; `CONVENTIONS.md:14` |
| 5 | Desync is detected, logged, and nothing else: no player signal, no halt, tape still written, ledger still recorded | 🔴 | `netplay.rs:641-655`; zero other `Desync` refs in `app/` |
| 6 | Ledger + attestation commit on a *predicted* MatchOver; only the recorder got the 30-frame guard | 🟠 | `recorder.rs:36-40` vs `grudge.rs:287-289`, `attest.rs:81-90,136` |
| 7 | No reconnect path anywhere. A tunnel dropout past 9 s is a dead match, structurally unrecoverable | 🟠 | `netplay.rs:461-469`, `:542-550`; `net:648-651` |
| 8 | Side channel trusts any peer, at any time: unvalidated `from` on `Bye`; mid-match `Profile` rewrites the ledger key; install-id never proved | 🟠 | `netplay.rs:712-748`; `net:193-199` |
| 9 | TURN failure is invisible and falls back to exactly the config Ray can't use; vendor is single-threaded with a 5 s inline upstream call | 🟠 | `netplay.rs:196-212`, `:400-410`; `ice_vendor:243-247`, `:295` |
| 10 | Zero automated coverage of the live netplay path; "loopback-verified" is a manual 0 ms-RTT rung where 1-9 are all invisible | 🟡 | no `crates/net/tests`, no `crates/app/tests`; `PLAYBOOK.md:87` |

---

## 1. 🔴 The rollback config is tuned for a LAN, and Ray is nine frames away

`perform_swap` builds the only `P2PSession` in the product. Four knobs are set:

```rust
// crates/app/src/netplay.rs:552-559
let mut sb = SessionBuilder::<GgrsCfg>::new()
    .with_num_players(2).expect("2 players")
    .with_input_delay(ONLINE_INPUT_DELAY)          // = 2   (:50)
    .with_disconnect_timeout(DISCONNECT_TIMEOUT)   // = 9 s (:65)
    .with_desync_detection_mode(DesyncDetection::On { interval: DESYNC_CHECK_INTERVAL });
```

`with_max_prediction_window`, `with_max_frames_behind` and `with_catchup_speed`
are **never called anywhere in the workspace** (grep over `crates/`: the only
hits for the other builder knobs are `with_check_distance`/`with_input_delay`
in SyncTest and replay harnesses). So the session takes ggrs's defaults:

```rust
// ggrs-0.12.0/src/sessions/builder.rs:21,24,26
const DEFAULT_MAX_PREDICTION_FRAMES: usize = 8;
const DEFAULT_MAX_FRAMES_BEHIND: usize = 10;
const DEFAULT_CATCHUP_SPEED: usize = 1;
```

And the ceiling is a hard stop, not a soft one:

```rust
// ggrs-0.12.0/src/sessions/p2p_session.rs:400-412
let frames_ahead = self.sync_layer.current_frame() - self.sync_layer.last_confirmed_frame();
frames_ahead < self.max_prediction as i32
...
} else {
    debug!("Prediction Threshold reached. Skipping on frame {}", ...);
}
```

Do the arithmetic on Ray's link. 300 ms RTT is 150 ms one-way = **9 frames at
60 Hz**. A remote input for frame *N* cannot arrive before our frame *N+9*, so
`current_frame - last_confirmed_frame` sits at ~9 — permanently *above* the
8-frame ceiling. The session lives pinned against the stop: it advances only as
fast as confirmations arrive, rolling back ~9 frames every single frame it does
advance, and a single dropped packet is an immediate visible freeze rather than
a prediction that gets quietly corrected. At 150 ms RTT (4-5 frames each way
plus the 2 frames of input delay) he is already at 7 of 8 — no headroom for the
jitter that defines a cellular link.

The escape hatch ggrs provides is also discarded:

```rust
// crates/app/src/netplay.rs:634-636
GgrsEvent::WaitRecommendation { skip_frames } => {
    tracing::debug!(target: "two_top::net", skip_frames, "wait recommendation");
}
```

That event is ggrs asking the client that is running ahead to stall N frames so
the two clocks meet in the middle — it fires only when `frames_ahead >=
MIN_RECOMMENDATION` (3) and at most once per 60 frames
(`p2p_session.rs:841-853`). Logging it at `debug!` and doing nothing means the
frame-advantage skew between Ray and his Wi-Fi opponent is never corrected;
whichever phone is ahead stays ahead and eats every stall.

Note also that the determinism gate is `check_distance: 7` (CI SyncTest) while
the live prediction window is 8 — the deepest rollback the product can actually
perform online is one frame deeper than anything CI has ever verified.

**Fix:** set `.with_max_prediction_window(16..20)` (≈330 ms of headroom at 60 Hz)
and raise `ONLINE_INPUT_DELAY` to 3-4 for online play, which buys back most of
the prediction depth at a cost Ray will not feel next to a freeze. Then actually
honour `WaitRecommendation` by skipping `skip_frames` advances. Both are pure
netcode changes — no `SIM_VERSION` bump, since neither alters a tick's result.

---

## 2. 🔴 OPPONENT AWAY can never appear — its clock stops during the stall

This is the finding behind Ray's nine frozen seconds, and it is a whole
shipped feature (`CLAUDE.md`: *"away-grace with honest forfeit blame"*) that
cannot fire in a real match.

The silence FSM measures in **sim frames**:

```rust
// crates/net/src/lib.rs:739-751
pub fn next_lobby_state_for_silence(curr: &LobbyState, frame: u32, last_msg: u32) -> Option<LobbyState> {
    let elapsed = frame.saturating_sub(last_msg);
    match curr {
        LobbyState::Connected { peer_id } if elapsed >= DISCONNECT_AFTER_FRAMES => { ... }
```

`frame` is `sim::FrameCount`, which advances in exactly one place:

```rust
// crates/sim/src/lib.rs:2883
pub fn advance_frame_count(mut frame: ResMut<FrameCount>) { frame.0 = frame.0.wrapping_add(1); }
```

…and that is a `GgrsSchedule` system, so it only ticks when ggrs emits an
`AdvanceFrame` request. Per finding #1, a silent peer stalls ggrs after ~8
frames. **From that moment `FrameCount` is frozen.**

Now trace the driver's half:

```rust
// crates/app/src/netplay.rs:669-672
if !world.non_send_resource::<MatchboxDriver>().interrupted {
    let frame = world.resource::<sim::FrameCount>().0;
    world.resource_mut::<LastPeerMessageFrame>().0 = frame;
}
```

`interrupted` flips true on `GgrsEvent::NetworkInterrupted` (`:623-629`), which
ggrs raises after `disconnect_notify_start` of silence. That knob is **also never
set** (`with_disconnect_notify_delay` appears nowhere), so it is ggrs's default
**500 ms** (`builder.rs:19`) — by which time ggrs has been stalled for ~370 ms
and `FrameCount` has been frozen for just as long.

So at the instant pinning stops, `LastPeerMessageFrame == FrameCount`, and
neither moves again. `elapsed` is **0 forever**. `DISCONNECT_AFTER_FRAMES` (60)
is never reached, so `LobbyState::Disconnected` never happens, so this:

```rust
// crates/app/src/lobby_overlay.rs:130-132
LobbyState::Disconnected { .. } => Some(format!(
    "{challenger} AWAY{dots}\nhold the field - forfeit soon"
)),
```

…never renders. `FORFEIT_AFTER_FRAMES` (600) is likewise unreachable, making
net's entire fallback gate dead code online — despite its own doc claiming it is
*"the fallback for … anything ggrs misses"* (`net:717-725`).

What Ray actually gets: a hard-frozen frame, no overlay, no spinner, no text,
for the full 9 s `DISCONNECT_TIMEOUT`, then an abrupt `X FLED` with no
explanation of what happened in between.

*(Exact frame counts want the field test the README admits hasn't run; the
mechanism — a frame-clock that stops during the event it measures — is provable
from the three files above.)*

**Fix:** measure peer silence on `Time<Real>`, not `FrameCount`. The comment at
`net:702-707` argues for frame-time so "both peers agree on the silence
threshold", but disconnection is observed locally and acted on locally — there
is nothing to agree about, and the price of the agreement is that the clock
stops. Separately, surface the *stall itself*: the moment ggrs returns no
`AdvanceFrame` request for >200 ms, put something on screen.

---

## 3. 🔴 Both phones record a win for the same match Ray's network dropped

Blame for a forfeit is decided independently on each device by one pure function:

```rust
// crates/app/src/grudge.rs:258-268
pub fn match_won(our_score: u8, their_score: u8, forfeited: bool, we_went_absent: bool) -> bool {
    if our_score >= MATCH_WIN_THRESHOLD { return true; }
    if their_score >= MATCH_WIN_THRESHOLD { return false; }
    // Nobody reached the threshold: a forfeit decided it. If our own phone
    // went away, the walk-out is ours to own; otherwise the field is ours.
    forfeited && !we_went_absent
}
```

The only input that can ever assign blame to *me* is `we_went_absent`, and that
is a purely local guess about the **process**, not the **network**:

```rust
// crates/app/src/netplay.rs:796-810
pub fn track_absence(time, mut focus_events: MessageReader<WindowFocused>, mut absence) {
    let now = time.elapsed_secs();
    if time.delta_secs() > ABSENCE_FREEZE_SECS { absence.0 = Some(now); }   // = 2.0 (:793)
    for ev in focus_events.read() { if !ev.focused { absence.0 = Some(now); } }
}
```

A tunnel, a Wi-Fi→cellular handoff, a carrier NAT rebind: the app keeps focus,
the main loop keeps running at 60 fps (it is rendering a frozen sim, not
frozen itself), `delta_secs()` stays at 16 ms. **`we_went_absent` is false on
the phone whose network died.**

Both sides therefore evaluate `forfeited && !we_went_absent` → `true`. Both
increment `record.wins`, both increment `rival.wins`, both call
`next_streak(streak, true)`:

```rust
// crates/app/src/grudge.rs:303-326
let forfeited = matches!(*lobby, net::LobbyState::Forfeited { .. });
let we_went_absent = absence.within(time.elapsed_secs(), RecentAbsence::FORFEIT_BLAME_SECS);
let won = match_won(ours, theirs, forfeited, we_went_absent);
if won { record.wins += 1; } else { record.losses += 1; }
...
rival.streak = next_streak(rival.streak, won);
save_career(&record);
```

Two consequences, both fatal to the pillar:

1. **Every network-caused forfeit is double-credited.** Ray's ledger and his
   opponent's ledger permanently disagree about a match they both played. The
   rivalry screen's "YOU LEAD 3-1" is not a shared fact.
2. **A favourable forfeit is one toggle away.** Losing 1-4? Flip airplane mode.
   No focus loss, no frame freeze, so `we_went_absent` is false: the quitter
   banks a *win* and dodges the loss. The victim also banks a win, so nothing
   in either ledger looks anomalous. Nothing in the code challenges this —
   forfeits are explicitly excluded from the signed path (`net:255-257`,
   `attest.rs:94-97`), so the one mechanism that could prove an outcome is
   switched off for precisely the outcomes people dispute.

The comment at `grudge.rs:13-17` claims *"Forfeits are scored honestly"*. They
are scored *charitably*, to whoever is holding the phone.

**Fix (cheapest honest version):** stop claiming a win for a forfeit nobody can
prove. Record it as a third outcome (`abandoned`) that shows on the rivalry row
as "unfinished" and moves neither W/L nor streak, unless the *departing* side
sent an explicit signed `Bye`. That makes the ledger's silence honest instead of
making both ledgers confidently wrong. Optionally extend the `MatchStatement`
scheme to cover a signed concession, which is the only construction that makes
a forfeit provable at all.

---

## 4. 🔴 The forfeit write goes into a rolled-back resource — the rulebook's own worked example

`CONVENTIONS.md:14`, verbatim:

> **Match/round state transitions are input-driven; never mutate `MatchState`
> (or score/arena reset) out-of-band.** … A "play again" button writing
> `*match_state` directly from a UI/app system would desync.

Three sites do exactly that, from the `Update` schedule:

```rust
// crates/app/src/netplay.rs:674-679   (ggrs Disconnected)
*world.resource_mut::<LobbyState>() = LobbyState::Forfeited { peer_id: addr_to_peer(addr) };
*world.resource_mut::<sim::MatchState>() = sim::MatchState::MatchOver;

// crates/app/src/netplay.rs:743-747   (peer said Bye)
*world.resource_mut::<LobbyState>() = LobbyState::Forfeited { peer_id: from };
*world.resource_mut::<sim::MatchState>() = sim::MatchState::MatchOver;

// crates/net/src/lib.rs:791           (silence grace)
*match_state = sim::MatchState::MatchOver;
```

And `MatchState` is registered for rollback:

```rust
// crates/sim/src/lib.rs:3360
.rollback_resource_with_copy::<MatchState>()
```

I tried to disprove this on the **ggrs `Disconnected`** path and it holds up:
`poll_remote_clients` sets `disconnect_frame` at the *top* of `advance_frame`
(`p2p_session.rs:266, 913-924, 652`) and performs the disconnect rollback later
in the *same* call (`:329`), all in `PreUpdate` — so the rollback has already
happened by the time `drain_session_events` runs in `Update`, and with the
remote endpoint dead no further rollbacks occur. That path is safe. Good.

The **`Bye` path is not.** `Bye` arrives on the *reliable* channel while the ggrs
session is fully alive and channel 0 is still delivering the peer's trailing
inputs. The sequence is:

- `Update N`: `pump_side_channel` reads `Bye` → `MatchState = MatchOver`,
  `LobbyState = Forfeited`. The summary card goes up. `record_match_result`
  fires on the rising edge (`grudge.rs:287-289`) and writes `career.json`.
  `sign_decided_match` fires and increments `attest.decided` (`attest.rs:136`).
- `PreUpdate N+1`: an input still in flight for an earlier frame contradicts the
  prediction → `check_simulation_consistency` → `adjust_gamestate` → `LoadWorld`
  → **`MatchState` is restored from the snapshot.** The summary card vanishes
  and the match resumes against a frozen opponent.
- ~9 s later ggrs's own `DISCONNECT_TIMEOUT` fires → `MatchOver` again →
  `record_match_result` fires **a second time** → the result is recorded twice.

Per finding #1, at 300 ms RTT there are ~9 outstanding predicted frames at all
times, so a rollback in that window is close to certain, not a corner case.

*(SPECULATIVE on frequency without the field test; the mechanism — an
out-of-band write to a `rollback_resource_with_copy` resource, while the
rollback machinery is still live — is fully determined by the four lines above.)*

**Fix:** don't write `MatchState` from `Update`. Follow the pattern the codebase
already documents as canonical (`sim::apply_rematch`, `sim:3047-3062`): put a
non-rolled-back `ForfeitRequested` flag in the world and have a `GgrsSchedule`
system consume it, so the transition resimulates identically. Failing that, at
minimum latch the forfeit outside the rollback set and re-assert it after every
`LoadWorld`.

---

## 5. 🔴 A desync is a log line and nothing else

This is the single most load-bearing safety signal in a rollback game, and its
entire handler is:

```rust
// crates/app/src/netplay.rs:641-655
GgrsEvent::DesyncDetected { frame, local_checksum, remote_checksum, addr } => {
    tracing::error!(
        target: "two_top::net",
        frame, local_checksum, remote_checksum, ?addr,
        "DESYNC DETECTED — local and remote state diverged",
    );
}
```

Grepping `Desync`/`DESYNC` across `crates/app/src/` and `crates/render/src/`
returns five hits, **all of them in this one file**: the module doc, the
`use` line, the interval constant, the builder call, and the match arm. There
is no UI, no state change, no abort, no flag anyone downstream reads.

So from the moment of divergence:

- Ray and his opponent are playing two different games on two screens, and
  neither is told. The module doc calls the `error!` *"the gate that matters"* —
  it is a gate for the developer reading `logcat`, not for the player.
- The match runs to a decision on each phone **independently**, so the two
  scores can differ. Each phone records its own version into `career.json`
  (`grudge.rs:307-326`) and its own tape (`recorder.rs:178-208`).
- The signed-result pillar quietly fails closed but says nothing: two divergent
  `MatchStatement`s never match, so the peer's signature is rejected
  (`attest.rs:172-180`, `"peer signature REJECTED — result stays unsigned"`),
  the sidecar is never written, and the ledger *still records the win* — just
  without the `attested_wins` bump. The one place a desync leaves a visible
  trace is a missing counter that nobody was watching.
- The tape is saved and offered for sharing. A desynced tape re-simulated by
  `replay_sync` produces one canonical history that matches at most one of the
  two phones' scores — i.e. the "provable artifact" is provably wrong for one
  player, with nothing marking it.

**Fix:** treat `DesyncDetected` as terminal. End the match, tell both players
plainly ("the two phones stopped agreeing — this one doesn't count"), skip the
ledger write entirely, and either skip the tape or stamp a `desynced` flag into
`ReplayHeader` so the theater and `replay_sync` refuse to present it as a
result. A desync is the one outcome where "record nothing" is unambiguously the
honest answer.

---

## 6. 🟠 The ledger and the attestation commit on a *predicted* match end

`MatchState` is derived purely from the rolled-back `MatchScore`:

```rust
// crates/sim/src/lib.rs:3000-3023
let match_won = score.p0 >= MATCH_WIN_THRESHOLD || score.p1 >= MATCH_WIN_THRESHOLD;
...
MatchState::InRound { .. } => { if match_won { MatchState::MatchOver } else ... }
```

so a mispredicted deciding kill produces a `MatchOver` that stands for up to
`max_prediction` frames before the correction rolls it back. The recorder knows
this and guards against it explicitly:

```rust
// crates/app/src/recorder.rs:36-40
/// Render-frames to wait after `MatchOver` before writing the file. The
/// tape already holds the deciding kill the moment it lands; the delay
/// lets the last few PREDICTED online ticks get resim-corrected (rollback
/// overwrites their tape slots) before the bytes are frozen.
const SAVE_DELAY_FRAMES: u8 = 30;
```

The other two consumers of the same edge got no such guard. Both fire
immediately:

```rust
// crates/app/src/grudge.rs:287-289   → save_career() at :326
let over = matches!(*state, MatchState::MatchOver);
let entered = over && !*prev_over;
*prev_over = over;

// crates/app/src/attest.rs:81-83, then :136
let over = matches!(*state, MatchState::MatchOver);
let entered = over && !*prev_over;
...
attest.decided += 1;
```

Consequences:

- **Ledger:** a phantom decision writes a win/loss + streak + `last_met_unix` to
  `career.json`; the real decision writes it again. Double-counted, and the
  streak can be advanced twice for one match.
- **Attestation, worse:** `attest.decided` is the `match_index` field of the
  canonical `MatchStatement` (`net:373-374` — *"`match_index` counts the matches
  this session already decided"*). The two peers mispredict *different* things,
  so a phantom decision on one phone desynchronises the counter permanently.
  Every subsequent statement in that session — including every RUN IT BACK —
  encodes a different `match_index` on each side, so the signatures cannot
  verify, forever, and the only symptom is `"peer signature REJECTED — result
  stays unsigned"` in a log nobody reads.

**Fix:** give both the recorder's treatment — a confirmed-frame or N-frame
settle before committing. Better: gate on ggrs's *confirmed* frame having passed
the deciding frame, which is the exact fact these two want and which the
recorder is only approximating with 30 frames.

---

## 7. 🟠 There is no reconnect. At all.

Grepping `reconnect|re-open|restart_session` across `crates/app/src` and
`crates/net/src` returns nine hits — **every one of them a comment**. The most
telling is the TODO that never landed:

```rust
// crates/net/src/lib.rs:646-651
/// Note: a transition `Disconnected -> Connected` is technically a
/// reconnect after a brief blip; we treat it as a fresh connection
/// edge for now (cycle 4 may refine the semantics if reconnects
/// need to keep the existing `P2PSession` alive instead of swapping
/// in a new one).
```

Structurally it cannot be retrofitted cheaply, because the unreliable channel is
consumed exactly once and the driver latches on that:

```rust
// crates/app/src/netplay.rs:461-469
if channel_taken {
    drain_session_events(world);
    pump_side_channel(world);
    return;                      // the pre-swap path is never re-entered
}
// :544-548
let channel = driver.socket.take_channel(GGRS_CHANNEL)
    .expect("unreliable channel 0 is present until taken exactly once");
driver.channel_taken = true;
```

`channel_taken` is never reset except by dropping the whole driver in
`leave_online_match` (`:888`), which also drops the `Session` and returns to
Title. So once ggrs says `Disconnected`, the match is over as a matter of data
structure, not policy.

For Ray this means: a **2-second** tunnel is survivable only because 2 < 9, and
only because the WebRTC transport happens to keep its candidate pair. A
Wi-Fi→cellular handoff changes the local address; nothing here performs an ICE
restart and `matchbox_socket` 0.14 does not do one for you, so the handoff is
unconditionally fatal even though both phones still have perfect connectivity
two seconds later.

**Fix:** it is a real feature, not a one-liner. The honest intermediate step is
to make the *failure* legible and cheap — surface "connection lost" the moment
ggrs interrupts (see #2), keep the tape, offer RUN IT BACK against the same room
code as a one-tap re-pair, and don't record a result (see #3). A true mid-match
reconnect needs a second socket + session rebuild keyed on the same room, which
is a workstream.

---

## 8. 🟠 The side channel trusts any peer, at any time, about anything

`pump_side_channel` drains the reliable channel and acts on every message. The
sender is bound to `from`, and `from` is never checked against the peer we are
actually playing:

```rust
// crates/app/src/netplay.rs:712-748
for (from, bytes) in inbound {
    let Some(msg) = decode_net_msg(&bytes) else { ... continue; };
    match msg {
        NetMsg::Profile(profile) => {
            world.resource_mut::<PeerProfile>().0 = Some(profile);
            reseed_our_table(world, profile.install_id);
        }
        NetMsg::Profile2(data2) => {
            world.resource_mut::<PeerProfile>().0 = Some(data2.profile());
            world.resource_mut::<PeerKeys>().0 = Some(data2.pubkey);
            ...
        }
        NetMsg::RematchWant => { world.resource_mut::<RematchConsent>().peer = true; }
        NetMsg::Bye => {
            *world.resource_mut::<LobbyState>() = LobbyState::Forfeited { peer_id: from };
            *world.resource_mut::<sim::MatchState>() = sim::MatchState::MatchOver;
        }
    }
}
```

Credit where due: the *shapes* are hardened well. `decode_net_msg` returns
`Option` and drops garbage (`net:244-246`), names are a fixed `[u8; NAME_MAX]`
so "a hostile peer cannot hand us a 10 MB name, because there is nowhere to put
one" (`net:177-181`) is a true claim, and the decode side clamps out-of-range
glyph indices with `NAME_ALPHABET[i as usize % NAME_ALPHABET.len()]`
(`profile.rs:181`) so no index can panic. The ggrs channel's malformed-packet
drop is tested (`net:873-880`). None of the "huge string / bad name" attacks
land.

What does land is **semantics**:

- **`Bye` from an unvalidated sender ends the match.** Nothing compares `from`
  to the lobby's `peer_id`. Two-peer rooms are a *deployment convention*
  (`?next=2`, `PLAYBOOK.md:112`), not something the client enforces — and the
  client will happily compose a room URL without it:
  `room_url_with_parts("ws://h/two-top", None, "forest")` → `"ws://h/two-top-forest"`
  (`room_code.rs:602-603`). In any room that pairs more than two, a third
  connection can end someone else's duel with five bytes.
- **`Profile`/`Profile2` are accepted at any time, repeatedly**, and
  `PeerProfile.install_id` is the key the ledger files the match under
  (`grudge.rs:313`). A peer can play the whole match as their real identity and
  re-send a `Profile` with a throwaway `install_id` a second before losing: the
  loss lands on a rival row that does not exist, their real rivalry stays clean.
- **The install-id is never proved, even though the key to prove it is already
  on the wire.** `net:193-199` says it is *"the only thing here that is ever
  trusted"* — and it is trusted on assertion. `Profile2` carries an ed25519
  pubkey, but the only thing ever signed is a `MatchStatement` at score-decided
  match end, and a rejected signature merely downgrades the result to unsigned
  while the ledger still records under the claimed id (`attest.rs:172-180` vs
  `grudge.rs:312-325`). So anyone can poison anyone's rivalry row: name, W/L,
  streak, and tape ring, keyed on an id they simply typed.

**Fix:** three small changes, in order of value. (a) Ignore any side-channel
message whose `from` is not the lobby's current `peer_id`. (b) Freeze
`PeerProfile`/`PeerKeys` after the first handshake of a session — later
`Profile` messages update the display name only, never `install_id`. (c) Add a
proof-of-possession to the handshake: each side signs the sorted session peer-id
pair with the key in its `Profile2`, and an install-id whose signature doesn't
verify is treated as a stranger (unsigned, unfiled) rather than as a rival.

---

## 9. 🟠 Losing TURN is silent, and the fallback is the config Ray can't use

The design is right: the APK carries a URL, the vendor holds the secret, the
credential is a throwaway. The failure behaviour is the problem.

```rust
// crates/app/src/netplay.rs:400-410
let Some(result) = outcome else { return; };           // still in flight, within budget
world.remove_resource::<PendingIce>();
...
let fetched = result.is_some();
let ice = result.unwrap_or_else(ice_server_config);
tracing::info!(target: "two_top::net", fetched, "ice config resolved");
open_socket(world, url, ice);
```

Timeouts are present and sane (2 s on the request, 2.5 s overall —
`:263`, `:309`), the parser is defensive and tested against garbage/empty/wrong
shape (`:290-300`, `:942-952`), and a timeout doesn't hang. That half is fine.

But `ice_server_config()` is the fallback, and on a public build it is
STUN-only by construction — the doc above it says so:

```rust
// crates/app/src/netplay.rs:186-189, 203-207
/// Public builds must never bake `TWOTOP_TURN_*` (extractable from any
/// distributed binary) — they carry `TWOTOP_ICE_URL` instead ...
let turn_url = get("TWOTOP_TURN_URL", option_env!("TWOTOP_TURN_URL"));
let mut config = RtcIceServerConfig::default();
let Some(url) = turn_url else { return config; };
```

…and the same file explains why that is exactly wrong for Ray:

```rust
// crates/app/src/netplay.rs:191-194
/// Why TURN matters: STUN-only traversal fails for phone pairs behind
/// carrier-grade NAT (very common on cellular). A TURN relay is the
/// fallback path that makes "two strangers on two networks" reliable.
```

So: vendor down or slow → 2.5 s later the socket opens with no relay → ICE never
completes → both phones sit at `AWAITING A CHALLENGER` indefinitely. The client
*knows* — it computes the fact and throws it away on a log line:

```rust
// crates/app/src/netplay.rs:417, 443-448
let has_turn = ice.urls.iter().any(|u| u.starts_with("turn"));
tracing::info!(target: "two_top::net", room = %url, turn_relay = has_turn, "matchbox socket built — connecting");
```

The player's only feedback is a generic hint after fifteen seconds
(`lobby_overlay.rs:65, 123-127`): *"if the other phone shows this too, the
networks may need the relay (TURN)"* — advice, not a diagnosis, shown
identically whether the relay is configured and healthy or was never fetched.

Compounding it, the vendor is a single-threaded server that makes a 5 s
blocking upstream call inside the request loop:

```rust
// crates/ice_vendor/src/main.rs:295
for request in server.incoming_requests() {
// :243-247
let body: serde_json::Value = ureq::post(endpoint)
    .set("Authorization", &format!("Bearer {token}"))
    .timeout(Duration::from_secs(5))
    .send_json(...)?.into_json()?;
```

One slow Cloudflare response blocks *every* queued `/ice` request (and
`/healthz`, which is exempt from the key and bucket but not from the queue,
`:296-299`). With the client's 2 s request timeout, the second concurrent match
entry during a Cloudflare hiccup is already falling back to STUN-only. The
module comment justifies single-threaded as *"the request rate is 'a duel is
starting somewhere'"* — true for volume, not for head-of-line blocking.

**Fix:** (a) tell the player. If `has_turn` is false, say so plainly on the
summoning screen ("playing without the relay — this may not connect on mobile
data") rather than after 15 s of dots. (b) Cache the last good ICE config on
disk with its `ttl_secs` (it is 4 h — `ice_vendor:48`) and prefer it over the
STUN-only fallback. (c) Thread the vendor, or at minimum move the Cloudflare
call off the accept loop and cache its result for the TTL — one upstream call
can serve every caller in that window.

---

## 10. 🟡 None of the above is testable, and the "verification" is a 0 ms-RTT rung

`crates/net` and `crates/app` have **no `tests/` directory** — the workspace's
integration tests live only in `fixed_math`, `sim`, `render`, `replay` and
`replay_sync`. Everything network-shaped is a `#[cfg(test)]` unit test of a pure
helper.

Those helpers pass, and that is the trap. `net:1170-1239` exercises
`next_lobby_state_for_silence` with hand-written frame numbers —
`(Connected, now = last_msg + 60)`, `(Disconnected, now = last_msg + 600)` — and
each assertion is correct. But per finding #2 the live driver **cannot produce
those arguments**, because the clock it reads is frozen whenever the condition
under test is true. The tests verify the arithmetic of a state machine whose
inputs never arrive. Same shape for
`reconnect_after_disconnect_fires_a_fresh_edge` (`:1153-1166`), which pins a
behaviour that `drive_netplay`'s `channel_taken` early-return makes unreachable.

Nothing exercises `drive_netplay`, `perform_swap`, `drain_session_events` or
`pump_side_channel`. And the manual gate is:

```
PLAYBOOK.md:87   ## Rung 2 — loopback netplay (no phone yet)
PLAYBOOK.md:24   | 2 | The netplay stack works (loopback, two processes) | no | local only |
```

Two processes on one machine: 0 ms RTT, 0% loss, no NAT, no handoff, no relay.
Findings 1, 2, 4, 5, 6 and 7 are all *invisible* in that environment by
construction — the prediction window is never approached, no rollback ever
contradicts a `MatchOver`, ggrs never interrupts, nothing ever desyncs. The
README's admission that *"the two-phone cross-carrier field test has not been
run"* understates it: there is no automated substitute either.

**Fix:** a headless two-`App` harness that drives two `P2PSession`s over an
in-process `NonBlockingSocket` with injectable latency, jitter and loss would
catch 1, 2, 5 and 6 without a phone or a signaling server. `MatchboxBridge`
already isolates the transport behind that exact trait (`net:144-156`), so the
seam is there — it just needs a second implementation. Add a Linux `tc netem`
lane over two loopback processes to cover the rest.

---

## Checked and not reported

Things I went looking for and could not make into a finding:

- **Panics on network-derived data.** The `expect`s in `perform_swap`
  (`netplay.rs:547, 554, 562, 565, 566`) are all reachable only once per socket
  and guarded by the `channel_taken` early-return; `take_channel` cannot be
  called twice. `decode_packet` and `decode_net_msg` both return `Option` and
  have refusal-path tests. The name decoder can't index out of range.
- **Peer-supplied string/allocation attacks.** Bounded by construction — the
  whole `NetMsg` enum is `Copy` with fixed-size arrays.
- **Signature forgery.** `MatchStatement::verify` rejects bad keys
  (`VerifyingKey::from_bytes` → `false`, `net:431-433`), bad hex
  (`Attestation::verify`, `net:514-523`), tampered scores, and swapped seats —
  all covered by `net:920-1010`. The canonical ordering makes both peers encode
  identical bytes. This part is sound.
- **Out-of-band `MatchState` on the ggrs `Disconnected` path specifically** — I
  expected it to be rolled back and it is not; ggrs performs its disconnect
  rollback inside the same `advance_frame` that queues the event, in `PreUpdate`,
  before `drain_session_events` runs. Only the `Bye` path is exposed (#4).
- **`Update`-schedule ordering ambiguity** between `drive_netplay` and ggrs — a
  non-issue: `GgrsPlugin::default()` runs in `PreUpdate`
  (`bevy_ggrs/src/lib.rs:217-225`), so all app systems observe post-advance state.
- **Double-recording via QUIT on the summary** — `quit_match` returns early on
  `MatchOver` (`screen.rs:1571-1576`) and the button is hidden (`:1661-1663`),
  so `record_abandoned_loss` cannot stack on `record_match_result`.
