#!/usr/bin/env bash
# macOS checks for the macOS checks workflow: the parts of
# docs/qa-checklist.md's "macOS session" a hosted Mac can run, each with a
# pass/fail assertion and, where it helps, a screenshot. Not covered here
# (they need a real Mac): iCloud eviction (M12), a network share (M13),
# battery (M15), the blur (M19), the real Files and Folders consent prompt.
#
# Needs target/release/magi and magi-cli, and `cliclick` (brew). Runs with
# the fake embedder and no features, so nothing downloads. Writes PNGs,
# logs and summary.md to $OUT; exits non-zero if any check fails.
set -uo pipefail

BIN=${BIN:-target/release}
OUT=${OUT:-shots}
WORK=$(mktemp -d)
mkdir -p "$OUT"
export MAGI_FAKE_EMBEDDER=1
export MAGI_CONFIG_DIR="$WORK/cfg" MAGI_DATA_DIR="$WORK/data"
mkdir -p "$MAGI_CONFIG_DIR" "$MAGI_DATA_DIR"
LOG="$MAGI_DATA_DIR/logs/magi.log"

RESULTS=()
FAILED=0
# record <id> <pass|fail|unknown> <what was seen>
record() {
  local mark
  case $2 in
    pass) mark="✅" ;;
    fail) mark="❌"; FAILED=1 ;;
    *) mark="⚠️" ;;
  esac
  RESULTS+=("| $1 | $mark | $3 |")
  echo "[$2] $1: $3"
}

shot() { screencapture -x "$OUT/$1.png"; echo "saved $OUT/$1.png"; }

# wait_for <seconds> <command...>: true once the command succeeds.
wait_for() {
  local limit=$1; shift
  local start=$SECONDS
  until "$@" >/dev/null 2>&1; do
    [ $((SECONDS - start)) -ge "$limit" ] && return 1
    sleep 2
  done
}

pid=""
launch() {
  "$BIN/magi" "$@" &
  pid=$!
  sleep 6
}
stop() {
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    pid=""
  fi
}
trap 'stop; cp "$LOG" "$OUT/magi.log" 2>/dev/null || true' EXIT

frames() { grep -c "search window first frame" "$LOG" 2>/dev/null || echo 0; }
# Roots are stored canonical: macOS's /var is /private/var.
root_status() { "$BIN/magi-cli" roots list | awk -F'\t' -v p="$1" '$2 == p || $2 == "/private" p { print $4 }'; }
status_is() { [ "$(root_status "$1")" = "$2" ]; }
finds() { "$BIN/magi-cli" search "$1" | grep -q -- "$2"; }
finds_nothing() { "$BIN/magi-cli" search "$1" | grep -q "^no results"; }
# Owner of the topmost normal window on screen. Not System Events'
# "frontmost process": a menu-bar-only (Accessory) app never becomes that,
# whatever window it shows (the menu bar keeps the previous app's).
# 1 = kCGWindowListOptionOnScreenOnly, 16 = ExcludeDesktopElements.
top_window_pid() {
  osascript -l JavaScript -e '
    ObjC.import("CoreGraphics");
    const list = ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo(1 | 16, 0)));
    const top = list.find(w => w.kCGWindowLayer === 0);
    top ? String(top.kCGWindowOwnerPID) : "";' 2>/dev/null
}
# How many normal windows Magi has on screen.
magi_windows() {
  osascript -l JavaScript -e "
    ObjC.import('CoreGraphics');
    const list = ObjC.deepUnwrap(ObjC.castRefToObject(\$.CGWindowListCopyWindowInfo(1 | 16, 0)));
    String(list.filter(w => w.kCGWindowLayer === 0 && w.kCGWindowOwnerPID === $pid).length);" 2>/dev/null
}

