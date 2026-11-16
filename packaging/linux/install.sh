#!/usr/bin/env sh
# Install 2-Top for this user from the unpacked archive: the game stays in
# this folder; a launcher, the icon and the twotop:// link handler go in
# ~/.local/share, so a join link opened in the browser lands in the game.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
icons="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"
mkdir -p "$apps" "$icons"
cp "$here/two-top.png" "$icons/two-top.png"
sed "s|^Exec=two-top|Exec=\"$here/two-top\"|" "$here/two-top.desktop" > "$apps/two-top.desktop"
update-desktop-database "$apps" 2>/dev/null || true
xdg-mime default two-top.desktop x-scheme-handler/twotop 2>/dev/null || true
echo "2-Top installed: launch it from your menu; twotop:// links now open it."
