#!/usr/bin/env bash
# Every way an online match can end, with two real peers (A at tier 8, B at
# tier 3, open ground). One scenario per run:
#
#   desync  A's sim is nudged off B's at tick 600 and A's checksum reports
#           are muted (debug builds: TWOTOP_FAULT), so only A's ggrs can see
#           it. B must still void the match within 2 s of A detecting it:
#           no tape, no ledger line on either phone.
#   kill    B is killed outright mid-match. A must notice within 3 s (the
#           connection closing, not the 9 s silence), wait the resume window
#           (shortened here) for B to come back, then book it unfinished.
#   early   B is killed before the first GO. Nothing was at stake: A books
#           nothing and goes back to the room.
#   freeze  B freezes for 12 s mid-match, then wakes. A's link times out at
#           ~9 s and it dials the resume room; B, waking to a dead link,
#           follows: the match resumes and is played to the end, the two
#           tapes identical and both signed results verifying.
#   relaunch B is killed outright mid-match and launched again 5 s later
#           (Android reclaiming the app during a call). B finds its
#           checkpoint and goes straight back to the resume room, where A
#           is waiting: the match resumes and finishes, as for freeze.
#   drop    Every UDP packet is dropped for 12 s mid-match (the network
#           going away under both phones). Both must resume the match and
#           finish it, as for freeze.
#   leave   A full match; then A presses LEAVE on the summary. Both ledgers
#           must agree, the two tapes must be the same bytes, both signed
#           results must verify, and B's card must say A left the table.
#
# Usage: scripts/endings_smoke.sh <scenario> [app-binary]
# Same needs as scripts/lib/peers.sh.
set -u
SCENARIO=${1:?usage: endings_smoke.sh desync|kill|early|freeze|relaunch|drop|leave [app]}
APP=${2:-target/debug/app}
. "$(dirname "$0")/lib/peers.sh"
peers_init "endings-$SCENARIO" "${PORT:-3541}" "${DISPLAY_NUM:-:96}"

case "$SCENARIO" in
  desync) peer_start a TWOTOP_AUTOPILOT=8 TWOTOP_ARENA=vigil TWOTOP_FAULT=desync@600 ;;
  kill) peer_start a TWOTOP_AUTOPILOT=8 TWOTOP_ARENA=vigil TWOTOP_RESUME_SECS=10 ;;
  *) peer_start a TWOTOP_AUTOPILOT=8 TWOTOP_ARENA=vigil ;;
esac
peer_start b TWOTOP_AUTOPILOT=3 TWOTOP_ARENA=vigil

started() { wait_for_log a "next=InRound" 120 && wait_for_log b "next=InRound" 30; }

# iptables as root, or through sudo (CI runners are not root).
ipt() { if [ "$(id -u)" = 0 ]; then iptables "$@"; else sudo iptables "$@"; fi; }