# ---------------------------------------------------------------- folders
# Prepared before the first launch, so the app meets them as a user would.
PLAIN="$WORK/plain"
DENIED="$WORK/denied"
mkdir -p "$PLAIN/Thing.app/Contents/Resources" "$DENIED"
echo "invoices quarterly ledger" > "$PLAIN/ledger.txt"
echo "zebrabundle hidden inside an app" > "$PLAIN/Thing.app/Contents/Resources/inner.txt"
# A decomposed name (n + combining acute), as some macOS volumes store it.
printf 'nota\n' > "$PLAIN/$(printf 'cancio\xcc\x81n_descompuesta.txt')"
echo "grantedword in a folder that starts denied" > "$DENIED/later.txt"

hdiutil create -quiet -size 16m -fs HFS+ -volname MagiQA "$WORK/drive.dmg"
hdiutil attach -quiet "$WORK/drive.dmg"
DRIVE=/Volumes/MagiQA
echo "removableword on a drive" > "$DRIVE/on_drive.txt"

cat > "$MAGI_CONFIG_DIR/config.toml" <<EOF
schema_version = 1

[features]
meaning = false
image_text = false
image_visual = false

[ui]
theme = "dark"
onboarding = "background"
EOF
for root in "$PLAIN" "$DENIED" "$DRIVE"; do "$BIN/magi-cli" roots add "$root"; done
# As if the user had clicked "Don't Allow" for this folder.
chmod 000 "$DENIED"

# --------------------------------------------- onboarding, launch at login
# Onboarding's last step leaves "Start with your computer" on: finishing it
# must register the login item (M21). Footer button: see macos-screenshots.sh.
launch
shot "01-onboarding"
read -r wx wy <<<"$(osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid)
  set {x, y} to position of window 1
  set {w, h} to size of window 1
  return (x as text) & \" \" & ((y + h - 600) as text)
end tell" 2>/dev/null || echo "")"
if [ -n "${wx:-}" ]; then
  cliclick "c:$((wx + 830)),$((wy + 568))"; sleep 2
  cliclick "c:$((wx + 830)),$((wy + 568))"; sleep 3
  if osascript -e 'tell application "System Events" to get the name of every login item' 2>/dev/null | grep -qi magi \
    || ls "$HOME/Library/LaunchAgents/" 2>/dev/null | grep -qi magi; then
    record "M21 launch at login" pass "login item registered after onboarding"
  else
    record "M21 launch at login" fail "no login item or LaunchAgent named Magi after onboarding"
  fi
else
  record "M21 launch at login" unknown "no UI scripting on this runner: could not click through onboarding"
fi
stop

# ------------------------------------------------- the running app (M2, M3)
launch
sleep 4
shot "02-search-on-launch"
type=$(lsappinfo list | awk -v p="pid = $pid " 'index($0, p) { if (match($0, /type="[^"]*"/)) print substr($0, RSTART + 6, RLENGTH - 7) }')
case "$type" in
  UIElement|BackgroundOnly) record "M2 no Dock icon" pass "process type $type" ;;
  "") record "M2 no Dock icon" unknown "lsappinfo did not list pid $pid" ;;
  *) record "M2 no Dock icon" fail "process type $type (a Dock app is Foreground)" ;;
esac

# Row 10: type into search, then read the logged latency.
cliclick t:invoices; sleep 3
shot "03-search-typed"
typing=$(grep "search results after typing stopped" "$LOG" | tail -1 | grep -oE 'elapsed=[0-9.]+(ms|µs|s)' | head -1)
if [ -z "$typing" ]; then
  record "row 10 results after typing" unknown "no timing line in magi.log"
else
  ms=$(echo "$typing" | awk -F= '{ v=$2; if (v ~ /ms$/) { sub(/ms/, "", v); print v } else if (v ~ /µs$/) { sub(/µs/, "", v); print v/1000 } else { sub(/s/, "", v); print v*1000 } }')
  if awk -v m="$ms" 'BEGIN { exit !(m < 400) }'; then
    record "row 10 results after typing" pass "$ms ms (< 400)"
  else
    record "row 10 results after typing" fail "$ms ms (>= 400)"
  fi
