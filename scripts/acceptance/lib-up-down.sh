#!/usr/bin/env bash
# Boot/teardown helpers for the acceptance harness.
#
# Sourced by drive scripts. Provides:
#   up_mock              start the in-process mock-openai-service; export MOCK_BASE_URL
#   up_server <base_url> start `openlet-server serve` pointed at <base_url>; poll health
#   build_tui            build tui/dist/cli.mjs (the launcher is the only other build site)
#   down                 kill the tracked child PIDs directly + tmux kill-session
#
# Teardown kills only OUR tracked child PIDs (the proven pattern from
# tui/tests/e2e/spawn-real-server.ts) — never a broad pkill. A direct
# `kill <pid>` on a child we spawned is allowed; an agent-issued kill/pkill
# of arbitrary processes is what the sandbox blocks.
set -euo pipefail

ACC_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$ACC_ROOT/../.." && pwd)"
RUN_DIR="${ACC_RUN_DIR:-$REPO_ROOT/.openlet-run/acceptance}"
mkdir -p "$RUN_DIR"

PROFILE="${OPENLET_E2E_PROFILE:-debug}"
BIN_DIR="$REPO_ROOT/target/$PROFILE"
SERVER_BIN="$BIN_DIR/openlet-server"
MOCK_BIN="$BIN_DIR/mock-openai-service"

SERVER_LOG="$RUN_DIR/server.log"
MOCK_LOG="$RUN_DIR/mock.log"

# Tracked child PIDs — populated by up_* and killed by down.
SERVER_PID=""
MOCK_PID=""
ACC_TMUX_SESSION="${ACC_TMUX_SESSION:-openlet-acc}"

_require_bin() {
  if [ ! -x "$1" ]; then
    echo "missing binary: $1 (run: cargo build -p openlet-server -p openlet-test-mock-provider)" >&2
    return 1
  fi
}

# Start the mock LLM. It logs `listening at http://127.0.0.1:<port>/v1`.
# We parse that VERBATIM (already ends in /v1) into MOCK_BASE_URL.
up_mock() {
  _require_bin "$MOCK_BIN"
  : >"$MOCK_LOG"
  "$MOCK_BIN" >"$MOCK_LOG" 2>&1 &
  MOCK_PID=$!
  local deadline=$((SECONDS + 15))
  MOCK_BASE_URL=""
  while [ $SECONDS -lt $deadline ]; do
    if ! kill -0 "$MOCK_PID" 2>/dev/null; then
      echo "mock exited during startup; log:" >&2; tail -n 20 "$MOCK_LOG" >&2; return 1
    fi
    # Match the bound base URL (ends in /v1).
    local url
    url="$(grep -oE 'http://127\.0\.0\.1:[0-9]+/v1' "$MOCK_LOG" | tail -1 || true)"
    if [ -n "$url" ]; then MOCK_BASE_URL="$url"; break; fi
    sleep 0.2
  done
  if [ -z "$MOCK_BASE_URL" ]; then
    echo "mock did not announce a base URL within 15s; log:" >&2; tail -n 20 "$MOCK_LOG" >&2; return 1
  fi
  export MOCK_BASE_URL
  echo "mock up at $MOCK_BASE_URL (pid $MOCK_PID)"
}

# Start the server. $1 = model base URL (mock or OpenRouter). The server
# binds 127.0.0.1:0-style via OPENLET_BIND; caller passes a concrete port.
# Required env the caller must export beforehand:
#   OPENLET_BIND, OPENLET_DEFAULT_MODEL, OPENLET_WORKSPACE, OPENLET_DATA_DIR
#   and (real mode) OPENAI_API_KEY.
up_server() {
  _require_bin "$SERVER_BIN"
  local base_url="$1"
  : >"$SERVER_LOG"
  local bind="${OPENLET_BIND:-127.0.0.1:8788}"
  OPENAI_API_BASE_URL="$base_url" "$SERVER_BIN" serve >"$SERVER_LOG" 2>&1 &
  SERVER_PID=$!
  local health="http://$bind/v1/health"
  local deadline=$((SECONDS + 30))
  while [ $SECONDS -lt $deadline ]; do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
      echo "server exited during startup; log:" >&2; tail -n 30 "$SERVER_LOG" >&2; return 1
    fi
    if curl -sf "$health" >/dev/null 2>&1; then
      echo "server healthy at $health (pid $SERVER_PID)"; return 0
    fi
    sleep 0.3
  done
  echo "server did not become healthy within 30s; log:" >&2; tail -n 30 "$SERVER_LOG" >&2; return 1
}

build_tui() {
  if [ ! -f "$REPO_ROOT/tui/dist/cli.mjs" ] || [ "${ACC_REBUILD_TUI:-0}" = "1" ]; then
    echo "building tui/dist/cli.mjs…"
    ( cd "$REPO_ROOT/tui" && { [ -d node_modules ] || npm install; } && npm run build )
  fi
}

# Teardown: kill only our tracked children, then the tmux session.
down() {
  if tmux has-session -t "$ACC_TMUX_SESSION" 2>/dev/null; then
    tmux kill-session -t "$ACC_TMUX_SESSION" 2>/dev/null || true
  fi
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [ -n "$MOCK_PID" ] && kill -0 "$MOCK_PID" 2>/dev/null; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
}
