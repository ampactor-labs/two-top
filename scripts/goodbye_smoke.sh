#!/usr/bin/env bash
# Two real peers in a live match; one QUITs (Escape twice in its window).
# The one who stayed must hear the goodbye at once and book the
# concession — not wait out ggrs's 9 s timeout and book it unfinished,
# which is what happened while leaving dropped the connection under the
# goodbye (SIGNALING.md, the clean-leave path).
#
# Same needs as scripts/two_peer_smoke.sh, plus xdotool.
# Usage: scripts/goodbye_smoke.sh [app-binary]
set -u
APP=${1:-target/debug/app}
WORK=$(mktemp -d "${TMPDIR_SMOKE:-target}/goodbye-smoke.XXXXXX")
PORT=${PORT:-3538}
D=${DISPLAY_NUM:-:93}
trap 'kill $(jobs -p) 2>/dev/null; wait 2>/dev/null' EXIT

matchbox_server "127.0.0.1:$PORT" > "$WORK/signal.log" 2>&1 &
Xvfb "$D" -screen 0 1280x1400x24 > /dev/null 2>&1 &
sleep 2
for peer in a b; do
  mkdir -p "$WORK/$peer"
  (
    export DISPLAY=$D HOME="$WORK/$peer" XDG_CONFIG_HOME="$WORK/$peer/.config" XDG_DATA_HOME="$WORK/$peer/.local/share"
    export MATCHBOX_ROOM="ws://127.0.0.1:$PORT/two-top?next=2" TWOTOP_AUTOSTART=1 TWOTOP_WINDOW=270x585
    if [ "$peer" = a ]; then export TWOTOP_AUTOPILOT=8; else export TWOTOP_AUTOPILOT=3; fi
    exec "$APP"
  ) > "$WORK/$peer.log" 2>&1 &
  eval "PID_$peer=$!"
  sleep 1
done

strip() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }
for _ in $(seq 1 120); do strip "$WORK/b.log" | grep -q "next=InRound" && break; sleep 1; done
if ! strip "$WORK/b.log" | grep -q "next=InRound"; then
  echo "FAIL: the match never started (logs in $WORK)"; exit 1
fi
sleep 3
WIN=$(DISPLAY=$D xdotool search --pid "$PID_b" 2>/dev/null | tail -1)
DISPLAY=$D xdotool windowfocus --sync "$WIN"
DISPLAY=$D xdotool key --window "$WIN" Escape; sleep 0.5
DISPLAY=$D xdotool key --window "$WIN" Escape
sleep 4
if ! strip "$WORK/b.log" | grep -q "abandoned duel recorded as a loss"; then
  echo "FAIL: the quitter did not leave (logs in $WORK)"; exit 1
fi
if strip "$WORK/a.log" | grep -q "peer said goodbye"; then
  echo "OK: the one who stayed heard the goodbye and took the concession"
  rm -rf "$WORK"
else
  echo "FAIL: the goodbye never arrived (logs in $WORK)"; exit 1
fi
