#!/usr/bin/env bash
# Two real peers, one real signaling server, nobody at either keyboard.
#
# Starts matchbox_server on localhost, then two desktop app processes in
# one quick-match room, each with its own home directory and the bot
# brain playing its local side (TWOTOP_AUTOPILOT: AUTOPILOT_A / AUTOPILOT_B
# set the tiers, empty for an idle seat). Both must decide the match with
# no desync (ggrs exchanges checksums every 30 frames); then both tapes
# must replay to the same per-frame checksums, and each attestation must
# verify against its tape.
#
# A manual tool, not yet a CI gate: under lavapipe a match takes minutes of
# wall time, and two deterministic bots sometimes settle into a symmetric
# orbit where neither lands a kill (raise TIMEOUT, or change the tiers).
#
# Needs: matchbox_server 0.14 on PATH (`cargo install matchbox_server
# --version ~0.14 --locked`), Xvfb, a Vulkan driver (lavapipe works:
# VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json), and built
# binaries (`cargo build -p app -p replay_sync`).
# Usage: scripts/two_peer_smoke.sh [app-binary]
#
# For a peer that pairs and never syncs (the sync budget and re-dial),
# start one app the same way and point `cargo run -p app --example
# silent_peer -- ws://127.0.0.1:$PORT/two-top-v<SIM_VERSION>?next=2 silent`
# at the room the app logs.
set -u
APP=${1:-target/debug/app}
# Work (homes, logs, tapes) lives under target/, removed on a pass.
WORK=$(mktemp -d "${TMPDIR_SMOKE:-target}/two-peer-smoke.XXXXXX")
PORT=${PORT:-3536}
DISPLAY_NUM=${DISPLAY_NUM:-:91}
TIMEOUT=${TIMEOUT:-240}
trap 'kill $(jobs -p) 2>/dev/null; wait 2>/dev/null' EXIT

matchbox_server "127.0.0.1:$PORT" > "$WORK/signal.log" 2>&1 &
sleep 1
export DISPLAY=$DISPLAY_NUM
Xvfb "$DISPLAY_NUM" -screen 0 1280x1400x24 > /dev/null 2>&1 &
sleep 2

for peer in a b; do
  mkdir -p "$WORK/$peer"
  (
    # STATE_LOG_DIR=<dir>: each peer logs every frame's state there
    # (TWOTOP_STATE_LOG), for finding where a desync began.
    if [ -n "${STATE_LOG_DIR:-}" ]; then export TWOTOP_STATE_LOG="$STATE_LOG_DIR/state_$peer.log"; fi
    export HOME="$WORK/$peer" XDG_CONFIG_HOME="$WORK/$peer/.config" XDG_DATA_HOME="$WORK/$peer/.local/share"
    export MATCHBOX_ROOM="ws://127.0.0.1:$PORT/two-top?next=2" TWOTOP_AUTOSTART=1 TWOTOP_WINDOW=270x585
    # Different tiers per seat: the same bot in both mirrored seats plays a
    # mirror match (fangs pass each other) and rounds stall out.
    if [ "$peer" = a ]; then tier=${AUTOPILOT_A-8}; else tier=${AUTOPILOT_B-3}; fi
    if [ -n "$tier" ]; then export TWOTOP_AUTOPILOT="$tier"; fi
    exec "$APP"
  ) > "$WORK/$peer.log" 2>&1 &
  sleep 1
done

deadline=$((SECONDS + TIMEOUT))
over() { sed 's/\x1b\[[0-9;]*m//g' "$1" | grep "next=MatchOver" | head -1 | sed -E 's/.*frame=([0-9]+).*/\1/'; }
until [ -n "$(over "$WORK/a.log")" ] && [ -n "$(over "$WORK/b.log")" ]; do
  if [ $SECONDS -ge $deadline ]; then
    echo "FAIL: no finished match on both peers within ${TIMEOUT}s (logs in $WORK)"
    exit 1
  fi
  if grep -q "PANIC" "$WORK/a.log" "$WORK/b.log"; then
    echo "FAIL: a peer panicked (logs in $WORK)"; exit 1
  fi
  if grep -q "DESYNC DETECTED" "$WORK/a.log" "$WORK/b.log"; then
    echo "FAIL: the peers desynced (logs in $WORK)"
    grep -h "DESYNC DETECTED" "$WORK/a.log" "$WORK/b.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-160
    exit 1
  fi
  sleep 2
done
FA=$(over "$WORK/a.log"); FB=$(over "$WORK/b.log")
# The app logs a transition on the frame it saw it, which prediction can
# put a frame apart on the two peers; ggrs's own checksum exchange (no
# DESYNC above) is the authority on agreement.
if [ $(( FA > FB ? FA - FB : FB - FA )) -le 2 ]; then
  echo "OK: both peers decided the match (frame $FA / $FB), no desync"
else
  echo "FAIL: the peers decided the match far apart ($FA vs $FB; logs in $WORK)"; exit 1
fi
sleep 4   # the summary settles the signed result and saves the tape

strip() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }
# A THROW pressed in the last input-delay frames of a round lands after
# MatchOver, where the sim reads it as an instant rematch: no summary, no
# tape, no signed result (a known sim-level issue; see docs).
# Nobody taps RUN IT BACK here, so any MatchOver -> Countdown is that.
if strip "$WORK/a.log" | grep -q "prev=MatchOver next=Countdown"; then
  echo "WARN: the match restarted from MatchOver at once (a late THROW became a rematch) — no tape to compare"
  warned=1
else
  tape() { strip "$1" | grep "match replay saved" | tail -1 | sed -E 's/.*path=([^ ]+).*/\1/'; }
  TA=$(tape "$WORK/a.log"); TB=$(tape "$WORK/b.log")
  if [ -n "$TA" ] && [ -n "$TB" ]; then
    # The header carries each phone's own clock, so compare what the tapes
    # replay to: the per-frame checksums of the whole match.
    RS=${REPLAY_SYNC:-target/debug/replay_sync}
    "$RS" --demo "$TA" --output "$WORK/a.tsv" > /dev/null 2>&1
    "$RS" --demo "$TB" --output "$WORK/b.tsv" > /dev/null 2>&1
    if [ -s "$WORK/a.tsv" ] && cmp -s "$WORK/a.tsv" "$WORK/b.tsv"; then
      echo "OK: both tapes replay to the same $(($(wc -l < "$WORK/a.tsv") - 1)) frames"
    else
      echo "FAIL: the tapes replay differently (logs in $WORK)"; exit 1
    fi
    for t in "$TA" "$TB"; do
      att="${t%.bmrg}.attest.json"
      if [ -f "$att" ] && "$RS" --demo "$t" --attest "$att" > /dev/null 2>&1; then
        echo "OK: $(basename "$att") verifies against its tape"
      else
        echo "WARN: no verifiable attestation beside $(basename "$t")"
        warned=1
      fi
    done
  else
    echo "WARN: a peer saved no tape"
    warned=1
  fi
fi
# A warning keeps the logs for a look; a clean pass leaves nothing behind.
if [ -n "${warned:-}" ]; then echo "(logs kept in $WORK)"; else rm -rf "$WORK"; fi
