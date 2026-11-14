#!/usr/bin/env bash
# Two real peers, one real signaling server, nobody at either keyboard.
#
# Starts matchbox_server on localhost, then two desktop app processes in
# one quick-match room, each with its own home directory and the bot
# brain playing its local side (TWOTOP_AUTOPILOT: AUTOPILOT_A / AUTOPILOT_B
# set the tiers, empty for an idle seat). Strict, every step a FAIL:
#   1. both decide the match with no desync (ggrs exchanges checksums every
#      30 frames), on the same tick (both log the same rematch_at_frame);
#   2. nobody pressed RUN IT BACK, so neither restarts (a THROW in flight
#      at the decision must not become a rematch, SIM_VERSION 16);
#   3. both save a tape and a signed result; the tapes replay to the same
#      per-frame checksums and each attestation verifies against its tape;
#   4. THROW pressed in both windows (RUN IT BACK) restarts both, no desync.
#
# A manual tool, not yet a CI gate: under lavapipe a match takes minutes of
# wall time, and two deterministic bots sometimes settle into a symmetric
# orbit where neither lands a kill (raise TIMEOUT, or change the tiers).
#
# Needs: matchbox_server 0.14 on PATH (`cargo install matchbox_server
# --version ~0.14 --locked`), Xvfb, a Vulkan driver (lavapipe works:
# VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json), and built
# binaries (`cargo build -p app -p replay_sync`), xdotool for step 4.
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

declare -A PID
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
  PID[$peer]=$!
  sleep 1
done
strip() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }
fail() { echo "FAIL: $* (logs in $WORK)"; exit 1; }
check_health() {
  if grep -q "PANIC" "$WORK/a.log" "$WORK/b.log"; then fail "a peer panicked"; fi
  if grep -q "DESYNC DETECTED" "$WORK/a.log" "$WORK/b.log"; then
    grep -h "DESYNC DETECTED" "$WORK/a.log" "$WORK/b.log" | sed 's/\x1b\[[0-9;]*m//g' | cut -c1-160
    fail "the peers desynced"
  fi
}
restarts() { strip "$1" | grep -c "prev=MatchOver.*next=Countdown"; }

deadline=$((SECONDS + TIMEOUT))
# The tick a rematch may first fire: the decision's tick plus the sim's
# grace. Both peers simulate the same match, so both log the same value.
over() { strip "$1" | grep "next=MatchOver" | head -1 | sed -E 's/.*rematch_at_frame: ([0-9]+).*/\1/'; }
until [ -n "$(over "$WORK/a.log")" ] && [ -n "$(over "$WORK/b.log")" ]; do
  [ $SECONDS -ge $deadline ] && fail "no finished match on both peers within ${TIMEOUT}s"
  check_health
  sleep 2
done
FA=$(over "$WORK/a.log"); FB=$(over "$WORK/b.log")
[ "$FA" = "$FB" ] || fail "the peers decided different matches (rematch at $FA vs $FB)"
echo "OK: both peers decided the match (rematch opens at tick $FA), no desync"

# 2 + 3: the summary files the result. Poll for both tapes and both signed
# results rather than guessing how long that takes.
tape() { strip "$1" | grep "match replay saved" | tail -1 | sed -E 's/.*path=([^ ]+).*/\1/'; }
deadline=$((SECONDS + 30))
until TA=$(tape "$WORK/a.log") && TB=$(tape "$WORK/b.log") && [ -n "$TA" ] && [ -n "$TB" ] \
    && [ -f "${TA%.bmrg}.attest.json" ] && [ -f "${TB%.bmrg}.attest.json" ]; do
  [ $SECONDS -ge $deadline ] && fail "both tapes and both signed results not filed within 30s (tapes: '${TA:-}' '${TB:-}')"
  check_health
  [ "$(restarts "$WORK/a.log")" = 0 ] || fail "the match restarted from MatchOver with nobody pressing RUN IT BACK"
  sleep 1
done
[ "$(restarts "$WORK/a.log")$(restarts "$WORK/b.log")" = 00 ] \
  || fail "the match restarted from MatchOver with nobody pressing RUN IT BACK"

# The header carries each phone's own clock, so compare what the tapes
# replay to: the per-frame checksums of the match, re-simulated under the
# rules it was played by (--live). Each phone stops recording a moment
# after it saw the decision, so one tape can run a few ticks longer:
# compare the ticks both hold.
RS=${REPLAY_SYNC:-target/debug/replay_sync}
"$RS" --live --demo "$TA" --output "$WORK/a.tsv" > /dev/null 2>&1 || fail "replay_sync could not replay $TA"
"$RS" --live --demo "$TB" --output "$WORK/b.tsv" > /dev/null 2>&1 || fail "replay_sync could not replay $TB"
LA=$(wc -l < "$WORK/a.tsv"); LB=$(wc -l < "$WORK/b.tsv")
N=$(( LA < LB ? LA : LB ))
if [ "$N" -gt 1 ] && cmp -s <(head -n "$N" "$WORK/a.tsv") <(head -n "$N" "$WORK/b.tsv"); then
  echo "OK: both tapes replay to the same $((N - 1)) frames (tapes $((LA - 1)) / $((LB - 1)))"
else
  fail "the tapes replay differently (tapes $((LA - 1)) / $((LB - 1)))"
fi
for t in "$TA" "$TB"; do
  att="${t%.bmrg}.attest.json"
  "$RS" --demo "$t" --attest "$att" > /dev/null 2>&1 || fail "$(basename "$att") does not verify against its tape"
  echo "OK: $(basename "$att") verifies against its tape"
done

# 4: RUN IT BACK from both windows: THROW (Space) on the summary is
# consent; once both have consented the match restarts on both.
for peer in a b; do
  win=$(xdotool search --sync --onlyvisible --pid "${PID[$peer]}" 2>/dev/null | head -1)
  [ -n "$win" ] || fail "no window for peer $peer"
  xdotool windowfocus --sync "$win" keydown space sleep 0.3 keyup space
done
deadline=$((SECONDS + 20))
until [ "$(restarts "$WORK/a.log")" != 0 ] && [ "$(restarts "$WORK/b.log")" != 0 ]; do
  [ $SECONDS -ge $deadline ] && fail "RUN IT BACK in both windows did not restart both"
  check_health
  sleep 1
done
sleep 3
check_health
echo "OK: RUN IT BACK restarted both peers, no desync"
# A clean pass leaves nothing behind.
rm -rf "$WORK"
