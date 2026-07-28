# NORTH_PLAN — executing the ultimate form

The phased build plan for [`docs/NORTH.md`](../NORTH.md). NORTH holds the why;
this holds the what, the order, and the gates. The style follows
BUILD_PLAN/COMPLETION_PLAN: phases with produces and exit criteria, one phase
landing before the next begins.

**Headline property: every phase is sim-neutral.** `SIM_VERSION` stays 14 for
the whole program; the ggrs wire format and the `sim` crate are untouched.
The determinism matrix keeps running as the standing gate, never as a
migration.

## Decision log (locked 2026-07-28)

1. **Demon mint v1 is the composed mint**: rune (8 baked sheets, shipped) x
   runtime accent-hue palette swap x 8 baked kill-sting WAVs, about 512
   identities. Geometry axes through the generator's `Build` class are v2, on
   the horizon.
2. **Tape sharing goes through a drop relay first** (persistent links;
   `ice_vendor`-shaped service). P2P live-spectate rides the same viewer
   later.
3. **Order after bedrock: proof, then PWA, then rivalry.** Keys and signed
   results while the side-channel work is warm; the wasm lane and web theater
   next because every later phase inherits the share loop; then the rivalry
   home, demons, shades, ritual.
4. **The server-side half of pillar II stays on the horizon.** This plan ends
   at peer-signed, locally stored, tool-verifiable results.

## Milestone map

| # | Phase | Produces | Sim-affecting? |
|---|-------|----------|----------------|
| N0 | Write it down | NORTH.md, this plan, doc pointers | No |
| N1 | Bedrock | Atomic persistence, packet refusal, doc truth, fmt+audit CI, ice_vendor hardening, APK strip | No |
| N2 | Keys and signed results | ed25519 identity, dual-signed statements, `.attest.json`, `replay_sync --attest` | No |
| N3 | wasm lane + web theater | wasm32 checksum gate, `web_theater` crate, `tape_drop` relay, share QR | No |
| N4 | Rivalry home | Rivals screen, ledger extensions, milestone flourish, pair-seeded scars | No |
| N5 | Minted demons v1 | Runtime hue mint, sting variants, extended art gates | No |
| N6 | Shades | Tape-fitted `BotStyle`, honest-gauntlet rules | No |
| N7 | Sit-down ritual | Join links, Android intent plumbing, QR on the dial | No |

## Phase N0 — write it down

**Produces:** `docs/NORTH.md`; this file; a pointer line in `CLAUDE.md`'s
status paragraph and in README's design-docs section.

**Exit criteria:** both docs committed; the prose linter's hard tier passes;
no code changed.

## Phase N1 — bedrock

The 2026-07-25 upgrade report, executed. Each item is its own commit.

1. **Atomic persistence.** `paths::write_atomic(path, bytes)`: write a `.tmp`
   sibling in the same directory, `std::fs::rename` over the target (atomic
   on the same filesystem on Linux/Android; no new dependency). Re-inventory
   every persistence write at execution time (the report names
   `profile.rs:258`, `settings.rs:117`, `grudge.rs:169`, `recorder.rs:226`;
   `room_code.rs` also writes) and route every site. Corrupt-file loads keep
   a `.corrupt` sibling instead of silently minting a fresh identity.
2. **Malformed-packet refusal.** `net::decode_packet` returns `Option`; drop
   undecodable packets with a `tracing::warn!`; `receive_all_messages` uses
   `filter_map`. The existing silence FSM scores the abuser as the walk-away.
   The residual ggrs-internal `expect` on per-player input bytes is an
   upstream issue, documented, not fixed here.
3. **Doc-truth batch.** The report's five stale claims (README test count and
   sim_version, `CONVENTIONS.md:55`, the throw-speed doc comment in
   `sim/src/lib.rs`, ARCHITECTURE's workflow list). Re-verify each against
   live code first; grep for the old numbers after.
4. **rustfmt.** `cargo fmt` as its own behavior-neutral commit, then a
   `cargo fmt --check` step in `ci.yml`.
5. **Advisory scan.** A weekly `cargo audit --locked` workflow that files an
   issue rather than failing PRs, so the deliberate version pins move on
   evidence instead of noise.
6. **ice_vendor hardening.** Per-IP token bucket
   (`BTreeMap<IpAddr, (u32, Instant)>`), an optional `X-App-Key` header baked
   into the APK, `/healthz` exempt for the Railway healthcheck.
7. **APK strip.** Measure `libapp.so` unstripped vs `llvm-strip`; add
   `strip = "symbols"` to `[profile.release]` if the delta is meaningful; one
   device install-and-play per SIDELOAD.md.
8. **Clutter.** Delete `logcat.txt`, `.tmp/`, `.tmpdir/` (untracked).

Deliberately skipped: the report's #8 monolith split. Revisit only if N4's
screen work makes `screen.rs` unmanageable.