# After a resume: both phones finish the match, their tapes are the same
# bytes, both signed results verify, and the ledgers agree.
finish_resumed() {
  wait_for_log a "resumed — the match continues" 60 || fail "A never resumed the match"
  wait_for_log b "resumed — the match continues" 10 || fail "B never resumed the match"
  until has_log a "next=MatchOver" && has_log b "next=MatchOver"; do
    check_health
    has_log a "DESYNC DETECTED" && fail "the resumed match desynced"
    has_log b "DESYNC DETECTED" && fail "the resumed match desynced"
    [ $SECONDS -ge 900 ] && fail "the resumed match never finished"
    sleep 2
  done
  deadline=$((SECONDS + 30))
  until TA=$(tape a) && TB=$(tape b) && [ -n "$TA" ] && [ -n "$TB" ] \
      && grep -qa SEAL "$TA" && grep -qa SEAL "$TB"; do
    [ $SECONDS -ge $deadline ] && fail "the resumed result was not filed on both phones"
    sleep 1
  done
  cmp -s "$TA" "$TB" || fail "the two tapes of the resumed match differ"
  for t in "$TA" "$TB"; do
    "$RS" --demo "$t" --attest > /dev/null 2>&1 || fail "$(basename "$t")'s seal does not verify"
  done
  sleep 2
  read -r aw al au <<< "$(ledger a)"; read -r bw bl bu <<< "$(ledger b)"
  [ "$aw $al $au" = "$bl $bw $bu" ] && [ "$au" = 0 ] && [ $((aw + al)) = 1 ] \
    || fail "the ledgers disagree (A: $aw $al $au, B: $bw $bl $bu)"
}

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
  wait_for_log a "dialing the resume room" 12 || fail "A never noticed B was gone"
  took=$(python3 -c "print(round($(log_time a 'dialing the resume room')-$t0,2))")
  python3 -c "import sys; sys.exit(0 if $took <= 3.0 else 1)" \
    || fail "A noticed ${took}s after B died (budget 3 s)"
  wait_for_log a "giving up the resume" 20 || fail "A never gave up on a peer that is gone"
  sleep 3
  [ "$(ledger a)" = "0 0 1" ] || fail "A booked $(ledger a), expected one unfinished"
  pass "A noticed B was gone ${took}s after it died, waited for it, booked unfinished"
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
  finish_resumed
  pass "a 12 s freeze was resumed and the match finished; tapes identical, both results verify"
  ;;
relaunch)
  started || fail "the match never started"
  sleep 4
  kill -9 "$(pid_of b)"
  sleep 5
  peer_start b TWOTOP_AUTOPILOT=3 TWOTOP_ARENA=vigil
  wait_for_log b "going back in" 20 || fail "the relaunched B never found its checkpoint"
  finish_resumed
  pass "B was killed and relaunched; it went back in, the match resumed and finished"
  ;;
drop)
  started || fail "the match never started"
  sleep 3
  ipt -I INPUT -i lo -p udp -j DROP || fail "could not drop UDP (needs root or sudo)"
  sleep 12
  ipt -D INPUT -i lo -p udp -j DROP
  finish_resumed
  pass "12 s without a network was resumed and the match finished; tapes identical, both results verify"
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
      && grep -qa SEAL "$TA" && grep -qa SEAL "$TB"; do
    [ $SECONDS -ge $deadline ] && fail "the result was not filed on both phones"
    sleep 1
  done
  win=$(DISPLAY=$DISPLAY_NUM xdotool search --sync --onlyvisible --pid "$(pid_of a)" 2>/dev/null | head -1)
  # The summary ignores LEAVE for its first 1.6 s (a stray tap from the
  # last exchange must not end it), and the filing can finish well inside
  # that: press again until A leaves.
  for _ in 1 2 3 4 5; do
    DISPLAY=$DISPLAY_NUM xdotool windowfocus --sync "$win" keydown Escape sleep 0.2 keyup Escape
    wait_for_log a "leaving — last word sent" 2 && break
  done
  has_log a "leaving — last word sent" || fail "A never left"
  wait_for_log b "peer ended the session" 10 || fail "B never heard A leave"
  has_log b "peer ended the session.*honoured=true" && fail "B let A's leaving undo a settled result"
  cmp -s "$TA" "$TB" || fail "the two tapes differ"
  for t in "$TA" "$TB"; do
    "$RS" --demo "$t" --attest > /dev/null 2>&1 || fail "$(basename "$t")'s seal does not verify"
  done
  read -r aw al au <<< "$(ledger a)"; read -r bw bl bu <<< "$(ledger b)"
  [ "$aw $al $au" = "$bl $bw $bu" ] && [ "$au" = 0 ] && [ $((aw + al)) = 1 ] \
    || fail "the ledgers disagree (A: $aw $al $au, B: $bw $bl $bu)"
  pass "A left the summary; ledgers agree (A $aw-$al, B $bw-$bl), tapes identical, both results verify"
  ;;
*) fail "unknown scenario $SCENARIO" ;;
esac
