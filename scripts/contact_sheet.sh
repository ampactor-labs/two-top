#!/usr/bin/env bash
# Every screen of the game at four window sizes, captured headlessly
# (TWOTOP_CAPTURE on a manual clock) into target/contact-sheet/: one PNG
# per scene and size, and index.html laying them out in a grid. A visual
# change is reviewed by looking at this page before and after it.
#
# Usage: scripts/contact_sheet.sh [app-binary]   (needs Xvfb + Vulkan)
set -u
APP=${1:-target/debug/app}
OUT=${OUT:-target/contact-sheet}
DISPLAY_NUM=${DISPLAY_NUM:-:97}
rm -rf "$OUT"; mkdir -p "$OUT"
Xvfb "$DISPLAY_NUM" -screen 0 2200x2600x24 > /dev/null 2>&1 &
XVFB=$!
trap 'kill $XVFB 2>/dev/null' EXIT
sleep 2

SIZES="540x1170 720x1280 1024x768 900x600"
# name | extra environment | when to capture
SCENES=(
  "title||@150"
  "settings|TWOTOP_AUTOSTART=settings|@90"
  "replays|TWOTOP_AUTOSTART=replays|@90"
  "rivals|TWOTOP_AUTOSTART=rivals|@90"
  "name|TWOTOP_AUTOSTART=name|@90"
  "arenas|TWOTOP_AUTOSTART=arenas|@90"
  "practice|TWOTOP_PRACTICE=1 TWOTOP_AUTOSTART=1|@400"
  "summary|TWOTOP_PRACTICE=1 TWOTOP_AUTOSTART=1 TWOTOP_AUTOPILOT=8|@matchover+150"
)
failed=0
for size in $SIZES; do
  for scene in "${SCENES[@]}"; do
    IFS='|' read -r name extra when <<< "$scene"
    home="$OUT/home-$size-$name"; mkdir -p "$home"
    shot="$OUT/$name-$size.png"
    # shellcheck disable=SC2086
    env -u TWOTOP_ROOM DISPLAY="$DISPLAY_NUM" HOME="$home" XDG_CONFIG_HOME="$home/.config" \
      XDG_DATA_HOME="$home/.local/share" TWOTOP_WINDOW="$size" $extra \
      TWOTOP_CAPTURE="$shot$when" timeout 900 "$APP" > "$OUT/$name-$size.log" 2>&1
    if [ -s "$shot" ]; then
      echo "OK: $name at $size"
      rm -rf "$home" "$OUT/$name-$size.log"
    else
      echo "MISSING: $name at $size (log kept)"
      failed=1
    fi
  done
done

{
  echo "<!doctype html><meta charset=utf-8><title>2-Top contact sheet</title>"
  echo "<style>body{background:#100e22;color:#d2c49c;font:14px monospace}"
  echo "td{vertical-align:top;padding:6px}img{max-width:260px;image-rendering:pixelated}</style>"
  echo "<h1>2-Top contact sheet — $(git rev-parse --short HEAD 2>/dev/null)</h1><table><tr><th></th>"
  for size in $SIZES; do echo "<th>$size</th>"; done
  echo "</tr>"
  for scene in "${SCENES[@]}"; do
    name=${scene%%|*}
    echo "<tr><th>$name</th>"
    for size in $SIZES; do
      echo "<td><a href=\"$name-$size.png\"><img src=\"$name-$size.png\" alt=\"$name $size\"></a></td>"
    done
    echo "</tr>"
  done
  echo "</table>"
} > "$OUT/index.html"
echo "contact sheet: $OUT/index.html"
exit $failed
