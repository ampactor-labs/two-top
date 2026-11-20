#!/usr/bin/env bash
# Two real peers in a live match; B goes away for AWAY_SECS (default 14).
# By default B announces it first (its window covered, so it reports itself
# occluded and sends `NetMsg::Pausing`): A must hold the match for the
# paused grace (20 s) and carry on when B returns. SILENT=1 freezes B
# without the announcement: at the 9 s silence grace A gives up on the
# old link and both resume the match on a new one.
#
# Usage: [SILENT=1] [AWAY_SECS=n] scripts/pause_smoke.sh [app-binary]
set -u
APP=${1:-target/debug/app}
AWAY_SECS=${AWAY_SECS:-14}
. "$(dirname "$0")/lib/peers.sh"
peers_init pause-smoke "${PORT:-3539}" "${DISPLAY_NUM:-:94}"
peer_start a TWOTOP_AUTOPILOT=8
peer_start b TWOTOP_AUTOPILOT=3
wait_for_log a "next=InRound" 120 && wait_for_log b "next=InRound" 30 || fail "the match never started"
sleep 3
if [ -z "${SILENT:-}" ]; then
  # No window manager: both windows sit at the origin. Raising A over B
  # covers B completely, and B's window reports itself occluded.
  WIN_A=$(DISPLAY=$DISPLAY_NUM xdotool search --pid "$(pid_of a)" 2>/dev/null | tail -1)
  DISPLAY=$DISPLAY_NUM xdotool windowraise "$WIN_A"
  wait_for_log b "going away mid-duel" 5 || fail "B never announced it was going away"
  sleep 0.5
fi
kill_peer b -STOP
T0=$(date +%s.%N)
sleep "$AWAY_SECS"
kill_peer b -CONT
sleep 8
if [ -n "${SILENT:-}" ]; then
  at=$(log_time a "dialing the resume room") || fail "a silent ${AWAY_SECS}s freeze was never noticed"
  wait_for_log a "resumed — the match continues" 40 || fail "the silent freeze was never resumed"
  wait_for_log b "resumed — the match continues" 10 || fail "B never resumed"
  pass "a silent freeze was noticed $(python3 -c "print(round($at-$T0,1))")s in (the grace is 9 s) and resumed"
  exit 0
fi
has_log a "peer is going away for a moment" || fail "A never heard the pause"
has_log a "next=Forfeited|peer disconnected" && fail "a ${AWAY_SECS}s announced pause ended the match"
has_log b "next=Forfeited|peer disconnected" && fail "a ${AWAY_SECS}s announced pause ended the match"
has_log a "network resumed" || fail "A never heard from B again"
pass "a ${AWAY_SECS}s announced pause held the match open, and it carried on"
