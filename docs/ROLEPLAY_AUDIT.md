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

### Next

In the order the "where to start" list gives, minus what landed: the
lifecycle handler (P6 #2/#3/#5/#6 — sunset still kills the app), the
forfeit ledger (P2 #3 — record *unfinished*, not two wins), `Desync`
as a terminal state (P2 #5), the out-of-band `MatchState` write on the
`Bye` path (P2 #4), the rollback window (P2 #1), and ingest validation
plus socket timeouts on `tape_drop` (P4 #2/#3). The first sim-affecting
batch — the round scoring, the respawn taunt, the sudden-death respawn
margin — wants one `SIM_VERSION` bump and one matrix run together.