**Exit criteria:** full gate green (nextest, clippy `--all-targets`, the new
fmt step); loopback smoke per PLAYBOOK Rung 2; a curl flood against
ice_vendor throttles; the doc grep comes back clean.

## Phase N2 — keys and signed results

A score-decided online match produces a dual-signed, independently verifiable
result artifact. No servers.

- `ed25519-dalek` (pinned) in `app`; `LocalProfile` gains a hex
  `signing_key`, serde-defaulted, minted beside the install-id. The secret
  never leaves the device. N1's atomic writes are the prerequisite.
- `ProfileData2 { install_id, name, pubkey: [u8; 32] }` on a new
  `NetMsg::Profile2` variant; `perform_swap` queues both `Profile` (legacy
  peers) and `Profile2`. Unknown side-channel variants are already ignored by
  old builds, which is the whole migration story. A `PeerKeys` resource sits
  beside `PeerProfile`, cleared in `leave_online_match`.
- `net::MatchStatement`, postcard-encoded: magic `2TRS`, statement version,
  `sim_version`, `arena_id`, both install-ids sorted ascending with pubkeys
  and scores, the deciding frame. Sorted order means both peers serialize
  identical bytes. Scope: score-decided matches only; forfeit frames are set
  out-of-band by the lobby FSM and can differ per peer, so forfeits stay
  ledger-only, as today.
