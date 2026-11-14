#!/usr/bin/env bash
# Every way an online match can end, with two real peers (A at tier 8, B at
# tier 3, open ground). One scenario per run:
#
#   desync  A's sim is nudged off B's at tick 600 and A's checksum reports
#           are muted (debug builds: TWOTOP_FAULT), so only A's ggrs can see
#           it. B must still void the match within 2 s of A detecting it:
#           no tape, no ledger line on either phone.
#   kill    B is killed outright mid-match. A must end it within 3 s (the
#           connection closing, not the 9 s silence) and book it unfinished.
#   early   B is killed before the first GO. Nothing was at stake: A books
#           nothing and goes back to the room.
#   freeze  B freezes for 12 s mid-match, then wakes. A ends it as a silent
#           drop at ~9 s (unfinished); B must end within 3 s of waking and
#           book the loss its own absence earned.
#   leave   A full match; then A presses LEAVE on the summary. Both ledgers
#           must agree, the two tapes must be the same bytes, both signed
#           results must verify, and B's card must say A left the table.
#
# Usage: scripts/endings_smoke.sh <scenario> [app-binary]
# Same needs as scripts/lib/peers.sh.
set -u
SCENARIO=${1:?usage: endings_smoke.sh desync|kill|early|freeze|leave [app]}
APP=${2:-target/debug/app}
. "$(dirname "$0")/lib/peers.sh"
peers_init "endings-$SCENARIO" "${PORT:-3541}" "${DISPLAY_NUM:-:96}"

case "$SCENARIO" in
  desync) peer_start a TWOTOP_AUTOPILOT=8 TWOTOP_ARENA=vigil TWOTOP_FAULT=desync@600 ;;
  *) peer_start a TWOTOP_AUTOPILOT=8 TWOTOP_ARENA=vigil ;;
esac
peer_start b TWOTOP_AUTOPILOT=3 TWOTOP_ARENA=vigil

started() { wait_for_log a "next=InRound" 120 && wait_for_log b "next=InRound" 30; }

case "$SCENARIO" in
desync)
  started || fail "the match never started"
  wait_for_log a "DESYNC DETECTED" 60 || fail "A never detected the injected desync"
  wait_for_log b "peer ended the session.*Desynced.*honoured=true" 10 \
    || fail "B never heard of the desync A detected"
  ta=$(log_time a "DESYNC DETECTED"); tb=$(log_time b "peer ended the session")
  took=$(python3 -c "print(round($tb-$ta,2))")
  python3 -c "import sys; sys.exit(0 if $tb-$ta <= 2.0 else 1)" \
    || fail "B voided the match ${took}s after A detected the desync (budget 2 s)"
  has_log b "DESYNC DETECTED" && fail "B's own check fired: the fault was not one-sided"
  sleep 3
  [ "$(ledger a)" = "0 0 0" ] && [ "$(ledger b)" = "0 0 0" ] \
    || fail "a desynced match was booked (A: $(ledger a), B: $(ledger b))"
  has_log a "match replay saved" || has_log b "match replay saved" && fail "a desynced match left a tape"
  pass "B voided the desync ${took}s after A detected it; nothing booked, no tape"
  ;;
kill)
  started || fail "the match never started"
  sleep 3
  kill -9 "$(pid_of b)"
  t0=$(date +%s.%N)
  wait_for_log a "next=Forfeited" 12 || fail "A never ended the match"
  took=$(python3 -c "print(round($(log_time a 'next=Forfeited')-$t0,2))")
  python3 -c "import sys; sys.exit(0 if $took <= 3.0 else 1)" \
    || fail "A ended the match ${took}s after B died (budget 3 s)"
  sleep 3
  [ "$(ledger a)" = "0 0 1" ] || fail "A booked $(ledger a), expected one unfinished"
  pass "A ended the match ${took}s after B was killed, booked unfinished"
  ;;
early)
  wait_for_log b "swapped to P2P session" 120 || fail "the peers never paired"
  has_log b "next=InRound" && fail "too late: the round had already started"
  kill -9 "$(pid_of b)"
  wait_for_log a "nothing booked, re-dialing|session never synchronized|re-dialing the room" 15 \
    || fail "A did not go back to the room"
  sleep 2
  [ "$(ledger a)" = "0 0 0" ] || fail "A booked $(ledger a) for a duel that never began"
  pass "a peer lost before the first GO: nothing booked, A re-dialed"
  ;;
freeze)
  started || fail "the match never started"
  sleep 3
  kill -STOP "$(pid_of b)"
  sleep 12
  kill -CONT "$(pid_of b)"
  t0=$(date +%s.%N)
  wait_for_log a "next=Forfeited" 5 || fail "A never ended the frozen match"
  wait_for_log b "session concluded" 10 || fail "B never ended the match after waking"
  took=$(python3 -c "print(round($(log_time b 'session concluded')-$t0,2))")
  python3 -c "import sys; sys.exit(0 if $took <= 3.0 else 1)" \
    || fail "B ended the match ${took}s after waking (budget 3 s)"
  sleep 3
  [ "$(ledger a)" = "0 0 1" ] || fail "A booked $(ledger a), expected one unfinished"
  [ "$(ledger b)" = "0 1 0" ] || fail "B booked $(ledger b), expected the loss its freeze earned"
  pass "A closed the silent match unfinished; B woke, ended it ${took}s later and took the loss"
  ;;
leave)
  started || fail "the match never started"
  until has_log a "next=MatchOver" && has_log b "next=MatchOver"; do
    check_health
    has_log a "DESYNC DETECTED" && fail "the peers desynced"
    [ $SECONDS -ge 600 ] && fail "no decided match in 600 s"
    sleep 2
  done
  # The filing: both tapes and both signed results.
  deadline=$((SECONDS + 30))
  until TA=$(tape a) && TB=$(tape b) && [ -n "$TA" ] && [ -n "$TB" ] \
      && [ -f "${TA%.bmrg}.attest.json" ] && [ -f "${TB%.bmrg}.attest.json" ]; do
    [ $SECONDS -ge $deadline ] && fail "the result was not filed on both phones"
    sleep 1
  done
  win=$(DISPLAY=$DISPLAY_NUM xdotool search --sync --onlyvisible --pid "$(pid_of a)" 2>/dev/null | head -1)
  DISPLAY=$DISPLAY_NUM xdotool windowfocus --sync "$win" keydown Escape sleep 0.2 keyup Escape
  wait_for_log a "leaving — last word sent" 10 || fail "A never left"
  wait_for_log b "peer ended the session" 10 || fail "B never heard A leave"
  has_log b "peer ended the session.*honoured=true" && fail "B let A's leaving undo a settled result"
  cmp -s "$TA" "$TB" || fail "the two tapes differ"
  for t in "$TA" "$TB"; do
    "$RS" --demo "$t" --attest "${t%.bmrg}.attest.json" > /dev/null 2>&1 || fail "$(basename "$t") does not verify"
  done
  read -r aw al au <<< "$(ledger a)"; read -r bw bl bu <<< "$(ledger b)"
  [ "$aw $al $au" = "$bl $bw $bu" ] && [ "$au" = 0 ] && [ $((aw + al)) = 1 ] \
    || fail "the ledgers disagree (A: $aw $al $au, B: $bw $bl $bu)"
  pass "A left the summary; ledgers agree (A $aw-$al, B $bw-$bl), tapes identical, both results verify"
  ;;
*) fail "unknown scenario $SCENARIO" ;;
esac
