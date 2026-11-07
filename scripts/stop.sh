#!/usr/bin/env bash
# Stops the cc-discord-presence daemon on Linux and macOS (Windows: stop.ps1).
#
# The daemon records its pid in an instance file, so that pid is tried first.
# The fallback matches the exact process name with `pgrep -x`, never the
# command line: the repository folder shares the daemon's name, and a command
# line match would also stop editors and shells that merely mention it.
set -u

NAME="cc-discord-presence"
# Linux truncates process names to 15 characters in `comm`.
SHORT_NAME="${NAME:0:15}"
CLAUDE_DIR="${CLAUDE_HOME:-$HOME/.claude}"
META_FILE="$CLAUDE_DIR/cc-discord-presence.instance.json"

is_daemon() {
  local comm
  comm="$(ps -p "$1" -o comm= 2>/dev/null | sed 's|.*/||')"
  [ "$comm" = "$NAME" ] || [ "$comm" = "$SHORT_NAME" ]
}

kill_by_metadata() {
  [ -f "$META_FILE" ] || return 1
  local pid
  pid="$(grep -o '"pid":[[:space:]]*[0-9]*' "$META_FILE" 2>/dev/null | grep -o '[0-9]*' | head -n1)"
  [ -n "$pid" ] || return 1
  # The recorded pid can belong to another program after a crash.
  is_daemon "$pid" || return 1
  kill "$pid" 2>/dev/null
}

kill_by_name() {
  local pids stopped=1 pid
  pids="$( (pgrep -x "$NAME"; pgrep -x "$SHORT_NAME") 2>/dev/null | sort -un)"
  for pid in $pids; do
    kill "$pid" 2>/dev/null && stopped=0
  done
  return "$stopped"
}

if kill_by_metadata; then
  echo "$NAME stopped"
elif kill_by_name; then
  echo "$NAME stopped (found by process name)"
else
  echo "$NAME is not running"
fi
