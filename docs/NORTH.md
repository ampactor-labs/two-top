# NORTH — the ultimate form

What this project is when it is finished. `ARCHITECTURE.md` says what is being
built, `BUILD_PLAN.md` says in what order, `CONVENTIONS.md` says the rules,
`MORGAN_NOTES.md` says why the pieces are shaped the way they are. This doc
says where it all points, so that every future phase can be checked against a
destination instead of a vibe. The execution plan is
`docs/plans/NORTH_PLAN.md`.

## The thesis

2-Top's finished form is not a fighting game with netplay attached. It is a
dueling practice with perfect memory: two demons, one table, and a 14 KB proof
of everything that has ever happened between two people.

The whole program rests on two primitives that already exist in the tree. The
`u128` install-id is identity: minted once per install (`app/src/profile.rs`),
carried on the side channel, the key the grudge ledger files every human
under. The `.bmrg` tape is memory: the canonical demo
(`tests/demos/canonical/match_v1.bmrg`) is 14,418 bytes and reproduces a full
match bit-for-bit on every platform the determinism matrix covers. Everything
in NORTH is those two primitives composed. Almost nothing new gets invented;
it gets meant.

The closest ancestor isn't Boomerang Fu; it's chess. A small, fixed ruleset;
no seasons, no store; opponents you have history with; and notation. A chess
game is a tiny text file that reproduces the game exactly, in any
implementation, forever. The tape is pixel-fighter PGN. MORGAN_NOTES already
names the consequence: replay-as-file is the foundation for spectating,
anti-cheat, and tournament records. NORTH is what happens when that stops
being a footnote and becomes the product definition.

`DESIGN_DIRECTION.md` has a phrase for why blood pools on the floor instead of
caking onto the fang: the arena remembers the violence. That is the whole
design, one level up. The ledger remembers the rival. The tape remembers the
match. The rune remembers the install.

The table remembers.

## Pillar I — the rivalry is the spine

Today the ledger is a summary line: `grudge.rs` keys rivals by install-id and
the match summary can say "4TH MEETING — YOU LEAD 2-1". Finished, the ledger
is the home surface. The screen you boot into is your tables: each named rival
a row, the lifetime score between you still open, their demon wearing their
mark, the last tape one tap away, RUN IT BACK armed. Retention without
manipulation; you come back because a human left it 5-3, not because a daily
chest expired.

Milestones live in the lore register, not a toast. On the hundredth meeting
with the same rival, every eye-pair in the dark beyond flares on GO
(`dark_beyond.rs` already flares them all on a kill; this is one more
trigger). The table itself remembers too: the cosmetic stain stream is
reseeded from the pair of install-ids, so the Anchor you two always play
carries scorch marks that strangers' Anchors don't. Render-side only,
`CosmeticRng`, never sim.

## Pillar II — every result is a proof

This is determinism cashing out as product. Each install mints an ed25519
keypair beside its install-id; pubkeys ride the side channel the way names
already do. When a match is decided on score, both phones serialize the same
canonical statement (sim version, arena, both identities sorted, the score,
the deciding frame; all of it rollback-deterministic, so both peers produce
identical bytes), sign it, and swap signatures. The result lands beside the
tape as an attestation file.

A signed result is self-certifying: anyone can re-run the inputs through
`replay_sync` and get the same per-frame checksums CI produces today, then
check both signatures. Anti-cheat is not a kernel driver; it is "your claim
must re-simulate." Ranked play, when it comes, needs only a dumb relay that
collects signed statements; disputes are settled by resimulation. Tournaments
pin a `SIM_VERSION` the way fighting games pin a patch, and the strict
version-matching rule the replay codec already enforces is the tournament
rule.

The honest boundary: forfeit results stay ledger-only, as today. A forfeit's
deciding frame is set out-of-band by the lobby FSM and the two phones can
disagree about it by a few ticks, so there is no shared statement to sign.
Score-decided matches are the ones that carry proofs.

## Pillar III — the demon is minted, not bought

`runes.rs` already calls itself "the install-id's demon, made visibly yours":
eight baked sheet variants per side, chosen by identity that already travels.
Finished, the mint widens without breaking a single readability law. The
install-id picks the rune (8, baked), an accent hue applied as a role-to-role
palette swap at sheet load (runtime, zero new files, never touching the team
channels or the contact channels), and one of eight kill stings synthesized
into the existing audio set. Around 512 composed identities from sixteen
existing PNGs and eight new WAVs; your rival's phone renders yours from the
sixteen bytes it already receives.

No asset transfer, no cosmetic economy, no store. Uniqueness as birthmark.
The silhouette classes stay exactly two, because Sirlin's law is the boundary
of the space, not a casualty of it: Cur reads as Cur and Stag reads as Stag
under the flood test no matter whose demon is wearing the body. Geometry
variation (horn curl, antler branching, tail sweep, cloak tatters) is designed
into the generator's `Build` profiles as a v2 axis; it stays on the horizon
until the composed mint has proven the appetite.

