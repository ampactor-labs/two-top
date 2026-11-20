#!/usr/bin/env bash
# LIVE end to end: a broadcast duel, watched from its link while it plays.
#
# A local tape relay (tape_drop), two autopilot peers in one room, peer A
# armed with TWOTOP_GO_LIVE=1, and a third app opened on A's watch link
# (`twotop://watch/<id>`) the moment it exists. Strict:
#   1. A opens a live tape and goes on air when the duel starts; B is told
#      (both wear the LIVE mark);
#   2. the viewer rolls the tape while it is still open;
#   3. once the match is decided, the relay's live tape replays to exactly
#      the per-frame checksums of A's saved tape, for every frame of it (the
#      broadcast is the match, confirmed ticks only, nothing made up);
#   4. A's tape carries the signed result in its seal, and it verifies.
#
# Needs what two_peer_smoke.sh needs, plus a built tape_drop
# (`cargo build -p app -p replay_sync -p tape_drop`).
set -u
APP=${1:-target/debug/app}
DROP_BIN=${DROP_BIN:-target/debug/tape_drop}
TIMEOUT=${TIMEOUT:-300}
DROP_PORT=${DROP_PORT:-3581}
. "$(dirname "$0")/lib/peers.sh"
peers_init live-smoke "${PORT:-3580}" "${DISPLAY_NUM:-:89}"

PORT=$DROP_PORT TRUSTED_PROXY_HOPS=0 "$DROP_BIN" > "$WORK/drop.log" 2>&1 &
PIDS+=($!); disown
sleep 1
DROP="http://127.0.0.1:$DROP_PORT"
WATCH="http://127.0.0.1:9/watch"
curl -sf "$DROP/healthz" > /dev/null || fail "the relay did not start"

# ARENA=<tag> picks the table (vigil keeps the bots from orbiting).
table=()
[ -n "${ARENA:-}" ] && table=("TWOTOP_ARENA=$ARENA")
peer_start a TWOTOP_AUTOPILOT=8 TWOTOP_GO_LIVE=1 TWOTOP_DROP_URL=$DROP TWOTOP_WATCH_URL=$WATCH "${table[@]}"
peer_start b TWOTOP_AUTOPILOT=3 "${table[@]}" 

wait_for_log a "live tape open" 30 || fail "peer a never opened a live tape"
ID=$(strip a | grep "live tape open" | head -1 | grep -o '#watch=[0-9a-f]*' | cut -d= -f2)
[ -n "$ID" ] || fail "no watch id in the live tape log"
echo "OK: live tape $ID open"

wait_for_log a "on air" 120 || fail "peer a never went on air"
wait_for_log b "the peer is broadcasting this duel" 20 || fail "peer b was never told it is on air"
echo "OK: on air, and the peer knows"

# The viewer: its own home, no room, opened on the link.
mkdir -p "$WORK/c"
(
  export DISPLAY=$DISPLAY_NUM HOME="$WORK/c" XDG_CONFIG_HOME="$WORK/c/.config" \
    XDG_DATA_HOME="$WORK/c/.local/share" TWOTOP_WINDOW=270x585 \
    TWOTOP_DROP_URL=$DROP TWOTOP_FIRST_BLOOD=0
  unset TWOTOP_ROOM TWOTOP_AUTOSTART
  exec "$APP" "twotop://watch/$ID"
) > "$WORK/c.log" 2>&1 &
PIDS+=($!); disown
wait_for_log c "tape rolling" 90 || fail "the viewer never rolled the live tape"
OPEN=$(curl -s -D - -o /dev/null "$DROP/tape/$ID?from=999999999" | tr -d '\r' | grep -i '^x-live-open' | cut -d' ' -f2)
echo "OK: the viewer is watching (tape open: ${OPEN:-?})"

over() { strip "$1" | grep "next=MatchOver" | head -1; }
deadline=$((SECONDS + TIMEOUT))
until [ -n "$(over a)" ] && [ -n "$(tape a)" ]; do
  [ $SECONDS -ge $deadline ] && fail "no decided, saved match within ${TIMEOUT}s"
  check_health
  has_log c PANIC && fail "the viewer panicked"
  sleep 2
done
TA=$(tape a)
# Let the last confirmed second reach the relay.
sleep 3
curl -sf "$DROP/tape/$ID" -o "$WORK/broadcast.live" || fail "the relay lost the live tape"
"$RS" --live --demo "$TA" --output "$WORK/a.tsv" > /dev/null 2>&1 || fail "replay_sync could not replay $TA"
"$RS" --live --demo "$WORK/broadcast.live" --output "$WORK/live.tsv" > "$WORK/rs-live.log" 2>&1 \
  || fail "replay_sync could not replay the broadcast"
LA=$(wc -l < "$WORK/a.tsv")
LL=$(wc -l < "$WORK/live.tsv")
[ "$LL" -ge "$LA" ] || fail "the broadcast holds $LL rows, the match $LA"
head -n "$LA" "$WORK/live.tsv" | cmp -s - "$WORK/a.tsv" \
  || fail "the broadcast replays differently from the match it carried"
echo "OK: the broadcast replays to the match's $((LA - 1)) frames exactly"
deadline=$((SECONDS + 30))
until grep -qa SEAL "$TA"; do
  [ $SECONDS -ge $deadline ] && fail "the tape was never sealed with its signed result"
  sleep 1
done
"$RS" --demo "$TA" --attest > /dev/null 2>&1 || fail "the tape's seal does not verify"
echo "OK: the tape carries its signed result, and it verifies"
has_log c PANIC && fail "the viewer panicked"
pass "LIVE: broadcast, watched, and true to the match"
