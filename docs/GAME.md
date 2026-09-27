# What is in the game

The content of 2-Top as it exists in the tree today, with the file that
defines each rule. The README summarizes this page; `PLAYBOOK.md` says how to
reach each surface on a phone or a desktop.

## The match

Two duelists fight with one boomerang each, and one hit kills. A round lasts
30 seconds at 60 ticks per second. Kills decide the match at five (`MATCH_WIN_THRESHOLD` in
`crates/sim/src/lib.rs`); the round clock only rotates state, so a round with
no kill expires without changing the score. A dead player respawns after
3 seconds with a 0.75-second spawn guard that any offensive act breaks,
taunting included. In the last 8 seconds of a round the floor crumbles in
from the edges (sudden death), except in the Pit and the Vigil, which have no
storm. A round boundary costs 1.5 seconds; the match's first countdown keeps
the full 3-2-1.

A dash gives a short invincibility window. Catching a returning boomerang
within 10 ticks of the recall press is a perfect catch: it empowers the next
throw and raises a streak (`CatchStreak`) whose tiers add speed and reach, up
to a "lightning" throw with full reach at any charge. A taunt roots you for
0.7 seconds; completing it feeds the streak one tier, a dash or a throw
cancels it with no reward, and taunting on the respawn tick forfeits the
spawn guard. On a desktop player 0 taunts with T and player 1 with Enter; on
a phone, the top strip of the screen.

## Seven arenas

`ArenaId` in `crates/sim/src/lib.rs`:

- **Anchor**: the neutral box with one central bone pyre for cover.
- **Crossing**: a blood chasm bisects the arena; hitting an altar sigil
  raises a temporary bone bridge.
- **Reliquary**: paired sigil-door teleporters and chain-linked pyres.
- **The Pit**: walled in. No void and no crumble; the boundary ricochets the
  boomerang back into play.
- **The Vigil**: the storm never comes and a round with no kill expires. Open
  sightlines and two unlinked pyres.
- **The Gallery**: a dense corridor maze.
- **The Forest**: bone trees block movement and ricochet throws. Two chips
  fell a tree and a Heavy throw fells it in one; fire spreads from tree to
  tree and burns the cover down for the rest of the match.

Online, the arena pick is part of the room name, so both phones must pick the
same table to meet.

## Seven pickups

`PickupKind` in `crates/sim/src/lib.rs`. At most one pickup waits on the
floor at a time, spawned on a randomized timer; a colored halo telegraphs the
kind, and the boomerang flies with the same tint.

| Pickup    | What the throw does                                                     |
| --------- | ----------------------------------------------------------------------- |
| Fire      | Faster, and lays a lethal fire trail                                    |
| Heavy     | Slower, plows through cover without ricocheting                         |
| Bouncy    | Gains speed with every wall ricochet                                    |
| Curve     | Curves in flight                                                        |
| Multishot | Throws a fan of three                                                   |
| Phantom   | Phases through walls and cover                                          |
| Swap      | While the boomerang is in flight, the recall press trades places with it |

## Tapes and the theater

Every decided match writes a `.bmrg` input tape; the canonical demo
(`tests/demos/canonical/match_v1.bmrg`) is 14,418 bytes. Because the
simulation is deterministic, the tape reproduces the whole match, and it
plays back through the live game's own presentation. The REPLAYS screen lists
the saved tapes (`~/Downloads/two-top/replays/` on a desktop,
`Android/data/com.ampactorlabs.twotop/files/replays/` on Android,
`localStorage` in a browser). Tap one to play, tap to pause, drag the bottom
strip to scrub, tap a speed from 0.5x to 4x. A tape from another
`SIM_VERSION` lists dimmed with its version tag and does not play.
`replay_viewer` plays a tape on a desktop from the command line.

## Sharing a match

