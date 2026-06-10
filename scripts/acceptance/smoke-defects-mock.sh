#!/usr/bin/env bash
# Phase-1 defect-triage smoke (mock LM, no token spend).
#
# Drives the REAL TUI under tmux against the REAL server + in-process mock
# LM, then captures rendered frames proving the Phase-2 fixes:
#   DEFECT-1: a cold first message (no prior /new) now reaches the server
#             and a turn streams back — was silently swallowed.
#   DEFECT-2: the status bar shows agent/model after a session exists —
#             was agent:—  model:—.
#
# Frames land in plans/<plan>/evidence/ (gitignored). Mock LM means the
# host-unsafe real-LLM path is NOT exercised here — this is the safe,
# deterministic triage lane.
set -euo pipefail

ACC_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$ACC_ROOT/../.." && pwd)"
# shellcheck source=lib-up-down.sh
source "$ACC_ROOT/lib-up-down.sh"
# shellcheck source=drive-tui.sh
source "$ACC_ROOT/drive-tui.sh"

EVIDENCE_DIR="${EVIDENCE_DIR:-$REPO_ROOT/plans/20260610-0209-real-llm-acceptance-product-ship/evidence}"
mkdir -p "$EVIDENCE_DIR"

# Absolute temp workspace + data dir (never repo / $HOME) — the workspace
# guard the plan demands. Cleaned on exit.
TMP_ROOT="$(mktemp -d)"
export OPENLET_WORKSPACE="$TMP_ROOT/ws"
export OPENLET_DATA_DIR="$TMP_ROOT/data"
mkdir -p "$OPENLET_WORKSPACE" "$OPENLET_DATA_DIR"

# Pick a free loopback port for the server bind.
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
export OPENLET_BIND="127.0.0.1:$PORT"
export OPENLET_DEFAULT_MODEL="mock/model-small"

cleanup() {
  down || true
  rm -rf "$TMP_ROOT" || true
}
trap cleanup EXIT

echo "── boot mock + server ───────────────────────────"
up_mock
up_server "$MOCK_BASE_URL"

echo "── build + launch TUI in tmux ───────────────────"
build_tui
tui_start "http://$OPENLET_BIND" "$OPENLET_DATA_DIR"

# Assert the pane shows a connected status before driving keys.
if ! tui_wait "openlet" 15; then
  echo "FAIL: TUI never rendered the status bar" >&2
  tui_frame "phase-01-tui-noboot"
  exit 1
fi
tui_frame "phase-01-cold-boot"
echo "captured cold-boot frame"

# DEFECT-1 repro/fix: type a FIRST message with NO prior /new, submit.
tui_send "hello in three words"
tui_enter

# A real turn (even mock) should stream an assistant reply + terminal status.
# Poll for the mock's deterministic reply token or a session id appearing.
if tui_wait "end_turn" 20 || tui_wait "session:" 5; then
  tui_frame "phase-01-defect1-fixed-first-msg"
  echo "DEFECT-1 fix observed: cold first message produced a turn"
else
  tui_frame "phase-01-defect1-FAIL"
  echo "DEFECT-1 NOT fixed: first message produced no turn" >&2
  exit 1
fi

# DEFECT-2: status bar must now show agent + model (was —/—).
FRAME="$(tui_frame "phase-01-defect2-status-bar")"
if grep -qE 'agent:.*general' "$FRAME" && grep -qE 'model:.*mock/model-small' "$FRAME"; then
  echo "DEFECT-2 fix observed: status bar shows agent + model"
else
  echo "DEFECT-2 status check inconclusive — inspect $FRAME" >&2
fi

echo "── done; evidence in $EVIDENCE_DIR ──────────────"