fi

# M3: Cmd+, opens settings, and its window must be on top of the others.
cliclick kd:cmd t:, ku:cmd; sleep 3
shot "04-settings"
front=$(top_window_pid)
if [ -z "$front" ]; then
  record "M3 settings comes to the front" unknown "could not read the window list"
elif [ "$front" = "$pid" ]; then
  record "M3 settings comes to the front" pass "Magi owns the topmost window"
else
  record "M3 settings comes to the front" fail "topmost window belongs to pid $front ($(ps -o comm= -p "$front")), Magi is $pid"
fi

# --------------------------------------------------------- folders (M5-M16)
# The settings window stays open: the folder rows are the evidence.
if wait_for 60 status_is "$DENIED" permission_denied; then
  shot "05-settings-denied-folder"
  record "M5 denied folder reported" pass "status permission_denied"
else
  record "M5 denied folder reported" fail "status $(root_status "$DENIED")"
fi
chmod 755 "$DENIED"
if wait_for 75 finds grantedword later.txt; then
  record "M6 recovers once allowed" pass "indexed within 75 s of chmod 755, no restart"
else
  record "M6 recovers once allowed" fail "status $(root_status "$DENIED"), file not found after 75 s"
fi

if wait_for 60 finds removableword on_drive.txt; then
  hdiutil detach -quiet "$DRIVE"
  if wait_for 60 status_is "$DRIVE" missing && finds_nothing removableword; then
    shot "06-settings-drive-missing"
    hdiutil attach -quiet "$WORK/drive.dmg"
    if wait_for 75 finds removableword on_drive.txt; then
      record "M16 unplugged drive" pass "missing and hidden when ejected, found again when re-mounted"
    else
      record "M16 unplugged drive" fail "not found again after re-mounting"
    fi
  else
    record "M16 unplugged drive" fail "after ejecting: status $(root_status "$DRIVE"), results hidden: $(finds_nothing removableword && echo yes || echo no)"
  fi
else
  record "M16 unplugged drive" fail "the drive's file was never indexed"
fi

if wait_for 60 finds ledger ledger.txt; then
  # One item: found by its name, its insides never read. (The name used to
  # stay unfound: the bundle sat in the queue retrying a read of a folder.)
  if ! finds_nothing zebrabundle; then
    record "M14 .app is one item" fail "a file inside Thing.app was indexed"
  elif wait_for 30 finds Thing Thing.app; then
    record "M14 .app is one item" pass "found by name, its contents not indexed"
  else
    record "M14 .app is one item" fail "Thing.app is not found by name (still queued?)"
  fi
  # M20: search the precomposed spelling for a decomposed file name.
  if "$BIN/magi-cli" search "canción" | grep -q descompuesta; then
    record "M20 accented names" pass "precomposed query finds the decomposed file name"
  else
    record "M20 accented names" fail "\"canción\" does not find cancio+U+0301n_descompuesta.txt"
  fi
else
  record "M14 / M20" fail "the plain folder was never indexed"
fi

