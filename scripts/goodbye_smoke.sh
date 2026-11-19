#!/usr/bin/env bash
# Two real peers in a live match; one QUITs (Escape twice in its window).
# The quitter files the loss and concedes (`NetMsg::Bye`); the one who
# stayed must hear it at once and book the concession, not wait out the
# silence and book it unfinished.
#
# Usage: scripts/goodbye_smoke.sh [app-binary]   (needs: scripts/lib/peers.sh)
set -u
APP=${1:-target/debug/app}
. "$(dirname "$0")/lib/peers.sh"
peers_init goodbye-smoke "${PORT:-3538}" "${DISPLAY_NUM:-:93}"
peer_start a TWOTOP_AUTOPILOT=8
peer_start b TWOTOP_AUTOPILOT=3
wait_for_log a "next=InRound" 120 && wait_for_log b "next=InRound" 30 || fail "the match never started"
# QUIT concedes only once the duel has begun on both phones.
wait_for_log b "the duel began" 30 || fail "the duel never began on B"
sleep 2
for _ in 1 2 3 4 5; do
  press b Escape; sleep 0.4; press b Escape
  wait_for_log b "abandoned duel recorded as a loss" 3 && break
done
has_log b "abandoned duel recorded as a loss" || fail "the quitter did not leave"
wait_for_log a "peer said goodbye" 5 || fail "the goodbye never arrived"
sleep 2
[ "$(ledger a)" = "1 0 0" ] || fail "the stayer booked $(ledger a), expected the concession"
pass "the one who stayed heard the goodbye and took the concession"
