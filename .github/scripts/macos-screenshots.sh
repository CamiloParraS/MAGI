#!/usr/bin/env bash
# Screenshots of onboarding, search, settings and the menu-bar icon on
# macOS, light and dark, for the Screenshots workflows: a hosted Mac runner
# has a logged-in desktop.
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

# A busy wallpaper, so a see-through window shows it. Best effort: the shots
# are still taken over the runner's plain desktop if neither way is allowed.
WALLPAPER=${WALLPAPER:-$PWD/fixtures/corpus/images/doroWallpaper.jpg}
osascript -e "tell application \"System Events\" to tell every desktop to set picture to \"$WALLPAPER\"" 2>/dev/null   || osascript -e "tell application \"Finder\" to set desktop picture to POSIX file \"$WALLPAPER\""   || echo "could not set the wallpaper" >&2
sleep 2

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
# The settings window's content size (settings::SIZE). Its sidebar: 8pt
# padding, the "Settings" label (~28pt), then a 32pt button per section,
# 2pt apart, in SECTIONS order.
SET_W=900
SET_H=620
NAV_X=100
nav_y() { echo $((52 + 34 * $1)); }
SECTIONS=(folders features general appearance index)
# The menu-bar icon, where System Events cannot say: its spot on the
# 1920x1080 runner in the first run.
TRAY_X=1683
TRAY_Y=11

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

# The front window's content top-left on screen, "x y", for content $1 x $2
# points (default: onboarding). System Events gives the window's frame,
# title bar included, so the content is its bottom $2 points.
origin() {
  local cw=${1:-$WIN_W} ch=${2:-$WIN_H}
  if osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid)
      set {x, y} to position of window 1
      set {w, h} to size of window 1
      return (x as text) & \" \" & ((y + h - $ch) as text)
    end tell" 2>/dev/null; then
    return
  fi
  # No UI scripting on this runner: GPUI centers the window on the main
  # screen. The +14 (half a title bar) is a guess; check the shots.
  local bounds
  bounds=$(osascript -e 'tell application "Finder" to get bounds of window of desktop' | tr -d ,)
  read -r _ _ sw sh <<<"$bounds"
  echo "$(((sw - cw) / 2)) $(((sh - ch) / 2 + 14))"
}

# Click at $1,$2 in a window whose content is $3 x $4 (default: onboarding).
click() {
  local ox oy
  read -r ox oy <<<"$(origin "${3:-$WIN_W}" "${4:-$WIN_H}")"
  echo "click $1,$2 in a window at $ox,$oy"
  cliclick "c:$((ox + $1)),$((oy + $2))"
  sleep 2
}

# The menu-bar icon's middle, "x y": from System Events, else TRAY_X/Y.
tray_spot() {
  osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid)
      set {x, y} to position of menu bar item 1 of menu bar 2
      set {w, h} to size of menu bar item 1 of menu bar 2
      return ((x + w div 2) as text) & \" \" & ((y + h div 2) as text)
    end tell" 2>/dev/null || echo "$TRAY_X $TRAY_Y"
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

  # Cmd+, in search opens settings (search closes as it loses focus).
  cliclick kd:cmd t:, ku:cmd
  sleep 3
  for i in "${!SECTIONS[@]}"; do
    click "$NAV_X" "$(nav_y "$i")" "$SET_W" "$SET_H"
    shot "$theme-5-settings-$((i + 1))-${SECTIONS[$i]}"
  done

  # The menu-bar icon: its tooltip on hover, then its menu (a click opens
  # it on macOS), closed again with Esc.
  read -r tx ty <<<"$(tray_spot)"
  echo "menu-bar icon at $tx,$ty"
  cliclick "m:$tx,$ty"
  sleep 3
  shot "$theme-6-tray-tooltip"
  cliclick "c:$tx,$ty"
  sleep 2
  shot "$theme-7-tray-menu"
  cliclick kp:esc
  stop

  # The blurred background, which macOS does not get yet (ADR-0011: "if it
  # proves to be real vibrancy"): search and settings over the wallpaper.
  export MAGI_BACKDROP=blurred
  launch
  shot "$theme-8-blurred-search"
  cliclick kd:cmd t:, ku:cmd
  sleep 3
  shot "$theme-9-blurred-settings-folders"
  click "$NAV_X" "$(nav_y 3)" "$SET_W" "$SET_H"
  shot "$theme-10-blurred-settings-appearance"
  stop
  unset MAGI_BACKDROP
done