# Row 11: idle memory, all windows closed. Keystrokes go to the frontmost
# app (Finder, for a menu-bar-only app), so press each window's close button.
cliclick kp:esc; sleep 1
osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid)
  repeat with w in windows
    try
      click (first button of w whose subrole is \"AXCloseButton\")
    end try
  end repeat
end tell" 2>/dev/null
sleep 20
shot "09-idle-no-windows"
open_windows=$(magi_windows)
# Graded on the physical footprint, Activity Monitor's "Memory" and the
# closest to Windows' private working set. RSS also counts shared
# frameworks and GPU-mapped memory, so it is printed but not graded.
rss_mb=$(( $(ps -o rss= -p "$pid") / 1024 ))
# Where the footprint goes, region by region (NFR-1 is unmet on macOS:
# ~346 MB on 2026-11-06), kept in the artifact.
vmmap --summary "$pid" > "$OUT/vmmap-idle.txt" 2>&1 || true
# What the heap holds, by type: Core Text/Foundation objects (CFData,
# CTFont...) point at fonts and AppKit; "non-object" is Rust's own.
heap -sortBySize "$pid" > "$OUT/heap-idle.txt" 2>&1 || heap "$pid" > "$OUT/heap-idle.txt" 2>&1 || true
footprint_mb=$(vmmap --summary "$pid" 2>/dev/null | awk '/^Physical footprint:/ {
  v = $3; unit = substr(v, length(v)); n = substr(v, 1, length(v) - 1) + 0
  if (unit == "G") n *= 1024; else if (unit == "K") n /= 1024
  printf "%d", n; exit }')
if [ "${open_windows:-0}" -gt 0 ]; then
  record "row 11 idle memory" unknown "$open_windows Magi window(s) still open, not idle: footprint ${footprint_mb:-?} MB, RSS $rss_mb MB"
elif [ -z "$footprint_mb" ]; then
  record "row 11 idle memory" unknown "vmmap gave no footprint; RSS $rss_mb MB"
elif [ "$footprint_mb" -le 150 ]; then
  record "row 11 idle memory" pass "footprint $footprint_mb MB (<= 150); RSS $rss_mb MB"
else
  record "row 11 idle memory" fail "footprint $footprint_mb MB (> 150); RSS $rss_mb MB"
fi

# -------------------------------------------- shortcut and single instance
before=$(frames)
cliclick kd:shift,cmd kp:space ku:shift,cmd; sleep 3
if [ "$(frames)" -gt "$before" ]; then
  shot "07-search-from-hotkey"
  record "M11 Shift+Cmd+Space" pass "search opened from the global shortcut"
else
  record "M11 Shift+Cmd+Space" unknown "no new search window (synthetic keys may not reach global shortcuts on a VM)"
fi
cliclick kp:esc; sleep 1

# Row 9: every first frame logged so far.
worst=$(grep "search window first frame" "$LOG" | grep -oE 'elapsed=[0-9.]+(ms|µs)' | awk -F= '{ v=$2; if (v ~ /µs$/) { sub(/µs/, "", v); v=v/1000 } else sub(/ms/, "", v); if (v > m) m=v } END { print m+0 }')
if [ "$(frames)" -eq 0 ]; then
  record "row 9 first frame" unknown "no first-frame line in magi.log"
elif awk -v m="$worst" 'BEGIN { exit !(m < 150) }'; then
  record "row 9 first frame" pass "worst $worst ms over $(frames) opens (< 150)"
else
  record "row 9 first frame" fail "worst $worst ms (>= 150)"
fi

# Row 2: a plain second launch hands over to the first and exits.
"$BIN/magi" & second=$!
if wait_for 15 bash -c "! kill -0 $second"; then
  record "row 2 single instance" pass "second launch exited"
else
  record "row 2 single instance" fail "second launch still running after 15 s"
  kill "$second" 2>/dev/null
fi
cliclick kp:esc; sleep 1

# M10: a force-quit leaves the socket file; the next launch must replace it.
kill -9 "$pid"; wait "$pid" 2>/dev/null; pid=""
launch
cliclick kp:esc; sleep 1
before=$(frames)
"$BIN/magi" --toggle & toggler=$!
if wait_for 15 bash -c "! kill -0 $toggler" && sleep 2 && [ "$(frames)" -gt "$before" ]; then
  shot "08-toggle-after-force-quit"
  record "M10 after a force-quit" pass "relaunch claimed single-instance; --toggle opened search and exited"
else
  record "M10 after a force-quit" fail "--toggle did not reach the relaunched app"
  kill "$toggler" 2>/dev/null
fi
stop

# ------------------------------------------------- Ask Magi (rows 26, 28)
# Alt+Enter from a real search. With a test account in the
# MAGI_ASK_TEST_EMAIL / MAGI_ASK_TEST_PASSWORD secrets it then signs in and
# asks the deployed server, one question per run. No screenshot while the
# email is on screen: the artifact outlives the run.
ask_names() {
  osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid) to get name of every window" 2>/dev/null
}
ask_open() { ask_names | grep -q "Ask Magi"; }
config_has() { grep -q "$1" "$MAGI_CONFIG_DIR/config.toml"; }
# The dock's send arrow, measured from the window's bottom-right corner
# (it sits 45 pt in and 36 pt up). The search query is already in the box,
# and the first question also saves the disclosure as accepted.
ask_click_send() {
  local ax ay aw ah
  read -r ax ay aw ah <<<"$(osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid)
  set {x, y} to position of window \"Ask Magi\"
  set {w, h} to size of window \"Ask Magi\"
  return (x as text) & \" \" & (y as text) & \" \" & (w as text) & \" \" & (h as text)
end tell" 2>/dev/null || echo "")"
  [ -n "${ah:-}" ] && cliclick "c:$((ax + aw - 45)),$((ay + ah - 36))"
}