On a build with both share endpoints baked at build time (`TWOTOP_DROP_URL`
for the `tape_drop` service and `TWOTOP_WATCH_URL` for the web theater), a
SHARE label appears on the match summary and in the theater. Tapping it posts
the tape to the drop and shows a QR code of `<watch-url>#watch=<id>`; any
phone camera opens the match in a browser, where it plays through the real
engine. Links live about a week (the drop's default TTL), and re-sharing the
same match re-mints the same link. Without both endpoints the label reads
SAVE TAPE, which in a browser downloads the file. The Pages build bakes
neither endpoint, so the deployed browser build offers SAVE TAPE, while its
`index.html` does fetch shared tapes from the drop.

## Identity, rivals, and the rules of leaving

- **Name and install-id.** A four-letter name entered on a 26-key grid; a
  fresh install gets a placeholder dealt from its install-id, a random `u128`
  minted once per install. The name rides the identity handshake to the
  opponent and into the tape header.
- **Signed results.** An ed25519 key is minted beside the install-id. When an
  online match is decided on score, both phones build the same canonical
  `MatchStatement`, sign it, and swap signatures over the reliable side
  channel; the pair lands beside the tape as `<stem>.attest.json`, and
  `replay_sync --attest` re-runs the tape and verifies both signatures
  offline. Forfeits stay in the ledger only.
- **Rivals.** A per-opponent ledger ("4TH MEETING, you lead 2-1") with a
  RIVALS screen: rivalry rows ranked with standing, streak and last meeting,
  and a per-rival detail with the rivalry's own tapes playable in place.
- **RUN IT BACK.** A finished online match restarts only when both sides
  consent.
- **Leaving.** A top-band tap or Esc leaves cleanly and the opponent's screen
  reads `<NAME> FLED`. If a phone goes silent mid-match the other side sees
  `<NAME> AWAY` for up to 9 seconds, then the match forfeits. The QUIT chip in
  the top corner arms on a first tap and quits on a second within 2.5
  seconds; quitting a live online duel records the loss.
- **Who gets the win.** The score decides first. Otherwise a clean goodbye
  concedes, your own freeze past the timeout is your loss, and a silent drop
  is unfinished: a meeting, nobody's win, nobody's loss (`match_outcome` in
  `crates/app/src/grudge.rs`). A detected desync is also nobody's result.

## The gauntlet and the shade

PRACTICE VS BOT runs the normal local session with the bot supplying the
second player's inputs. Beat it and the gauntlet tier climbs one rung, up to a
ceiling of 10 (`GAUNTLET_MAX_TIER` in `crates/app/src/grudge.rs`), where the
button reads GAUNTLET MASTERED. Lose, or quit a live gauntlet match, and the
tier drops one rung from the tier you lost at, never below 1. A fresh
install's bot starts as a passive dummy, and within a match it sharpens one
notch once you have landed three kills (`in_match_ramp` in
`crates/app/src/bot.rs`), so the first match is the tutorial. Practice never
touches the online record.

At three tapes against one rival, SPAR THEIR SHADE fits that rival's measured
input habits (throw cadence, charge holds, dash appetite, plant discipline)
onto the bot's own knobs (`crates/app/src/shade.rs`). A shade match is
practice: the rivalry ledger and the gauntlet tier do not move, and the UI
labels the opponent SHADE and never uses the rival's name.

## The sit-down ritual

With PRIVATE selected, the dial shows its own join QR on builds with
`TWOTOP_WATCH_URL` baked. The other phone's system camera opens the join page
(`web/join.html`) with the code and the table named; its OPEN IN 2-TOP button
deep-links an installed app to that code and arena through
`twotop://join/<CODE>-<arena>`, an intent filter in the APK manifest. A phone
without the app gets the APK link and dials by hand. Both phones then tap
DUEL AT the same code.

## Settings

Haptics, sound effects, music, the stick deadzone and a southpaw layout (the
whole touch layout mirrored left for right) live behind the SETTINGS button.
Settings persist as JSON in the platform config directory, or in
`localStorage` in a browser.