- On MatchOver-entry with the threshold reached (same guards as
  `record_match_result`): build, sign, queue
  `NetMsg::MatchSig { sig: [[u8; 32]; 2] }` (split halves keep the enum
  `Copy`; serde's array impls cap at 32). On the peer's sig: verify, then
  write `<tape-stem>.attest.json` beside the tape via `write_atomic`.
  `RivalRecord` gains `attested_wins`, serde-defaulted.
- `replay_sync --attest <tape> <attest.json>`: replay the tape headlessly,
  rebuild the statement from what the sim actually produced, compare, verify
  both signatures, print the verdict. A claim either re-simulates or it
  doesn't.

**Exit criteria:** unit tests for statement byte-identity across handle
orderings, sign/verify round-trip, tamper rejection, and the legacy-peer mix
degrading to an unsigned ledger entry. A loopback run yields an
`.attest.json` that verifies; a bit-flipped copy fails. Full gate green.

## Phase N3 — the wasm lane, the web theater, share links

Completes the viewer half of COMPLETION_PLAN P.5/P.6.

- **wasm determinism lane.** CI job driving the canonical tape under
  `wasm32-unknown-unknown` via `wasm-bindgen-test` in headless Chrome,
  asserting per-frame checksums equal the committed TSV. Q16.16 plus BTree
  containers plus portable hashers is why this can go green. Risk item:
  headless-browser CI is the fiddly part; the fallback gate is a manually
  run browser page asserting the same checksums, with CI automation as a
  fast-follow.
- **`web_theater` crate** (new workspace member): bevy wasm build reusing
  `sim`, `render`, `replay`, and the theater's playback pattern
  (`build_playback_session`, the snapshot-ring scrub). Loads a tape by drop
  id from the URL fragment. No matchbox, no ureq; the cfg surface stays small
  because full web netplay is horizon, not this phase.
- **`tape_drop` crate** (new): `POST /tape` (64 KB cap, content-addressed id,
  TTL eviction, N1's token bucket), `GET /tape/<id>`, `GET /healthz`.
  Deployed beside ice_vendor; the APK bakes `TWOTOP_DROP_URL` like
  `TWOTOP_ICE_URL`.
- **Share UI.** SHARE on the match summary (once `LastSavedReplay` lands) and
  on theater rows: POST the tape, render the link as a QR (pure-Rust
  `qrcode` crate, rasterized to a bevy `Image`, palette colors). The victory
  screen carries the QR.
- **Hosting.** The static `web_theater` bundle on GitHub Pages or Railway
  static; the URL goes in README's Play-it section.

**Exit criteria:** the wasm checksum gate green (or the documented manual
gate plus a tracked follow-up); a phone-shared link plays scrub-correct in a
desktop browser and an Android browser; the drop relay refuses a flood; full
gate green. COMPLETION_PLAN P.5/P.6 checkboxes updated to reflect the viewer
half.

## Phase N4 — the rivalry home

- `AppScreen::Rivals` plus a `rivals.rs` screen module patterned on
  `arena_select.rs` rows and the theater's list ceiling: rows sorted by
  meetings, `display_name` (the existing collision-tag logic), lifetime
  score, last-met date (`theater.rs` has `date_label`), the rune mark. Tap
  for the rival detail: score line, RUN IT BACK state, their tapes.
- Ledger extensions, all serde-defaulted: `last_met_unix`, `streak`, a capped
  `tapes` ring (the recorder appends the saved filename; it already knows
  `PeerProfile` at save time), plus N2's `attested_wins`.
- A RIVALS button beside REPLAYS in `screen.rs`'s title tables.
- Milestone one-shot at 10, 50, 100 meetings (one table): the dark beyond
  full-flares on the next GO.
- Our-table scars: seed the floor-stain cosmetic stream from the sorted
  install-id pair at online match start. `render::CosmeticRng` only.
- If `screen.rs` (2,194 lines) turns hostile here, do the report's mechanical
  split of `screen.rs` only, with `pub use` shims, as its own commit.

**Exit criteria:** rivals screen navigable on desktop and phone; old
`career.json` files round-trip (extend the existing v1/v2 test); the flourish
fires in a staged test; a two-phone session per PLAYBOOK Rung 4 populates
both sides. Full gate green.

## Phase N5 — minted demons v1

- Runtime accent-hue mint: a seed-derived role-to-role palette swap applied
  to the loaded player sheets. Legal accent roles only; never the team
  channels, never Hit White, never the Blood/contact channels (the
  DESIGN_DIRECTION floor contract, extended to bodies). Own demon from own
  seed; opponent's from `PeerProfile`; tapes fall back to the header name
  exactly as `runes.rs` does today.
- `generate_audio.py` emits 8 kill-sting variants; `audio.rs` picks by the
  killer's install-id. Couch and practice keep the classic sting.
- The rune axis is untouched; the composed space is roughly 8 x 8 x 8.
- `check_palette.py` learns to validate a swap table maps role to role, and
  runs against a swapped-sheet dump in CI.

**Exit criteria:** two installs render distinct demons on both phones in the
same match; the rivals screen and the theater show the same mint per
identity; art gates green; full gate green.

## Phase N6 — shades

- Style extraction as a pure function over a rival's tape ring: throw
  cadence, mean charge hold, dash rate, stick activity, aim spread.
  Input-stream stats only; no resimulation needed for v1.
- `BotStyle` overlay on `bot.rs`'s existing tier knobs (charge percent,
  reaction ticks, orbit bias, plant discipline). The gauntlet offers
  SHADE OF <NAME> once a rival has three tapes; the shade wears their rune
  and hue.
- Honesty rules enforced and tested: shade matches are `PracticeMode`, never
  mutate `CareerRecord.rivals`, and never move the gauntlet tier.

**Exit criteria:** extractor unit tests on synthetic tapes map to expected
knob ranges; the no-ledger-write regression test; an operator feel gate (two
very different tape sets produce visibly different shades); full gate green.

## Phase N7 — the sit-down ritual

- Join links: `https://<web-host>/join#<CODE>-<arena>` (the web page says
  "dial this") and `twotop://join/<CODE>-<arena>` opening the app directly,
  via intent filters in the cargo-apk manifest metadata.
- The sharp edge is intent plumbing: reading the launch URI on Android takes
  a small JNI call (`getIntent().getData()` through the `jni`/`ndk` crates
  against `ANDROID_APP`), parsed into `RoomCode` slots plus `SelectedArena`;
  the existing `room_url(arena)` path does the rest with no connect-path
  changes.
- The private-dial screen shows its own join-link QR (N3's qrcode-to-Image
  path). The scan side is the system camera; no in-app scanner, no
  permission.
- LAN/offline signaling stays horizon; PLAYBOOK documents the interim
  hotspot-plus-laptop recipe.

**Exit criteria:** phone A shows the QR, phone B's system camera opens it,
both land in the same private room and arena, the duel starts (documented as
a PLAYBOOK rung variant). Desktop `--room` flows unchanged. Full gate green.

## Cross-cutting rules

- Every phase ends with `cargo nextest run --workspace --locked`,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`, and
  (from N1 on) `cargo fmt --check`. Commits are kernel-style; trailers per
  `.protocol/config`.
- Network-behavior phases (N1's packet refusal, N2, N7) add a loopback smoke
  per PLAYBOOK Rung 2. N4 and N7 carry the two-phone operator gates, which
  also retire the standing two-phone social-loop test.
- New dependencies are pinned `=` and flow through the N1 audit job. New
  services copy ice_vendor's shape: tiny_http, env config, `/healthz`,
  Railway.
- Doc true-ups ride each phase: CLAUDE.md's status paragraph, README,
  COMPLETION_PLAN's P.5/P.6 boxes at N3, PLAYBOOK at N7.

## Verification ladder (end-to-end)

1. **Bedrock:** a kill -9 during a settings-write loop leaves a loadable
   file; garbage on the ggrs channel warns and forfeits the right way; the
   ice_vendor flood throttles.
2. **Proof:** loopback match, then `replay_sync --attest` says OK; flip one
   byte and it says no.
3. **Broadcast:** phone match, SHARE QR, browser plays the tape
   scrub-correct; wasm checksums equal the native TSV.
4. **The whole table:** two phones, one table. QR pair, duel, devour; both
   rivals screens update; both demons mint distinct; after three tapes the
   shade is waiting.
