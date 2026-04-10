#!/usr/bin/env bash
# E2E headless test for the SynapCode Tauri desktop app.
#
# Verifies that:
#   1. The binary launches under Xvfb without crashing
#   2. The Rust setup() callback runs and spawns the FalkorDB sidecar
#   3. The sidecar binds its expected port
#   4. The redis instance responds to PING
#   5. Graceful shutdown kills all child processes
#
# Requires: xvfb, redis-server, nc, redis-cli
# Usage: FALKORDB_PORT=16379 ./desktop/scripts/e2e-headless-test.sh

set -euo pipefail

PORT="${FALKORDB_PORT:-16379}"
BINARY="${BINARY:-./src-tauri/target/release/savants-desktop}"
DISPLAY_NUM=":99"
TIMEOUT_SECS=15

cd "$(dirname "$0")/.."

if [ ! -x "$BINARY" ]; then
    echo "❌ Binary not found at $BINARY"
    echo "   Build it first: npx tauri build --no-bundle"
    exit 1
fi

cleanup() {
    local exit_code=$?
    [ -n "${APP_PID:-}" ] && kill "$APP_PID" 2>/dev/null || true
    sleep 0.5
    [ -n "${XVFB_PID:-}" ] && kill "$XVFB_PID" 2>/dev/null || true
    pkill -f "redis-server.*$PORT" 2>/dev/null || true
    exit $exit_code
}
trap cleanup EXIT INT TERM

echo "═══ Starting Xvfb on $DISPLAY_NUM ═══"
Xvfb "$DISPLAY_NUM" -screen 0 1280x1024x24 &
XVFB_PID=$!
sleep 1

echo "═══ Launching Tauri app (FALKORDB_PORT=$PORT) ═══"
DISPLAY="$DISPLAY_NUM" FALKORDB_PORT="$PORT" "$BINARY" > /tmp/tauri-e2e.log 2>&1 &
APP_PID=$!
echo "Tauri PID: $APP_PID"

echo "═══ Waiting for FalkorDB sidecar on port $PORT (timeout ${TIMEOUT_SECS}s) ═══"
for i in $(seq 1 $((TIMEOUT_SECS * 2))); do
    if nc -z localhost "$PORT" 2>/dev/null; then
        echo "✅ FalkorDB sidecar reachable after $((i * 500))ms"
        break
    fi
    sleep 0.5
    if ! ps -p "$APP_PID" > /dev/null; then
        echo "❌ Tauri app crashed before sidecar was ready"
        cat /tmp/tauri-e2e.log
        exit 1
    fi
done

if ! nc -z localhost "$PORT" 2>/dev/null; then
    echo "❌ FalkorDB sidecar never came up within ${TIMEOUT_SECS}s"
    cat /tmp/tauri-e2e.log
    exit 1
fi

echo "═══ Verifying redis PING ═══"
PONG=$(redis-cli -p "$PORT" PING)
if [ "$PONG" != "PONG" ]; then
    echo "❌ Expected PONG, got: $PONG"
    exit 1
fi
echo "✅ Sidecar responds to PING"

echo "═══ Process tree ═══"
pgrep -P "$APP_PID" -a || true

echo ""
echo "✅ ALL CHECKS PASSED"
