#!/usr/bin/env bash
# Two real peers in a live match; one steps away (its window covered, the
# desktop stand-in for a phone going to the background) and freezes for
# 14 s — past the 9 s grace a silent peer gets. It announced the pause, so
# the one who stayed must hold the match open (STEPPED AWAY) and the match
# must carry on when it comes back: no forfeit on either side.
#
# SILENT=1 freezes it without the announcement instead: then the one who
# stayed must forfeit it at the 9 s grace, as before ggrs's own timeout
# moved out to 20 s.
#
# Same needs as scripts/two_peer_smoke.sh, plus xdotool.
# Usage: [SILENT=1] scripts/pause_smoke.sh [app-binary]
set -u
APP=${1:-target/debug/app}
WORK=$(mktemp -d "${TMPDIR_SMOKE:-target}/pause-smoke.XXXXXX")
PORT=${PORT:-3539}
D=${DISPLAY_NUM:-:94}
AWAY_SECS=${AWAY_SECS:-14}
trap 'kill -CONT $(jobs -p) 2>/dev/null; kill $(jobs -p) 2>/dev/null; wait 2>/dev/null' EXIT

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
# No window manager: both windows sit at the origin, B on top. Raising A
# over B covers it completely, and B's window reports itself occluded.
if [ -z "${SILENT:-}" ]; then
  WIN_A=$(DISPLAY=$D xdotool search --pid "$PID_a" 2>/dev/null | tail -1)
  DISPLAY=$D xdotool windowraise "$WIN_A"
  for _ in $(seq 1 20); do strip "$WORK/b.log" | grep -q "going away mid-duel" && break; sleep 0.25; done
  if ! strip "$WORK/b.log" | grep -q "going away mid-duel"; then
    echo "FAIL: B never announced it was going away (logs in $WORK)"; exit 1
  fi
  sleep 0.5
fi
kill -STOP "$PID_b"
T0=$(date +%s.%N)
sleep "$AWAY_SECS"
kill -CONT "$PID_b"
sleep 8
if [ -n "${SILENT:-}" ]; then
  at=$(strip "$WORK/a.log" | grep "next=Forfeited" | head -1 | cut -c1-27)
  if [ -z "$at" ]; then
    echo "FAIL: a silent ${AWAY_SECS}s freeze was never forfeited (logs in $WORK)"; exit 1
  fi
  took=$(python3 -c "import datetime,sys; t=datetime.datetime.fromisoformat(sys.argv[1].rstrip('Z')); print(round(t.timestamp()-float(sys.argv[2]),1))" "$at" "$T0")
  echo "OK: a silent freeze was forfeited ${took}s in (the grace is 9 s)"
  rm -rf "$WORK"; exit 0
fi
fail=
if ! strip "$WORK/a.log" | grep -q "peer is going away for a moment"; then
  echo "FAIL: A never heard the pause"; fail=1
fi
if strip "$WORK/a.log" "$WORK/b.log" | grep -qE "next=Forfeited|peer disconnected"; then
  echo "FAIL: a ${AWAY_SECS}s announced pause ended the match"; fail=1
fi
if ! strip "$WORK/a.log" | grep -q "network resumed"; then
  echo "FAIL: A never heard from B again"; fail=1
fi
if [ -n "$fail" ]; then echo "(logs in $WORK)"; exit 1; fi
echo "OK: a ${AWAY_SECS}s announced pause held the match open, and it carried on"
rm -rf "$WORK"
