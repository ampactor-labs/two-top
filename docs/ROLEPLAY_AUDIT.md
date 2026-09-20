# Roleplay Audit — seven seats at the table

`DESIGN_DIRECTION.md` audited the art. `GAME_DESIGN_AUDIT.md` audited the
match. This is the third pass and the first one taken from **outside the
code**: seven imagined players, each with a different reason to pick the
phone up, each played through the shipped build by reading what the build
actually does.

Method: one persona per seat, each tracing their own session through the
source and citing the line that proves every claim. No persona was allowed
to report a finding they could not land on a file and a line. Where a
persona expected a bug and the code turned out to be right, they said so —
those "checked and sound" sections are load-bearing, because the most
expensive thing an audit can do is send someone to fix something that
works.

The seven seats:

| | Persona | Seat | Report |
|---|---|---|---|
| P1 | Dani & Mo | two strangers, first sixty seconds | [`p1-first-time.md`](audits/personas/p1-first-time.md) |
| P2 | Ray | a real network, on a train | [`p2-network.md`](audits/personas/p2-network.md) |
| P3 | Sam | alone at 1am, the solo ladder and the metagame | [`p3-solo.md`](audits/personas/p3-solo.md) |
| P4 | Jules | posts clips, cares whether the proof is real | [`p4-share.md`](audits/personas/p4-share.md) |
| P5 | Tobi | plays with their thumbs | [`p5-feel.md`](audits/personas/p5-feel.md) |
| P6 | Priya | a phone that has other ideas | [`p6-lifecycle.md`](audits/personas/p6-lifecycle.md) |
| P7 | Vic | reads the rulebook before the tutorial | [`p7-competitive.md`](audits/personas/p7-competitive.md) |

**54 findings — 16 🔴, 25 🟠, 13 🟡** — collapsing to roughly 45 distinct
issues once the overlaps between seats are merged (the deep-link no-op,
the version-skew hole and the log-only desync were each found
independently by two or three personas, which is corroboration rather
than noise).

---

## The one-paragraph version

The game is built. The systems around the game are honest about what they
intend and mostly don't do it yet. Nearly every finding in this document
is one of two shapes: **a failure the player is never told about**, or **a
number two phones don't agree on**. The product's whole pitch — a rivalry
ledger, signed results, a tape you can post — rests on both phones telling
the same story, and right now two phones that played the same match can
both record a win, both show a different score, and neither can prove
which was real. The code knows: there is a `tracing::error!` at almost
every one of these points, aimed at a `logcat` no player will ever read.

---

## The pattern worth leading with

**In case after case, this repo already contains the correct fix, applied
somewhere else, with a comment explaining why.** That is the most
actionable thing seven independent readings turned up, and it means most
of this list is cheaper than it looks:

| The right answer, already shipped | Where it wasn't applied |
|---|---|
| `read_profile` quarantines a corrupt file and remints (`profile.rs:307-316`) | `load_career` silently returns a default, then overwrites the evidence (P3 #1) |
| The theater refuses a foreign-version tape *on screen* — *"the no-migrations law should read as a law, not as data loss"* (`theater.rs:159-162`) | RIVALS logs it and does nothing (P3 #8); the web theater boots the title screen (P4 #5) |
| The recorder waits 30 frames for predicted ticks to settle before writing (`recorder.rs:36-40`) | The ledger and the attestation both commit on the predicted edge (P2 #6) |
| *"No more invisible screen-half zones"* (`screen.rs:968`) | DEL/DONE is a full-width invisible band on the name keyboard (P1 #7) |
| `SpawnGuard` — *"so it can never be an offensive shield"* (`sim:182`) | Taunting doesn't break the guard (P7 #1) |
| `write_atomic` is used by **every** persisted file, with no bypass anywhere (verified independently by P3 and P6) | — this one is simply done right |

Where a persona proposes a fix below, check this table first: the pattern
is usually three lines away in a neighbouring module.

---

## Six cross-cutting themes

### 1. Failure is silent — the dominant theme, ~15 findings

The single most common shape in this audit. A thing fails, the code
notices, writes a log line, and the player sees nothing change.

- Signaling dies at ~6 s; the SUMMONING overlay vanishes and is replaced
  by **nothing**, forever, with no retry (P1 #3).
- The join deep-link no-ops whenever the app is already running — the one
  ritual the product is built around — with zero feedback (P1 #4, P6 #9).
- `DesyncDetected` is a `tracing::error!` and nothing else. Two players
  play two different games to two different conclusions and each records
  a result (P2 #5, P1 #5, P6 #7c).
- RIVALS offers ROLL rows and SPAR THEIR SHADE that are dead on tap after
  a version bump (P3 #8).
- A shared tape that fetches but won't decode boots twelve viewers into
  the title screen of a game they've never heard of (P4 #5).
- SHARE has no progress, no error, no retry, and a QR render failure
  leaves a blank interactive screen (P4 #7).
- A blocked `fetch` kills the whole web page with a raw `TypeError` (P4 #6).

**The fix is one shared mechanism, not fifteen.** `theater.rs`'s
`TapeNoticeState` already is it, and its comment already argues the case:
*"on a phone that is indistinguishable from a dead button… the REFUSAL
just becomes something the thumb can see."*

### 2. The ledger is not a shared fact

The rivalry home, the streaks, the "YOU LEAD 3-1" — none of it is a
number both phones agree on.

- A network drop makes **both** phones record a win for the same match
  (P2 #3). Neither ledger looks anomalous.
- Losing 1-4? Airplane mode banks a win and dodges the loss — no focus
  loss, no frame freeze, so the local absence check never fires (P2 #3).
- A 2-second thermal hitch or a pulled notification shade convicts you of
  walking out, for a 20-second window that nothing ever clears (P6 #4).
- Android killing the cached process makes the two ledgers disagree
  permanently — her phone records nothing, his records a win (P6 #5).
- A mid-match `Profile` rewrite files the loss under a throwaway
  install-id (P2 #8).
- The install-id is never proved, though the key to prove it is already
  on the wire (P2 #8).

And the mechanism that was supposed to settle this **proves less than the
docs claim**: both signatures verify against public keys carried inside
the statement being signed, keys minted locally with no enrollment, so
two `OsRng` calls forge any result (P4 #4a). There's no tape binding
either — a genuine attestation for "5-2 on the Pit" verifies against *any*
tape that re-simulates to 5-2 on the Pit (P4 #4b). `docs/NORTH.md:71`'s
"ranked play needs only a dumb relay collecting signed statements" is, as
written, a sybil faucet.

### 3. The app does not know the phone exists

Zero lifecycle handling anywhere in 13 crates — no `WillSuspend`, no
`onPause`, no `onNewIntent`, no `WinitSettings` (P6, verified by
repo-wide grep).

- A phone call past 9 s is an unrecoverable forfeit with no reconnect
  path in the codebase (P6 #2, P2 #7).
- The documented 10-second grace that was supposed to cover it is
  **counted in sim frames**, which stop advancing during the exact stall
  it measures — so it can never run (P2 #2, P6 #2).
- Android's scheduled dark theme flipping at sunset recreates the
  activity, re-enters `android_main`, and `init_logging()` panics on
  subscriber re-registration — a panic the code documents against itself
  (P6 #3).
- No `WAKE_LOCK`: the screen times out while you wait for a challenger,
  and the summons dies (P6 #6).

### 4. Unbounded stores behind fixed-size windows

- REPLAYS fully reads and fully decodes **every** tape on disk to draw 8
  rows, synchronously on the main thread — ANR territory within a year
  of normal play (P3 #5).
- Nothing ever deletes a tape. No cap, no rotation, no delete affordance.
  The escape hatch the screen offers is a printed filesystem path (P3 #5).
- Eight gauntlet runs push every real match off the REPLAYS screen (P3 #6).
- Rival #9 is unreachable forever — no detail view, so their tape ring
  can't be played and their shade can't be summoned (P3 #6).
- Logs rotate daily with no `max_log_files` (P6 #10c).

### 5. Version skew is undefended at four layers, and defended beautifully at the fifth

Tapes are strictly version-matched with a refusal the player can see.
Everything else pairs happily and diverges:

- No `version_code` anywhere, and the APK ships from a *rolling* tag, so
  Android's downgrade protection never engages (P6 #7a).
- Persisted JSON has `#[serde(default)]` (good backward compat) and **no
  forward compat** — an old build reads a new file fine and writes it
  back with the new fields gone. The casualty is `signing_key`, which
  orphans every attestation the player holds (P6 #7b).
- The room name carries code + arena, never `SIM_VERSION`, so mismatched
  builds can see each other (P1 #5, P6 #7c).
- Every `SIM_VERSION` bump silently breaks every share link already
  posted — and `GAME_DESIGN_AUDIT` #1 is already asking for a bump (P4 #5).

**One line fixes most of it:** put `SIM_VERSION` in the room-name suffix
and incompatible builds simply never pair.

### 6. Verified on a LAN, shipped to a train

- The rollback session takes ggrs's **default 8-frame prediction window**
  — never raised — while a 300 ms cellular RTT is 9 frames one way. The
  session lives pinned against the stop (P2 #1).
- `WaitRecommendation`, ggrs's own fix for clock skew, is logged at
  `debug!` and discarded (P2 #1).
- CI's SyncTest runs `check_distance: 7`; the live prediction window is 8.
  **The deepest rollback the product can perform online is one frame
  deeper than anything CI has ever verified** (P2 #1).
- `crates/net` and `crates/app` have no `tests/` directory at all. The
  manual gate is two processes on one machine: 0 ms RTT, 0% loss, no NAT,
  no relay — an environment where findings 1, 2, 4, 5, 6 and 7 are all
  invisible *by construction* (P2 #10).

---

## Where to start

Ordered by (damage × reachability) ÷ cost, across all seven seats.

**Now — cheap, and each closes a crash or a lie:**

1. **A stranger's packet kills the app.** ggrs `expect`s on per-player
   input bytes; `decode_packet` validates only the envelope. The APK
   pairs on the public quick-match room by default, so the sender is any
   stranger. The repo already documents the hole as upstream and owns the
   right seam to close it (P6 #1). 🔴
2. **Scrubbing a `frame_count <= 1` tape panics in release.** One-line
   codec assert. Reachable from any traded or web-linked tape, and the
   codec has a test that *guarantees* such a tape round-trips (P3 #2). 🔴
3. **`SIM_VERSION` in the room name.** One line; closes most of theme 5. 🔴
4. **Arena pick silently partitions the room**, and then the app blames
   the network. Two phones in different rooms render byte-identical
   waiting text. This is the single most likely way a first session dies
   (P1 #1). 🔴
5. **The XFF rate-limit bypass** in `tape_drop` *and* `ice_vendor` — one
   line each, and every other bound in that service depends on it
   (P4 #1). 🔴
6. **Quarantine a corrupt `career.json`** the way `profile.json` already
   is. Minutes (P3 #1). 🔴

**Next — the structural ones, in dependency order:**

7. **Lifecycle handling.** P6 #2/#3/#5/#6 and P1 #4 all unblock from one
   handler. Start with `config_changes` (a manifest line) and an
   idempotent `init_logging` (P6 #3), because sunset currently kills the
   app.
8. **Stop recording a result nobody can prove.** Record a forfeit as a
   third outcome — *unfinished* — that moves neither W/L nor streak
   unless the departing side sent a signed concession. This makes the
   ledger's silence honest instead of making both ledgers confidently
   wrong (P2 #3), and it is a precondition for theme 2 meaning anything.
9. **Make `DesyncDetected` terminal and visible**, skip the ledger write,
   stamp or skip the tape (P2 #5).
10. **Don't write `MatchState` from `Update`.** The `Bye` path writes a
    rolled-back resource out-of-band, against `CONVENTIONS.md:14`'s own
    worked example, and a rollback un-ends the match and double-records
    it. The canonical pattern (`sim::apply_rematch`) is already in the
    repo (P2 #4).
11. **Rollback config for a real network** — prediction window, input
    delay, and honouring `WaitRecommendation` (P2 #1). Pure netcode, no
    `SIM_VERSION` bump.
12. **Tape-drop ingest validation + socket timeouts** — together they
    turn an open anonymous file host into a tape relay (P4 #2, #3).

**Then — the ones players feel weekly:**

13. A recorder ring + a DUELS/PRACTICE filter on REPLAYS (P3 #5b, #6).
14. The gauntlet's three missing rules: quit penalty, tier cap, reset
    floor. All single-expression edits (P3 #3, #4, #10).
15. The free respawn taunt — one line, plus deciding which of two
    contradictory doc comments is the truth (P7 #1).
16. A shared refusal-notice mechanism, applied everywhere in theme 1.
17. Tape hash in `MatchStatement`, and rewrite `NORTH.md:64-72` to claim
    what the signature actually proves (P4 #4b).

---

## Honest limits of this audit

- **Everything here is read, not run.** No persona executed the app. The
  cross-platform determinism matrix has not been run against these
  findings, and per `CLAUDE.md` that means any determinism claim in them
  is unverified.
- **Two seats are thinner than the other five.** P1–P4 and P6 were each
  written by a dedicated agent with a full sweep of their area. P5 and P7
  were written inline after that run was cut off by an API rate limit,
  and each covers roughly half its brief. Both name their own gaps at the
  end. The largest unswept areas are **the game-feel layer** (shake,
  flash, kill-cam, hit-stop, audio, haptics) and **the pickups and the
  six modifier behaviors** — the latter being, in a one-hit-kill sim,
  where the next free move most likely is.
- **Three findings are explicitly SPECULATIVE** as to consequence and are
  marked as such in place: the Android Back gesture (P1 #8 — could be 🔴
  if it finishes the activity mid-summons), the in-process
  `ANativeActivity_onCreate` recreation step (P6 #3), and the wasm bundle
  size (P4 #8). Each names what could not be determined without hardware
  or a build.
- **The test suite was not run for this audit.** Two attempts failed on
  the session's disk allowance — the linker died with `SIGBUS` writing to
  a full filesystem, the same failure `determinism.yml` already documents.
  The 570-test figure in `CLAUDE.md` matches a static count of `#[test]`
  functions; CI is the place that signal comes from, not this audit.

---

## Status

First pass, 2026-09-17: the six "now — cheap" items plus the packet
panic. Six carry a test written against the failure; the lobby copy has
no unit seam and was read, not run. Verified by
`cargo test --workspace --locked` (586 passed, 0 failed),
`cargo clippy --workspace --all-targets --locked -- -D warnings` and
`cargo fmt --all --check`, all run in this session with their real exit
codes captured. **The cross-platform determinism matrix has not run**;
none of these touch the sim, so no `SIM_VERSION` bump was taken, and the
canonical demos and their checksums are untouched.

| Finding | Fix | Proof |
|---|---|---|
| P6 #1 — a peer's packet panics the process inside ggrs | A wire-format guard in `crates/net` mirrors ggrs's `Message` layout, walks an `Input` stream the way ggrs's decoder will, and refuses any frame that is not exactly one `PlayerInput` wide. The RLE layer underneath is vetted first with checked bounds (see corrections below). | `net::input_stream_tests` — the mirror's bytes must decode as a real `ggrs::Message`, or the guard is inert |
| P3 #2 — scrubbing a `frame_count ≤ 1` tape panics in release | `replay::decode` refuses a header whose `frame_count` disagrees with the input count (`FrameCountMismatch`); the theater additionally skips scrubbing under two frames | `replay/tests/codec.rs::frame_count_must_match_the_inputs` |
| P1 #5 / P6 #7c — mismatched builds pair and desync | `sim::SIM_VERSION` rides the room name (`two-top-CURS-pit-v14`); two builds never see each other. `PLAYBOOK.md` and `SIGNALING.md` updated | `room_code::different_sim_versions_never_share_a_room` |
| P1 #1 — arena pick silently partitions the room, then blames TURN | The waiting overlay names the table on both paths, and the 15 s stall hint leads with "check both picked the same table" | UI copy; no unit seam |
| P4 #1 — XFF rate-limit bypass in `tape_drop` and `ice_vendor` | One shared shape in both: `TRUSTED_PROXY_HOPS` (default 1, Railway's edge) picks the entry the outermost trusted proxy appended, never the client's leftmost; `0` ignores the header outright. The inverted comment is gone | `forwarded_ip_tests` in both services |
| P3 #1 — a corrupt `career.json` is silently replaced | `paths::quarantine_corrupt` — one helper, now used by both `profile.json` and `career.json` — moves the bytes aside as `.corrupt` and logs at `error!` | `grudge::a_corrupt_career_file_is_quarantined_not_overwritten` |

### Second pass, 2026-09-17

The structural tier that needs no `SIM_VERSION` bump. Verified the same
way (593 passed, 0 failed; clippy `-D warnings`; fmt). Where the
proof column says *read*, there is no unit seam and no two-peer harness
(P2 #10 still stands): the mechanism was traced, not exercised.

| Finding | Fix | Proof |
|---|---|---|
| P2 #4 — the `Bye`/forfeit paths write `MatchState` out-of-band; a rollback un-ends the match and double-records it | Terminal lobby states (`Forfeited`, `Desynced`) drop the ggrs `Session` in `PostUpdate` the same frame — no session, no rollback, the write stands. bevy_ggrs fetches the session with `get_resource_mut`, and the Title already runs sessionless | `net::terminal_is_forfeit_or_desync_only`; mechanism read |
| P2 #5 — a desync is a log line and nothing else | `LobbyState::Desynced` is terminal: the summary reads THE TWO PHONES STOPPED AGREEING / this one doesn't count, and the ledger, the attestation, the recorder and the rematch gate all skip it | state pinned in `net` tests; skips read |
| P1 #3 — signaling death parks the lobby at `Idle`, which renders nothing | `LobbyState::SummonFailed`, rendered: COULDN'T REACH THE ROOM SERVER / check this phone's connection / then CANCEL and try again; PLAY THE BOT arms at once | `net` tests; copy read |
| P1 #2 — a double-tap on FIND OPPONENT lands in a bot match | PLAY THE BOT arms only after `BOT_OFFER_DELAY_SECS = 5` of waiting (at once on `SummonFailed`); the fled-opponent summary offers it as before | `screen::bot_offer_tests` |
| P6 #4 — a 2 s hitch or a pulled notification shade convicts this phone of walking out; never cleared | An absence is a freeze ≥ `DISCONNECT_TIMEOUT`, the only freeze that can forfeit us on the other phone; `WindowFocused` is ignored; cleared on `OnEnter(InMatch)` and on teardown | `netplay::only_a_freeze_the_peer_would_time_out_counts_as_absence` |
| P3 #3 / #4 / #10 — quitting keeps the tier; the counter climbs past the bot; a loss resets to the dummy | `GAUNTLET_MAX_TIER = 10` (where the bot's last knob saturates), labelled MASTERED; a loss or a live-match quit falls to `loss_tier(best)` — two rungs under the best, never the dummy again | three `grudge` tests |
| P6 #3 — dark theme at sunset recreates the activity, re-enters `android_main`, and `init_logging` panics on re-registration | `try_init` in every branch, the panic hook installs once, and the manifest absorbs uiMode, density, fontScale, smallestScreenSize, screenLayout, locale and the rest | `logging::init_logging_twice_does_not_panic`; the manifest is unverified on a device |

### Third pass, 2026-09-17

The netcode tier and the ledger's honesty, still with no `SIM_VERSION`
bump. Verified the same way (598 passed, 0 failed; clippy
`-D warnings`; fmt). Same caveat: no two-peer harness, so the netplay
mechanisms are traced, not exercised.

| Finding | Fix | Proof |
|---|---|---|
| P2 #3 — both phones record a WIN for a match a network drop ended; airplane mode banks a win | `Forfeited` carries `conceded`; `grudge::match_outcome` is three-way. A goodbye is a concession (the leaver filed the loss first), our own freeze ≥ `DISCONNECT_TIMEOUT` is our loss, and a silent drop is **unfinished** — a meeting, but nobody's win, nobody's loss, streak untouched. The summary reads CONNECTION LOST; the tape's winner is `None`; RIVALS counts it | `grudge::only_a_goodbye_concedes_and_only_our_own_freeze_loses` and two more; `screen::a_silent_drop_crowns_nobody` |
| P2 #6 — the ledger and the attestation commit on a *predicted* `MatchOver` | `attest::MatchOverSettled` carries both edges: `over` (the raw `MatchOver`, which a rollback can still undo) and `settled` (the session's confirmed frame has passed the frame `MatchOver` was first seen at — at once when there is no P2P session to roll anything back). The ledger and the signer commit on `settled`; `tape_before` is captured on `over` so the tape/attestation pairing survives either order of save and settle | `attest` test still signs on tick 1; mechanism read |
| P2 #1 — the rollback session runs at ggrs's LAN default of 8 predicted frames | `with_max_prediction_window(16)`, `ONLINE_INPUT_DELAY` 2 → 3, and the CI SyncTest / `sync_test` default raised from 7 to 16 so the deepest live rollback is a depth CI verifies (ggrs requires `check_dist < max_prediction` strictly, so those sessions size their window one frame above the depth) | `determinism_locked_600_frame_synctest` at depth 16 |
| P2 #8 — the side channel trusts any sender; a mid-match `Profile` rewrites the ledger key | `netplay::side_channel_verdict`: a message not from the paired peer is ignored; a `Profile` whose install-id differs from the one on file is refused | `netplay::the_side_channel_trusts_only_the_paired_peer_and_its_first_identity` |
| (self-review) — the summary card and the ledger disagreed about a decided score whose link then died | `summary_text`'s unfinished branch now tests `!threshold_hit`, matching `grudge::match_outcome`, which checks the score before the forfeit. Found by re-reading the batch's own diff, not by a test | `screen::a_decided_score_still_crowns_the_winner_even_if_the_link_then_dies`, which also asserts the ledger agrees on the same inputs |
| P4 #2 — `tape_drop` is an anonymous 64 KB blob host | Ingest requires the `BMRG` magic and a minimum length (postcard puts the `[u8; 4]` first, unprefixed); GET is metered like POST; every response carries `X-Content-Type-Options: nosniff` | `tape_drop::ingest_tests` |

### Fourth pass, 2026-09-17 — the sim batch (SIM_VERSION 15)

The first changes in this program that touch the simulation, taken
together under one bump so they share one matrix run. Verified by
`cargo test --workspace --locked` (602 passed, 0 failed), clippy
`-D warnings`, fmt. The canonical demo and its checksum TSV were
regenerated (`gen_canonical --write`), as `CONVENTIONS.md` § replay
requires on any bump. Note what that regeneration showed: the demo's
checksums come back **byte-identical** across the bump. It runs 1800
frames, the round expires at 1980, and it presses no TAUNT — so it
crosses no boundary and takes no taunt, and `CatchStreak` is not a
checksummed column regardless. The canonical demo therefore exercises
none of this batch; the dedicated sim suites do, but the MATRIX does
not. That is a real coverage gap, worth closing with a demo long enough
to cross a boundary. **The cross-platform matrix has still not run
here** — that is CI's job, and until it does, the determinism claim for
this bump is unverified on every target but linux-x64.

| Change | Why | Proof |
|---|---|---|
| `reset_round_state` no longer wipes `CatchStreak` | The headline. The match ends on kills, checked identically in `InRound` and `RoundOver`, so the boundary decides nothing — and the one piece of state it touched was the perfect-catch ladder. The clock's only gameplay effect was punishing whoever was playing best | `match_state::the_round_boundary_no_longer_confiscates_the_catch_streak` |
| Taunting breaks `SpawnGuard` (P7 #1) | `TAUNT_FRAMES` (42) fits inside `SPAWN_GUARD_FRAMES` (45), so a taunt begun on the respawn tick completed entirely inside invulnerability — a free streak tier every death, against the guard's own promise that it "can never be an offensive shield" | `taunt::taunting_on_respawn_forfeits_the_guard_instead_of_hiding_behind_it` |
| `SUDDEN_DEATH_MIN_FACTOR` 0.4 → 0.45 (P7 #2) | At 0.4 the crumbled floor's half-height was `750 × 0.4 = 300`, exactly the respawn points' \|y\| — and in I16F16 a hair outside. Widening the floor rather than moving the spawns keeps the opening duel distance untouched | `respawn::respawn_points_stay_inside_the_crumbled_floor` |
| The boundary costs 1.5 s, not 4.0 s | Half the `RoundOver` beat, one countdown digit mid-match (the match's first countdown keeps the full 3‑2‑1). 150 frames per boundary handed back to play | `match_state::the_round_boundary_costs_a_beat_not_four_seconds` |

One more thing this batch fixed, found by running the suite more times in
a day than it usually sees: **`render`'s two `depth_projection_*` tests
raced.** Both write one process-global — one publishes the linear
fallback, the other a perspective span — and cargo runs a binary's tests
on parallel threads, so the loser read the winner's value. It failed as
`75 vs 75` (75 = `100 × WORLD_TILT_Y`, exactly the fallback). It
predates this whole program; `git diff` on `crates/render/` is empty in
both directions. The pair is now serialized behind a poison-tolerant
lock, proven over 20 consecutive parallel runs. It matters beyond the
one test: while it was live, **any** single green run on this repo had a
chance of being green for the wrong reason.

Two ship blockers went with it, both in the Android manifest and neither
sim-affecting: `debuggable` is now **false** (it shipped `true` in the
public release APK, so `adb shell run-as` could read `profile.json` — the
ed25519 seed the whole signed-results pillar rests on), and the build
carries an explicit monotonic `version_code`/`version_name` so Android's
downgrade protection engages instead of letting an older APK reinstall
over a newer one and write `profile.json` back without `signing_key`.

### Corrections to the record

Things this pass found while fixing that the reports above got wrong or
missed. Recorded here because a reader working from the persona files
would otherwise verify the wrong thing.

- **P6 #1's mechanism was wrong in detail, and the hole is wider than it
  said.** The panic is not "5 raw bytes trips the assert"; a 2-Top peer
  speaks for one handle, so ggrs's `assert!(len % handles.len() == 0)`
  is `len % 1` and never fires. The reachable panic is the delta
  decoder (`ggrs/src/network/compression.rs`): each frame's width comes
  from a two-byte length prefix the *remote* wrote, so a stream decoding
  to a 3-byte frame hits `expect("input deserialization failed")` in
  `to_player_inputs`. Underneath that, `bitfield_rle` (via `varinteger`)
  reads `buf[off]` past the end of a stream ending on a continuation
  byte, and sizes its output allocation from the remote's repeat count —
  a 1 GiB `vec!` from five bytes. All three are closed by the guard.
  **One remains, and is not closable at this boundary:** protocol.rs
  also `assert!(last_recv_frame + 1 >= body.start_frame)` on the wire's
  own `start_frame`; checking it needs ggrs's private receive
  bookkeeping. A hostile peer can still trip it mid-match. The honest
  fix is a warn-and-drop in ggrs itself — filed as a follow-up, not
  fixed.
- **`GAME_DESIGN_AUDIT.md` § Not findings says `SpawnGuard`'s
  break-on-act rule "closes the obvious offensive-shield exploit."
  P7 #1 shows the taunt is the exception** — taunting is not in the
  break list, `TAUNT_FRAMES = 42` fits inside `SPAWN_GUARD_FRAMES = 45`,
  and the payout lands with three guard frames to spare. Not fixed in
  this pass: it is a one-line sim change, which means a `SIM_VERSION`
  bump and a matrix run, and it should ride the same bump as
  `GAME_DESIGN_AUDIT` #1 rather than take one of its own.
- **P2 #4's prescription does not exist.** It named `sim::apply_rematch`
  as "the pattern the codebase already documents as canonical — a
  non-rolled-back flag consumed by a `GgrsSchedule` system."
  `apply_rematch` is input-driven: it restarts on a THROW rising edge
  derived from the rolled-back input history, exactly as CONVENTIONS §4
  requires, and no `RematchRequested` resource exists anywhere. A forfeit
  has no inputs to derive an edge from — the peer is gone — so the honest
  fix is not to route it through the sim at all, but to make the
  out-of-band write unrollbackable by dropping the session. The finding
  was right; the fix it proposed was not.
- **`GAME_DESIGN_AUDIT` #1's framing was half wrong, and it is worth
  saying plainly because it drove this program's priority list.** "The
  round does not score" reads as a defect; it is a design choice, and the
  code says so — `MATCH_WIN_THRESHOLD`'s own comment states that the
  scoring rule is first-to-5-kills and that "the round timer still
  rotates state for input-gating and future cleanup pulses, but doesn't
  independently end the match." In a one-hit-kill game with a 3 s
  respawn, the kill IS the dramatic beat; rounds would double-count it
  and charge dead time for the privilege. The real defects were narrower
  and are what SIM_VERSION 15 fixes: the boundary confiscating the
  skill ladder, and costing 4 s to decide nothing. The scoring rule was
  never the bug.
- **The two-phone field test HAS been run** — Wi-Fi against mobile data,
  at an earlier revision, by the operator. `README.md` said flatly that
  it had not. It now says what is true: verified once, then drifted, with
  the netplay layer substantially changed since. That distinction matters
  for how much the online path can be trusted.
- **P2 #1 overstated "`WaitRecommendation` is discarded."** The event is
  logged and unused, but bevy_ggrs already corrects the skew it reports,
  continuously: the clock runs 10% slow for as long as
  `session.frames_ahead() > 0` (`bevy_ggrs/src/schedule_systems.rs`).
  What was actually missing was the prediction window, which is now 16.
- **P4 #3's socket timeouts cannot be done with `tiny_http` 0.12.** It
  exposes only `recv_timeout` on the accept loop; a client that trickles a
  body holds the request thread with no deadline the library can set. A
  worker pool would bound the damage to the pool size, not remove it.
  Still open; it needs a different server.

### Next

What is left, in value order: the lifecycle handler proper (P6 #2/#5/#6
— a phone call is still a 9 s forfeit with no reconnect, and there is
no `WAKE_LOCK`), a two-peer in-process harness so the netplay mechanisms
above stop being "read, not run" (P2 #10), the tape hash in
`MatchStatement` (P4 #4b), a recorder ring and a DUELS/PRACTICE filter
on REPLAYS (P3 #5/#6), the shade's honesty (P3 #7/#8), and a server for
`tape_drop` that can set a deadline (P4 #3). The first sim-affecting
batch — the round scoring, the respawn taunt, the sudden-death respawn
margin — wants one `SIM_VERSION` bump and one matrix run together.

---

## Playtest round 1 — the operator on a real phone

Three findings from the first sideload of the shipped APK. Two were
cosmetic, one was a hang. All three were reproduced here before being
fixed, and the two visual ones were verified with headless captures
(`TWOTOP_CAPTURE` + `TWOTOP_AUTOSTART`, lavapipe under Xvfb) rather than
argued about — which is how the QR's first bad placement was caught too,
and how this round caught an audit theory that was simply wrong.

### 🔴 The gauntlet bot froze on the spot, strobing

Not a render bug. `bot_decide`'s housekeeping branch walked at a dropped
fang **unconditionally**, and it sits above both the recall and the
throw. But every movement intent the policy emits runs through `steer`,
which refuses to cross the edge cushion (`edge_safe`, 82% of the safe
bounds) or enter cover's padded ring. A fang that settles in either
place — and a fang knocked Loose near the rim settles there often — is
one the bot is permitted to want and forbidden to reach. The wanting and
the refusing alternate frame by frame, so the bot oscillated around the
cushion boundary: stuck in place, and strobing, because the sprite's
facing row is chosen from the velocity sign. It also never threw again
for the rest of the round, because the branch it was trapped in preempts
the throw. Up to 30 s of punching bag, every time it happened.

The fix is not a bigger cushion. A Loose fang is **hold-recallable** —
`recall_boomerangs` turns it Returning on a THROW press edge when the
owner has no free slot — so the unreachable case has a clean answer that
was already in the sim. The walk now runs only when the fang is
genuinely reachable (inside the box the retrieval walk is allowed to
enter, clear of every padded ring); otherwise the bot pulses THROW and
reels it home while it keeps orbiting. The retrieval walk also gets its
own, looser edge margin, so a fang in the cushion is reachable rather
than merely wanted.

Sim-neutral: the policy is an input source, not sim state. No
`SIM_VERSION` bump.

### 🟠 The settings screen read as floating words

Six bare centered strings over the live table, with `<` and `>` buried
mid-string while the tap zones are the entire left and right screen
halves — so the glyph you aim at is not the thing you hit, and nothing
said the rows were controls at all. The group headers sat almost exactly
equidistant between the group above them and their own rows (0.060 vs
0.072 of screen height), so each one captioned whichever group your eye
reached first.

Rows are now bordered boxes in the same language as every other control
in the game, with the arrows as their own entities parked at the box's
ends (world-unit offsets, not normalized fractions — a fraction would
slide them off a fixed-width box on any aspect but the phone's). The
header gaps are now 0.109 before and 0.049 after.

*Correction to this document's own first pass:* the rows were diagnosed
here as "the string wraps onto three lines." They do not.
`Text2d`'s `TextBounds` defaults to `UNBOUNDED`, so nothing in this
screen has ever wrapped. The capture is what settled it.

### 🟠 The join QR was unscannable

110 world units ≈ 100 px on a 1080-wide phone: about three pixels per
module, which no camera holds focus on across a table. The sit-down
ritual's entire display half was therefore decorative. Now 330 units
(~10 px/module) with a `SCAN TO JOIN` caption — the ritual asks a
stranger to point a camera at a square of noise, so it should say so.
Clearance from the portrait and the screen edge verified at 1080x2400
and 1920x1080.

### 🟠 Nothing local could catch the manifest error that broke the release

The APK job runs only on a push to `main`, and nothing else in the tree
reads `[package.metadata.android]` — so the cargo-apk panic that killed
the first publish (`version_name` set in a table cargo-apk derives
itself) was undetectable until after the merge. `scripts/check_android_
manifest.py` now runs on every branch and PR (`ci.yml`, `manifest` job):
the parse-time rules cargo-apk enforces, plus the two ship blockers that
can ride an otherwise-green build — `debuggable = false`, and no
`TWOTOP_TURN_*` baked into the public APK. Each gate was checked against
a deliberately reintroduced regression before landing, including the
exact one that broke the release.

---

## The browser build, and what it takes to reach an iPhone

An iPhone has no APK to sideload, and a native iOS port needs a paid
developer account, a Mac, and a port of every platform seam the Android
build owns. The wasm build already deployed to Pages is the whole game,
already online — so the question was never "how do we reach iOS", it was
"why has nobody played it in a browser". Four reasons, all found by
reading the code against a phone's constraints rather than a desktop's.

### 🔴 The web build drew no touch controls

`TouchControlsPlugin` gated its `shown` flag on `cfg!(target_os =
"android")`. `InputTouchPlugin` is registered unconditionally, so on a
phone browser the stick, throw and dash **worked** — they were simply
never drawn. On Android that gate is invisible; on a desktop browser the
keyboard covers for it. On an iPhone there is no keyboard and no visible
control, which is an unplayable game that looks like a broken one. The
gate now names the real condition: any target whose only input device is
a finger.

### 🔴 Nothing persisted in a browser, ever

`paths::config_dir` fell through to `dirs::config_dir()` off Android, and
on wasm32 that is `None` — every `std::fs` call on that target returns
`Unsupported`. So every save silently did nothing and every load came
back empty: a fresh install-id minted on **every page load**, the name
grid every visit, an empty rivalry ledger, and settings that reset
between rounds. The rivalry pillar cannot exist on a platform with no
memory.

The fix keeps the four persisted documents (profile, settings, room code,
career) in their exact native shape and changes only the floor under
them. Three seams — `read_document`, `write_atomic`, `quarantine_corrupt`
— now dispatch to `localStorage` on wasm, keyed under a `two-top/`
namespace because a GitHub Pages origin hosts a whole account. The
corrupt-document promise is kept too: bad bytes move to a `.corrupt` key
rather than being dropped. Every access degrades to in-memory-only rather
than panicking, since Safari in private browsing and any "block all
cookies" setting make `localStorage` throw rather than exist.

Tapes and crash logs stay unavailable on the web (`shared_dir` is `None`
there, and now says so explicitly): they are files a human opens with a
Files app, and a page cannot put one there unprompted. The REPLAYS screen
is empty in a browser by construction.

### 🟠 No Add to Home Screen

No manifest, no `apple-touch-icon`, no `apple-mobile-web-app-capable` —
so the closest thing the web build has to an install did nothing. There
are now generated icons (`scripts/generate_web_icons.py`, built from the
duelist sheet and the locked palette so they cannot drift off either),
a manifest for Android/desktop, and the `apple-*` tags iOS reads instead
of it. The viewport also stops double-tap zoom, and the canvas sizes in
`dvh` — iOS Safari's `vh` is the *tallest* the viewport ever gets, so
`100vh` put the dash button underneath the URL bar.

### 🟡 The bundle, and a number this document got wrong

The first pass here called the wasm "28 MB, brutal on cellular". That is
the **uncompressed** size. Pages serves it gzipped, and the byte count a
phone actually waits for was already 6.9 MB — checked with a request,
which is what the first claim should have been.

The deploy now runs `wasm-opt -Oz`, and the measured result corrects the
framing a second time: 28.5 MB → 22.1 MB raw, but only 6.94 MB → 6.78 MB
gzipped. The **download** barely moves. The raw size is the actual
payoff — that is what the browser decompresses, parses and holds, and
iOS Safari discards tabs over precisely that. 6.4 MB less peak memory on
an iPhone justifies the pass; 160 KB less transfer would not have.

The pass is shared with the wasm determinism lane
(`scripts/wasm_opt.sh`), so the 1800-frame headless checksum probe
validates the exact bytes a visitor receives. Run here before landing:
`CHECKSUMS-OK 1800 frames` against the optimized module in headless
Chromium — the optimizer provably does not change the simulation.

### Still unverified

Nobody has pointed a real iPhone at the deployed URL. Everything above is
a fix for a defect found by reading, and the checksum probe proves the
browser build is the same game — but iOS Safari's WebGL2 and WebRTC
behaviour under a real finger is not something this repo can test from
CI. That is the next thing worth doing, and it costs nothing.
