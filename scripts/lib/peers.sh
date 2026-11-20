# Shared plumbing for the real-peer smokes: one room server, one X display,
# app peers each with its own home, and log helpers. Source it:
#
#   . "$(dirname "$0")/lib/peers.sh"
#   peers_init my-smoke 3540 :95       # work dir, room port, display
#   peer_start a TWOTOP_AUTOPILOT=8    # extra env per peer
#   wait_for_log a "next=InRound" 120 || fail "the match never started"
#
# Needs matchbox_server 0.15, Xvfb and a Vulkan driver (lavapipe works),
# xdotool for window input, and built binaries (`cargo build -p app -p
# replay_sync`). A failure keeps the work dir (logs, homes, tapes); a pass
# removes it (`pass`).

APP=${APP:-target/debug/app}
RS=${REPLAY_SYNC:-target/debug/replay_sync}

peers_init() {
  WORK=$(mktemp -d "${TMPDIR_SMOKE:-target}/$1.XXXXXX")
  PORT=$2
  DISPLAY_NUM=$3
  PIDS=()
  trap 'for p in "${PIDS[@]}"; do kill -CONT "$p" 2>/dev/null; kill "$p" 2>/dev/null; done' EXIT
  matchbox_server "127.0.0.1:$PORT" > "$WORK/signal.log" 2>&1 &
  PIDS+=($!); disown
  Xvfb "$DISPLAY_NUM" -screen 0 1280x1400x24 > /dev/null 2>&1 &
  PIDS+=($!); disown
  sleep 2
}

# peer_start <name> [VAR=value ...]: an app in the shared room, with its own
# home, playing on autopilot unless TWOTOP_AUTOPILOT is overridden.
peer_start() {
  local name=$1; shift
  mkdir -p "$WORK/$name"
  (
    export DISPLAY=$DISPLAY_NUM HOME="$WORK/$name" XDG_CONFIG_HOME="$WORK/$name/.config" \
      XDG_DATA_HOME="$WORK/$name/.local/share"
    export TWOTOP_ROOM="ws://127.0.0.1:$PORT/two-top?next=2" TWOTOP_AUTOSTART=1 TWOTOP_WINDOW=270x585
    for kv in "$@"; do export "${kv?}"; done
    exec "$APP"
  ) > "$WORK/$name.log" 2>&1 &
  eval "PID_$name=$!"
  PIDS+=($!); disown
  sleep 1
}

pid_of() { eval "echo \$PID_$1"; }

# kill_peer <name> [signal]: e.g. kill_peer b -9, kill_peer b -STOP.
kill_peer() { kill "${2:--TERM}" "$(pid_of "$1")" 2>/dev/null; }

# press <peer> <key>: hold a key in the peer's window. Without a window
# manager focus can miss, so callers check the effect and retry.
press() {
  local win
  win=$(DISPLAY=$DISPLAY_NUM xdotool search --sync --onlyvisible --pid "$(pid_of "$1")" 2>/dev/null | head -1)
  [ -n "$win" ] || return 1
  DISPLAY=$DISPLAY_NUM xdotool windowraise "$win" windowfocus --sync "$win" \
    keydown "$2" sleep 0.3 keyup "$2" 2>/dev/null
}

# room_of <peer>: the room URL the peer dialed (versioned by the app).
room_of() { strip "$1" | grep -o 'room=ws://[^ ]*' | tail -1 | cut -d= -f2-; }
strip() { sed 's/\x1b\[[0-9;]*m//g' "$WORK/$1.log"; }
has_log() { strip "$1" | grep -qE "$2"; }

# wait_for_log <peer> <regex> <seconds>: 0 once the line appears.
wait_for_log() {
  local deadline=$((SECONDS + $3))
  until has_log "$1" "$2"; do
    [ $SECONDS -ge $deadline ] && return 1
    sleep 0.5
  done
}

# log_time <peer> <regex>: the first matching line's timestamp, epoch secs.
log_time() {
  local line
  line=$(strip "$1" | grep -E "$2" | head -1 | cut -c1-27)
  [ -n "$line" ] || return 1
  python3 -c "import datetime,sys; print(datetime.datetime.fromisoformat(sys.argv[1].rstrip('Z')).replace(tzinfo=datetime.timezone.utc).timestamp())" "$line"
}

# ledger <peer>: "wins losses unfinished" from its career.json (0 0 0 if none).
ledger() {
  local f
  f=$(find "$WORK/$1" -name career.json 2>/dev/null | head -1)
  if [ -z "$f" ]; then echo "0 0 0"; return; fi
  python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get('wins',0), d.get('losses',0), d.get('unfinished',0))" "$f"
}

# tape <peer>: the last tape the peer saved.
tape() { strip "$1" | grep "match replay saved" | tail -1 | sed -E 's/.*path=([^ ]+).*/\1/'; }

check_health() {
  if has_log a PANIC || has_log b PANIC; then fail "a peer panicked"; fi
}

fail() {
  echo "FAIL: $* (logs in $WORK)"
  # The last lines of every log, so a CI failure explains itself.
  for log in "$WORK"/*.log; do
    [ -f "$log" ] || continue
    echo "--- tail of $(basename "$log")"
    sed 's/\x1b\[[0-9;]*m//g' "$log" | grep -v "^ \+at \|^ *[0-9]\+: " | tail -15
  done
  exit 1
}
pass() { echo "OK: $*"; rm -rf "$WORK"; }
