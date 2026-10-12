#!/usr/bin/env bash
# Screenshots of onboarding, search and settings on
# Linux (X11), light and dark, for the Debian Screenshots workflow: the
# Linux twin of macos-screenshots.sh, on a virtual display.
#
# Needs target/debug/magi and magi-cli built, plus Xvfb, openbox, xdotool,
# feh and ImageMagick. Each theme gets a scratch profile (MAGI_CONFIG_DIR /
# MAGI_DATA_DIR), a three-file folder and no search features, so nothing
# downloads. Writes PNGs to $OUT.
set -euo pipefail

BIN=${BIN:-target/debug}
OUT=${OUT:-shots}
WORK=$(mktemp -d)
mkdir -p "$OUT" "$WORK/corpus"
export MAGI_FAKE_EMBEDDER=1
for i in 1 2 3; do echo "note $i about invoices" > "$WORK/corpus/n$i.txt"; done

# A virtual screen with a window manager, so windows get focus and a frame.
export DISPLAY=${DISPLAY:-:99}
Xvfb "$DISPLAY" -screen 0 1920x1080x24 &
sleep 2
openbox &
sleep 1

# A busy wallpaper, so a see-through window shows it. Best effort.
WALLPAPER=${WALLPAPER:-$PWD/fixtures/corpus/images/doroWallpaper.jpg}
feh --bg-fill "$WALLPAPER" || echo "could not set the wallpaper" >&2

# Where to click, in pixels from the window content's top-left (see
# macos-screenshots.sh; scale 1 on Xvfb). The footer's right button is Start
# on setup and Start searching on the finish screen.
NEXT_X=830
NEXT_Y=568
# "Start with your computer": turned off, so no autostart entry is written.
LOGIN_X=818
LOGIN_Y=361
# The settings sidebar: a 32px button per section, 2px apart, in SECTIONS order.
NAV_X=100
nav_y() { echo $((52 + 34 * $1)); }
SECTIONS=(folders features general appearance index)

pid=""
stop() {
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    pid=""
  fi
}
trap stop EXIT

launch() {
  "$BIN/magi" &
  pid=$!
  # A debug build on a software renderer takes a while to show its first window.
  sleep 15
  kill -0 "$pid" 2>/dev/null || { echo "magi exited on launch (its stderr is above)" >&2; exit 1; }
}

# Click at $1,$2 in the focused window's content (xdotool's --window
# coordinates are the client area's, the frame left out).
click() {
  local win
  win=$(xdotool getactivewindow)
  echo "click $1,$2 in window $win"
  xdotool mousemove --window "$win" "$1" "$2" click 1
  sleep 2
}

shot() {
  import -window root "$OUT/$1.png"
  echo "saved $OUT/$1.png"
}

for theme in light dark; do
  export MAGI_CONFIG_DIR="$WORK/$theme/cfg" MAGI_DATA_DIR="$WORK/$theme/data"
  mkdir -p "$MAGI_CONFIG_DIR" "$MAGI_DATA_DIR"
  cat > "$MAGI_CONFIG_DIR/config.toml" <<EOF
schema_version = 1

[features]
meaning = false
image_text = false
image_visual = false

[ui]
theme = "$theme"
onboarding = "background"
EOF
  "$BIN/magi-cli" roots add "$WORK/corpus"

  launch
  shot "$theme-1-setup"
  click "$LOGIN_X" "$LOGIN_Y"
  click "$NEXT_X" "$NEXT_Y"
  shot "$theme-2-finish"
  # Start searching: onboarding closes and search opens as the hotkey
  # opens it, so without the shortcut hint.
  click "$NEXT_X" "$NEXT_Y"
  shot "$theme-3-search-after-onboarding"
  stop

  # Opening Magi again once onboarding is done: search, with the hint.
  launch
  shot "$theme-4-search-on-launch"

  # Ctrl+, in search opens settings (search closes as it loses focus).
  xdotool key ctrl+comma
  sleep 3
  for i in "${!SECTIONS[@]}"; do
    click "$NAV_X" "$(nav_y "$i")"
    shot "$theme-5-settings-$((i + 1))-${SECTIONS[$i]}"
  done
  stop
  # Magi's own log rides along in the artifact (errors, what it saw).
  cp "$MAGI_DATA_DIR/logs/magi.log" "$OUT/$theme-magi.log" 2>/dev/null || true

  # The blur forced: openbox has no compositor, so this shows what Magi
  # paints when the blur it asked for never arrives.
  export MAGI_BACKDROP=blurred
  launch
  shot "$theme-8-blurred-search"
  xdotool key ctrl+comma
  sleep 3
  shot "$theme-9-blurred-settings-folders"
  click "$NAV_X" "$(nav_y 3)"
  shot "$theme-10-blurred-settings-appearance"
  stop
  unset MAGI_BACKDROP
done
