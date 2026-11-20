# Feel and visuals audit (P9)

What this covers: the contact sheet (`scripts/contact_sheet.sh`: every screen at
540x1170, 720x1280, 1024x768 and 900x600, rendered headlessly on lavapipe), the
feel lab readouts in practice, and the live/theater smokes. What it cannot cover
is how the game feels in a hand. That needs the phones, so those checks are listed at
the end as a script for the next device session.

Fixes go through the tables (`TextRole`, `Tuning`, `palette`, `ui::z`,
`modifier_color`). A change to sim feel needs a SIM bump of its own. None was needed here.

## Text roles

| Finding | Where | Verdict |
|---|---|---|
| Strokes were dropped at small sizes: the pixel font was antialiased at fractional scales | every screen, phone sizes | **Fixed.** `ui::pixel_size` snaps each role to whole screen pixels per font pixel, and `smoothing_for` turns smoothing off at 1.5 or more screen pixels per font pixel. |
| Title secondary buttons (PRACTICE / RIVALS / REPLAYS / SETTINGS) sit about 12 px from the screen edge at 540 wide | title, 540x1170 | Acceptable. They are inside the safe area on every phone in the matrix. Revisit when edge-to-edge (targetSdk 35+) lands, together with the insets. |
| Settings went from 6 to 9 rows (GO LIVE, SEND MY SHADE, LADDER) | settings, all sizes | **Re-packed.** The pitch is now 0.048 and the box 56 tall. Bands are still disjoint and clear of BACK (`layout_bands_are_disjoint_and_clear_of_back`). In landscape (900x600) the boxes touch but do not overlap. |
| The REPLAYS empty state prints the full replays path | replays | Kept on purpose: the path is how a desktop player finds their tapes. It is bounded at 1000 units and wraps. On Android it reads as the app's files dir. |
| The desktop key hint (P0/P1 bindings) overlaps the arena's bottom edge in landscape | practice, 900x600 | Desktop dev build only, and the phone never draws it. No change. |

## Contrast

- The score pips (cold stone on void) read at every size. Once a pip is lit, the
  player colour carries the meaning.
- The LIVE mark uses `P0_BLOOD` and the VERIFIED badge uses `P1_CYAN`. Both stay
  saturated against the void and the floor dither. SIGNED (still checking) is
  `SPARK`, the colour every other in-progress word uses.
- The arena label on the title ("< Anchor >") is the lowest-contrast text on
  the screen (bone at 0.7 alpha). It is a secondary control, and the arena
  roster screen says the same thing larger.

## Motion magnitudes

These have not been measured yet. The tool for it is the feel lab (practice, three-finger
tap or F1): hitboxes on, the per-duelist readout, and speeds of 0.25x and paused. What the
code guarantees today:
- The kill-cam punch (1.6x) holds through the summary. Anchored UI keeps its on-screen
  size (`ScreenAnchor` multiplies by `ViewRect::scale`).
- Screen shake follows the settings slider (0 to 100 %). At 0 the kill flash and haptics
  still fire, by design.

## Input to visible

- **Online:** the irreversible beats wait for confirmation (`render::facts`). These are
  the death sting, haptics, pip slam, stains, kill-cam and devour. The cost is one or
  two confirmed ticks of delay on the sting; in return a rolled-back kill never flashes.
  Bursts, sparks and hit-stop stay immediate.
- **Live viewers:** a viewer plays only confirmed ticks. The LIVE edge keeps 8 ticks
  of slack so that one frame's burst of ticks cannot run past what has arrived. If a
  hitch does run past, the theater goes back to the last good tick and replays it
  (`extend_live`). The first live smoke run, with a 3-tick edge, logged one such re-seek;
  the edge is now 8.

## Consistency

- Every screen's exit is the top strip or BACK at `ui::BACK`. The new labels (CLIP,
  SHARE, SEND RESULT) sit in the corner row under the strip, which is the SHARE row,
  and tests keep each one out of the exit strip.
- Every link card uses the same layout: a QR in the middle, the URL, then TAP TO CLOSE
  (`share::spawn_link_card`). SHARE, GO LIVE and SEND MY SHADE all use it.

## Arena and fang readability

The contact sheet captures the default table (Anchor) only. There, fangs and duelists
read at every size, against a floor dither that is busy by design. The other six tables
have not been checked at phone sizes. Adding an `ARENA` axis to the contact sheet is the
next step for this section.

## For the next device session

1. Feel lab in practice (three-finger tap): check that a dash out of a thrown fang's path
   feels fair at 0.25x and that the hitboxes match what you see.
2. GO LIVE from Settings, play a duel, and watch it on a third phone from the QR. The
   LIVE mark should be on both duelists' screens.
3. Screen-off and app switch mid-duel: the opponent should see AWAY, then the comeback
   3-2-1.
4. A notched phone in landscape and portrait: make sure no tap target sits under the
   cutout.
