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
# A CI gate on main and nightly (.github/workflows/netplay.yml, open ground,
# one retry for a timeout only): under lavapipe a match takes minutes, and
# two deterministic bots sometimes orbit without a kill (raise TIMEOUT, or
# change the tiers). ARENA=<tag> picks the table.
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
TIMEOUT=${TIMEOUT:-240}
. "$(dirname "$0")/lib/peers.sh"
peers_init two-peer-smoke "${PORT:-3536}" "${DISPLAY_NUM:-:91}"
# Different tiers per seat: the same bot in both mirrored seats plays a
# mirror match (fangs pass each other) and rounds stall out.
# STATE_LOG_DIR=<dir>: each peer logs every frame's state there
# (TWOTOP_STATE_LOG), for finding where a desync began.
for peer in a b; do
  if [ "$peer" = a ]; then tier=${AUTOPILOT_A-8}; else tier=${AUTOPILOT_B-3}; fi
  env=()
  [ -n "$tier" ] && env+=("TWOTOP_AUTOPILOT=$tier")
  [ -n "${ARENA:-}" ] && env+=("TWOTOP_ARENA=$ARENA")
  [ -n "${STATE_LOG_DIR:-}" ] && env+=("TWOTOP_STATE_LOG=$STATE_LOG_DIR/state_$peer.log")
  peer_start "$peer" "${env[@]}"
done
check_health() {
  if has_log a PANIC || has_log b PANIC; then fail "a peer panicked"; fi
  if has_log a "DESYNC DETECTED" || has_log b "DESYNC DETECTED"; then
    strip a | grep -h "DESYNC DETECTED" | cut -c1-160
    fail "the peers desynced"
  fi
}
restarts() { strip "$1" | grep -c "prev=MatchOver.*next=Countdown"; }

deadline=$((SECONDS + TIMEOUT))
# The tick a rematch may first fire: the decision's tick plus the sim's
# grace. Both peers simulate the same match, so both log the same value.
over() { strip "$1" | grep "next=MatchOver" | head -1 | sed -E 's/.*rematch_at_frame: ([0-9]+).*/\1/'; }
until [ -n "$(over a)" ] && [ -n "$(over b)" ]; do
  [ $SECONDS -ge $deadline ] && fail "no finished match on both peers within ${TIMEOUT}s"
  check_health
  sleep 2
done
FA=$(over a); FB=$(over b)
[ "$FA" = "$FB" ] || fail "the peers decided different matches (rematch at $FA vs $FB)"
echo "OK: both peers decided the match (rematch opens at tick $FA), no desync"

# 2 + 3: the summary files the result. Poll for both tapes and both signed
# results rather than guessing how long that takes.
deadline=$((SECONDS + 30))
until TA=$(tape a) && TB=$(tape b) && [ -n "$TA" ] && [ -n "$TB" ] \
    && [ -f "${TA%.bmrg}.attest.json" ] && [ -f "${TB%.bmrg}.attest.json" ]; do
  [ $SECONDS -ge $deadline ] && fail "both tapes and both signed results not filed within 30s (tapes: '${TA:-}' '${TB:-}')"
  check_health
  [ "$(restarts a)" = 0 ] || fail "the match restarted from MatchOver with nobody pressing RUN IT BACK"
  sleep 1
done
[ "$(restarts a)$(restarts b)" = 00 ] \
  || fail "the match restarted from MatchOver with nobody pressing RUN IT BACK"

# Both tapes hold exactly the confirmed ticks through the decision, so
# they must hold the same inputs. The header carries each phone's own
# clock, so compare what they replay to: per-frame checksums under the
# rules the match was played by (--live), every frame, same length.
"$RS" --live --demo "$TA" --output "$WORK/a.tsv" > /dev/null 2>&1 || fail "replay_sync could not replay $TA"
"$RS" --live --demo "$TB" --output "$WORK/b.tsv" > /dev/null 2>&1 || fail "replay_sync could not replay $TB"
LA=$(($(wc -l < "$WORK/a.tsv") - 1)); LB=$(($(wc -l < "$WORK/b.tsv") - 1))
[ "$LA" -gt 0 ] && cmp -s "$WORK/a.tsv" "$WORK/b.tsv" \
  || fail "the tapes replay differently (tapes $LA / $LB frames)"
echo "OK: both tapes replay to the same $LA frames"
for t in "$TA" "$TB"; do
  att="${t%.bmrg}.attest.json"
  "$RS" --demo "$t" --attest "$att" > /dev/null 2>&1 || fail "$(basename "$att") does not verify against its tape"
  echo "OK: $(basename "$att") verifies against its tape"
done

# 4: RUN IT BACK from both windows: THROW (Space) on the summary is
# consent; once both have consented the match restarts on both.
# Without a window manager focus can miss; press until the window's app
# logs the consent (a repeat press is the same consent, never a second one).
for peer in a b; do
  for _ in 1 2 3 4 5; do
    press "$peer" space
    wait_for_log "$peer" "rematch consent given" 1 && break
  done
  has_log "$peer" "rematch consent given" || fail "peer $peer never took the RUN IT BACK press"
done
deadline=$((SECONDS + 20))
until [ "$(restarts a)" != 0 ] && [ "$(restarts b)" != 0 ]; do
  [ $SECONDS -ge $deadline ] && fail "RUN IT BACK in both windows did not restart both"
  check_health
  sleep 1
done
sleep 3
check_health
pass "RUN IT BACK restarted both peers, no desync"