launch
sleep 4
cliclick t:invoices; sleep 3
shot "10-ask-search"
cliclick kd:alt kp:return ku:alt
if wait_for 10 ask_open; then
  shot "11-ask-opened"
  record "row 26 Alt+Enter" pass "the Ask Magi window opened"
  if [ -z "${MAGI_ASK_TEST_EMAIL:-}" ] || [ -z "${MAGI_ASK_TEST_PASSWORD:-}" ]; then
    record "row 28 Ask Magi flow" unknown "no test account secrets: sign-in and asking not run"
  else
    # Tab and Enter: covered by sign_in_works_from_the_keyboard_alone.
    cliclick "t:$MAGI_ASK_TEST_EMAIL" kp:tab "t:$MAGI_ASK_TEST_PASSWORD" kp:return
    if wait_for 30 config_has signed_in_as; then
      shot "12-ask-disclosure"
      ask_click_send
      if wait_for 10 config_has "disclosed = true"; then
        sleep 25
        shot "13-ask-answer"
        record "row 28 Ask Magi flow" unknown "signed in, asked, disclosure saved; grade 13-ask-answer.png by eye"
      else
        record "row 28 Ask Magi flow" fail "signed in, but sending the first question did not save the disclosure (see 12-ask-disclosure.png)"
      fi
    else
      record "row 28 Ask Magi flow" fail "not signed in after 30 s (a keychain prompt may block this runner)"
    fi
  fi
else
  shot "11-ask-not-opened"
  if [ -z "$(ask_names)" ]; then
    record "row 26 Alt+Enter" unknown "no window names to read (UI scripting?); see 11-ask-not-opened.png"
  else
    record "row 26 Alt+Enter" fail "no Ask Magi window within 10 s"
  fi
fi
stop

# Diagnosis for row 11, not graded: the footprint of a launch that never
# opens a window (`--background`, as at login). Close to the idle figure
# means the memory is taken at startup; far below means windows leave it.
"$BIN/magi" --background & pid=$!
sleep 30
bg_mb=$(vmmap --summary "$pid" 2>/dev/null | awk '/^Physical footprint:/ { print $3; exit }')
echo "background launch, no window ever opened: footprint ${bg_mb:-?}" | tee "$OUT/background-footprint.txt"
heap -sortBySize "$pid" > "$OUT/heap-background.txt" 2>&1 || true
stop

{
  echo "## macOS checks"
  echo
  echo "| Check | Result | Seen |"
  echo "| --- | --- | --- |"
  printf '%s\n' "${RESULTS[@]}"
  echo
  echo "Manual only (docs/qa-checklist.md): M12 iCloud, M13 network share, M15 battery, M19 blur, the real consent prompt, the keychain prompt."
} | tee "$OUT/summary.md" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"

exit $FAILED
