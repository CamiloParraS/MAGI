#!/usr/bin/env bash
# Screenshots of onboarding and search on macOS, light and dark, for the
# Screenshots workflow: a hosted Mac runner has a logged-in desktop.
#
# Needs target/debug/magi and magi-cli built, and `cliclick` (brew). Each
# theme gets a scratch profile (MAGI_CONFIG_DIR / MAGI_DATA_DIR), a three-file
# folder and no search features, so nothing downloads. Writes PNGs to $OUT.
set -euo pipefail

BIN=${BIN:-target/debug}
OUT=${OUT:-shots}
WORK=$(mktemp -d)
mkdir -p "$OUT" "$WORK/corpus"
export MAGI_FAKE_EMBEDDER=1
for i in 1 2 3; do echo "note $i about invoices" > "$WORK/corpus/n$i.txt"; done

# The onboarding window's content size (onboarding::SIZE), and where to
# click, in points from the content's top-left. The footer's right button
# is Start on setup and Start searching on the finish screen.
WIN_W=880
WIN_H=600
NEXT_X=830
NEXT_Y=568
# "Start with your computer": turned off, so no LaunchAgent is registered.
LOGIN_X=818
LOGIN_Y=361

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
  # A debug build takes a while to show its first window.
  sleep 10
}

# The content's top-left on screen, "x y". System Events gives the window's
# frame, title bar included, so the content is its bottom WIN_H points.
origin() {
  if osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid)
      set {x, y} to position of window 1
      set {w, h} to size of window 1
      return (x as text) & \" \" & ((y + h - $WIN_H) as text)
    end tell" 2>/dev/null; then
    return
  fi
  # No UI scripting on this runner: GPUI centers the window on the main
  # screen. The +14 (half a title bar) is a guess; check the shots.
  local bounds
  bounds=$(osascript -e 'tell application "Finder" to get bounds of window of desktop' | tr -d ,)
  read -r _ _ sw sh <<<"$bounds"
  echo "$(((sw - WIN_W) / 2)) $(((sh - WIN_H) / 2 + 14))"
}

click() {
  local ox oy
  read -r ox oy <<<"$(origin)"
  echo "click $1,$2 in a window at $ox,$oy"
  cliclick "c:$((ox + $1)),$((oy + $2))"
  sleep 2
}

shot() {
  screencapture -x "$OUT/$1.png"
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
  stop
done
