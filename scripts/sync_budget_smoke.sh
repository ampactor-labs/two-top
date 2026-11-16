#!/usr/bin/env bash
# A peer that pairs and never plays (crates/app/examples/silent_peer.rs).
# The app must give up on it within the sync budget (12 s), re-dial the
# room once, and on a second such peer say it could not reach the
# challenger, with nothing booked either time. MODE=leave: the fixture
# drops its connection right after pairing instead, which the app must
# read at once (the peer's connection closing).
#
# Usage: [MODE=silent|leave] scripts/sync_budget_smoke.sh [app-binary]
# Needs the example built: cargo build -p app --example silent_peer
set -u
APP=${1:-target/debug/app}
MODE=${MODE:-silent}
SILENT_PEER=${SILENT_PEER:-target/debug/examples/silent_peer}
. "$(dirname "$0")/lib/peers.sh"
peers_init sync-budget-smoke "${PORT:-3542}" "${DISPLAY_NUM:-:97}"
peer_start a
wait_for_log a "WaitingForPeer" 30 || fail "A never reached the room"
ROOM=$(room_of a); [ -n "$ROOM" ] || fail "A logged no room"

challenge() {
  "$SILENT_PEER" "$ROOM" "$MODE" > "$WORK/silent_$1.log" 2>&1 &
  PIDS+=($!); disown
}
challenge 1
wait_for_log a "swapped to P2P session" 20 || fail "A never paired with the silent peer"
t0=$(date +%s.%N)
wait_for_log a "session never synchronized" 20 || fail "A never gave up on the first silent peer"
took=$(python3 -c "print(round($(log_time a 'session never synchronized')-$t0,1))")
wait_for_log a "re-dialing the room" 5 || fail "A did not re-dial"
budget=$([ "$MODE" = leave ] && echo 3 || echo 14)
python3 -c "import sys; sys.exit(0 if $took <= $budget else 1)" || fail "A gave up after ${took}s (budget ${budget}s)"
wait_for_log a "WaitingForPeer.*\$" 20
sleep 2
challenge 2
wait_for_log a "challenger unreachable after a re-dial" 30 || fail "A never told the player after the second peer"
[ "$(ledger a)" = "0 0 0" ] || fail "A booked $(ledger a) for duels that never began"
pass "A gave up on a $MODE peer after ${took}s, re-dialed once, then told the player; nothing booked"
