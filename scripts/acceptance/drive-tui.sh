#!/usr/bin/env bash
# tmux send-keys + capture-pane driver for the real TUI.
#
# This is the ONLY genuinely-new driver in the acceptance harness: it
# exercises the real keyboard/TTY path that ink-testing-library and a
# PATH-shimmed `node` both bypass. Headless agent-API driving stays in the
# Rust LiveServer tier — do NOT reimplement it here.
#
# Sourced by acceptance drivers. Provides:
#   tui_start <base_url> [state_dir]  launch `node tui/dist/cli.mjs` in a
#                                     detached tmux pane with env via -e
#   tui_send "<text>"                 type text into the pane (no Enter)
#   tui_enter                         send a SEPARATE Enter keystroke
#   tui_submit "<text>"               tui_send + tui_enter (text then Enter)
#   tui_frame <label>                 capture-pane -p → evidence/<label>.txt
#   tui_wait "<substr>" [timeout_s]   poll capture-pane until substr appears
#
# Env injection uses `tmux new-session -e` (tmux >=3.0): a new pane inherits
# the tmux SERVER's env captured at server start, NOT the caller's exports.
# Without -e, a pre-existing tmux server hands the pane stale/absent vars and
# the TUI connects nowhere.
set -euo pipefail

_DRV_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
_DRV_REPO="$(cd "$_DRV_ROOT/../.." && pwd)"
ACC_TMUX_SESSION="${ACC_TMUX_SESSION:-openlet-acc}"
ACC_EVIDENCE_DIR="${ACC_EVIDENCE_DIR:-$_DRV_REPO/plans/20260610-0209-real-llm-acceptance-product-ship/evidence}"
mkdir -p "$ACC_EVIDENCE_DIR"

# Scrub secrets from any captured text before it lands on disk. Matches
# OpenRouter keys (sk-or-...) and absolute home paths. Mechanized, not manual.
_scrub() {
  sed -E -e 's/sk-or-[A-Za-z0-9_-]{20,}/sk-or-REDACTED/g' \
         -e 's#/home/[^/[:space:]]+#/home/REDACTED#g'
}

# $1 = base_url (e.g. http://127.0.0.1:8788), $2 = optional state dir.
tui_start() {
  local base_url="$1"
  local state_dir="${2:-}"
  command -v tmux >/dev/null 2>&1 || { echo "tmux not installed" >&2; return 1; }
  [ -f "$_DRV_REPO/tui/dist/cli.mjs" ] || { echo "tui/dist/cli.mjs missing — build first" >&2; return 1; }

  tmux has-session -t "$ACC_TMUX_SESSION" 2>/dev/null && tmux kill-session -t "$ACC_TMUX_SESSION"

  # Env via -e so the pane actually sees it (see header note).
  local -a env_flags=( -e "OPENLET_BASE_URL=$base_url" )
  [ -n "$state_dir" ] && env_flags+=( -e "OPENLET_STATE_DIR=$state_dir" )

  tmux new-session -d -s "$ACC_TMUX_SESSION" -x 120 -y 40 "${env_flags[@]}" \
    "node '$_DRV_REPO/tui/dist/cli.mjs'; echo TUI_EXITED; sleep 5"
}

# Type text WITHOUT submitting. `-l` sends the bytes literally (so "/help"
# is not interpreted as a tmux key name).
tui_send() {
  tmux send-keys -t "$ACC_TMUX_SESSION" -l "$1"
}

# A SEPARATE Enter keystroke. A combined send leaves text unsubmitted in
# the ink prompt buffer (observed this session).
tui_enter() {
  tmux send-keys -t "$ACC_TMUX_SESSION" Enter
}

tui_submit() {
  tui_send "$1"
  # Brief settle so the buffer registers before the separate Enter.
  sleep 0.3
  tui_enter
}

# Capture the current rendered frame to evidence/<label>.txt (scrubbed).
tui_frame() {
  local label="$1"
  local out="$ACC_EVIDENCE_DIR/${label}.txt"
  tmux capture-pane -t "$ACC_TMUX_SESSION" -p | _scrub >"$out"
  echo "$out"
}

# Poll capture-pane until <substr> appears or timeout. Returns 0 on hit.
# Real LLM is slow — default 30s.
tui_wait() {
  local substr="$1"
  local timeout="${2:-30}"
  local deadline=$((SECONDS + timeout))
  while [ $SECONDS -lt $deadline ]; do
    if tmux capture-pane -t "$ACC_TMUX_SESSION" -p 2>/dev/null | grep -qF "$substr"; then
      return 0
    fi
    sleep 0.5
  done
  echo "tui_wait timed out (${timeout}s) waiting for: $substr" >&2
  return 1
}
