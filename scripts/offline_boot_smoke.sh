#!/usr/bin/env bash
# Builds with no room server: couch, practice and a launch carrying a join
# link must boot and run without a panic. (Netplay's resources are only
# added online; an always-on system reading one panicked on the first
# frame, and every online smoke was blind to it.)
#
# Usage: scripts/offline_boot_smoke.sh [app-binary]   (needs Xvfb + Vulkan)
set -u
APP=${1:-target/debug/app}
. "$(dirname "$0")/lib/peers.sh"
peers_init offline-boot "${PORT:-3545}" "${DISPLAY_NUM:-:92}"
for mode in couch practice join; do
  H="$WORK/$mode"; mkdir -p "$H"
  extra=(); args=()
  case $mode in
    practice) extra=(TWOTOP_PRACTICE=1 TWOTOP_AUTOSTART=1) ;;
    join) args=(--join CURS-pit) ;;
  esac
  env -u TWOTOP_ROOM DISPLAY="$DISPLAY_NUM" HOME="$H" XDG_CONFIG_HOME="$H/.config" \
    XDG_DATA_HOME="$H/.local/share" "${extra[@]}" timeout 20 "$APP" "${args[@]}" > "$WORK/$mode.log" 2>&1
  code=$?
  [ "$code" = 124 ] || fail "$mode exited early ($code)"
  has_log "$mode" "Encountered|panicked|PANIC" && fail "$mode panicked"
  echo "OK: $mode ran 20 s"
done
has_log join "join link accepted" || fail "the --join launch did not take its link"
pass "couch, practice and a --join launch all boot and run"