## Pillar IV — the tape is the broadcast

The PWA milestone (COMPLETION_PLAN tasks P.5/P.6) finishes viewer-first.
Every tape gets a link and a QR; any browser plays the match in the real
engine, with the real presentation: kill-cam, pip-slam, the devouring, scrub
and slow-mo. A clutch comeback is a URL, not a screen recording. The victory
screen carries the QR, so the loser's friend scans it and the comeback plays
on their phone before the table cools.

The transport is a drop relay in `ice_vendor`'s weight class: POST a tape,
get a content-addressed link with a TTL. Links outlive the phone session,
which is what makes them worth sending. Live spectating is the same viewer
leaning forward (stream confirmed inputs a beat behind live over WebRTC;
matchbox has been browser-native the whole time) and stays on the horizon
until the still-tape loop earns it.

The real gate is a wasm32 lane in the replay determinism matrix. The sim
being Q16.16 end-to-end with BTree containers and portable hashers is exactly
why that lane can go green; floats would have made the web build a fork.

## Pillar V — shades in the gauntlet

Tapes are measurable: throw cadence, charge holds, dash rate, stick activity,
aim spread. Fit the practice bot's existing tier knobs from a rival's tapes
and their shade waits in the gauntlet while they sleep, wearing their rune and
their hue. `runes.rs` already dresses ghosts honestly from the tape header;
the shade inherits the rule.

Two honesty rules keep it clean, both enforced in code. Shades never write to
the grudge ledger; the record is human-only, the same ethic that makes the
away-grace forfeit assign blame honestly. And the framing stays "sparring
partner with their habits," because a fitted bot is a caricature, not a
person, and pretending otherwise would cheapen the real rematch it exists to
sharpen.

## Pillar VI — the ritual of sitting down

The name is a restaurant table for two. The depth-duel camera tilts the arena
into a tabletop between you; arenas are tables; agreement is structural
because the pick rides the room name ("dial CURS, pick the Pit" —
MORGAN_NOTES). Finished, the physical ritual completes: the private dial
shows its join link as a QR, the other phone's system camera opens it, and a
deep link drops both phones into the same room and arena with zero typing.
No in-app scanner, no camera permission; the OS already owns that job.

Two phones flat on a real 2-top, two people leaning in, a third phone
watching the tape. That scene is the product photograph, and every pixel of
it runs on machinery that already exists.

## The subtraction list

Powerful and clean are the same decision here. What the finished form
refuses, on purpose:

- No accounts. Install-id plus a typed name is the account model; OAuth would
  delete a design, not add a feature.
- No store. Demons are minted at birth (Pillar III).
- No seasons, no currency, no decay. The only number that ever grows is
  history.
- No 4-player. The name says two; MORGAN_NOTES already rejected the chaos.
- No staked matches. Ruled a separate product long ago; still ruled.
- No server memory. Identity and history live in the phones. The only
  infrastructure the game owns is a room list, a credential vendor, and (with
  Pillar IV) a tape drop. Everyone else can't ship this shape because of
  their server bill; that is the moat.

## Bedrock

The 2026-07-25 upgrade report reads differently under NORTH. Atomic
persistence writes are not hygiene; the ledger is the product, and a mid-write
process kill that mints a fresh install-id kills a rivalry on both phones and
orphans a demon. Refusing malformed peer packets is the social layer's honesty
extended into the transport; the public table must be safe to sit at. The
credential vendor deserves the same discipline as the sim because it is the
only server the game owns. Those land first, as phase N1 of the plan.

## Horizon

Designed, recorded here, and deliberately not scheduled. Each waits for
evidence (mostly: real player volume) before it is worth its weight.

- Result relay plus leaderboard page; tournament brackets pinned to a
  `SIM_VERSION`, every reported result carrying its tape.
- Live spectating over the Pillar IV viewer.
- Full web netplay (the rest of COMPLETION_PLAN P.5; the viewer ships first).
- LAN and offline signaling for internet-free tables; the interim recipe is a
  hotspot plus a laptop `matchbox_server`, in PLAYBOOK.
- Demon geometry mint v2 through the generator's `Build` class; the runtime
  Rust atelier only if the baked matrix ever proves too small.

## Sim-neutrality

The entire NORTH program is sim-neutral: `SIM_VERSION` stays 14 throughout,
the ggrs wire format is untouched, and no phase adds a migration. The
determinism matrix keeps running as the standing gate it already is. That is
the quiet proof that the architecture was pointed here all along; the ultimate
form is reachable without touching the part that had to be perfect.
