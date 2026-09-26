# What is in the game

This page lists the modes and content of 2-Top as of `SIM_VERSION` 15. It
moved here from the README. The rules live in `crates/sim/src/lib.rs`; the
screens and the online identity layer live in `crates/app/src`.

## The match

Each player has one boomerang, which the game calls a fang. You throw it,
catch it on the way back or recall it, and a hit kills. Rounds last 30
seconds, but only kills score: the first player to five kills wins the
match. A dash gives invincibility frames (a short window in which hits do
not land).

## Arenas

There are seven arenas, each built around one rule.

| Arena       | Rule                                                                                                                                                                                                                  |
| ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Anchor      | An open box with one bone pyre in the center.                                                                                                                                                                         |
| Crossing    | A blood chasm splits the arena. Hitting an altar sigil raises a temporary bone bridge.                                                                                                                                |
| Reliquary   | Paired teleporter doors and chain-linked bone pyres.                                                                                                                                                                  |
| The Pit     | The arena is walled in. There is no void, and the boundary ricochets your fang back into play.                                                                                                                                     |
| The Vigil   | The storm never comes and the floor never shrinks. A round with no kill expires without a score.                                                                                                                     |
| The Gallery | A dense corridor maze.                                                                                                                                                                                                |
| The Forest  | Twelve bone trees block movement and ricochet fangs. Two hits fell a tree, or one hit from a Heavy fang. Fire spreads from tree to tree and burns them down, and the sightlines it opens stay open for the rest of the match. |

On arenas with a storm, the floor crumbles inward over the last seconds of
each round (sudden death).

## Pickups

Seven pickups change how your fang flies. Each one sits on the floor inside
a colored halo in the same tint the fang will fly with.

| Pickup    | Effect                                                            |
| --------- | ----------------------------------------------------------------- |
| Fire      | A faster fang that leaves a lethal fire trail.                    |
| Heavy     | A slower fang that plows through cover without ricocheting.       |
| Bouncy    | Gains speed with every wall ricochet.                             |
| Curve     | Curves in flight.                                                 |
| Multishot | Throws a fan of three fangs.                                      |
| Phantom   | Passes through walls and cover.                                   |
| Swap      | While the fang is in flight, recall trades places with it.        |

## Catches and the taunt

A perfect catch raises a streak that makes your next throws faster and
longer. The taunt roots you in place for 0.7 seconds; if you survive it, the
streak climbs one tier. Dashing or throwing cancels the taunt with no reward.

## Replays

Every decided match writes a tape (a `.bmrg` file) holding both players'
inputs for every frame. Because the simulation is deterministic, a tape
replays the whole match exactly, on any device. The REPLAYS screen plays
tapes through the live game's own presentation, with scrubbing and playback
speeds from 0.5x to 4x. A tape recorded by a build with a different
`SIM_VERSION` shows dimmed, labeled with its version, and does not play. On
builds with the share service configured, SHARE posts a tape and shows a QR
code of a link that plays the match in a browser.

## Online identity

- A four-letter name, entered like arcade initials, on top of a durable
  install id.
- An ed25519 signing key, created alongside the install id. After a match
  decided on score, both phones sign the same result statement, and the app
  saves it as an `.attest.json` file beside the tape.
- A rivalry record per opponent ("4TH MEETING, you lead 2-1"), listed on the
  RIVALS screen with each rival's recent tapes.
- RUN IT BACK: a rematch starts only when both players ask for it.
- Forfeits: whoever walks away owns the loss, and quitting a live duel from
  the in-match QUIT chip counts the same way. A connection that drops
  silently records the meeting but gives nobody the win.

## Practice

The gauntlet is a practice ladder against a bot, with ten tiers. A win
raises the tier by one. A loss drops it one rung below the tier you lost at,
so a losing streak keeps easing the bot off. Practice results never touch
the online record. From a rival's page, SPAR THEIR SHADE starts a practice
match against a bot tuned to habits fitted from that rival's tapes.

## Settings

The settings screen covers haptics, sound effects, music, the stick deadzone
and a southpaw layout that mirrors the touch controls left to right.
[`PLAYBOOK.md`](../PLAYBOOK.md) describes the screens in more detail, with
their desktop keys.
