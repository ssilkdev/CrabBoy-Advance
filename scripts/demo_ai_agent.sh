#!/usr/bin/env bash
# Demo: AI Agent plays a real ROM against the mock vision endpoint.
#
#   ./scripts/demo_ai_agent.sh "/path/to/game.gba"
#
# Starts the mock server, runs the live harness, prints where the frames and
# transcript landed, then tears the server down.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$SCRIPT_DIR"

ROM="${1:-${GBA_TEST_ROM:-}}"
if [[ -z "$ROM" || ! -f "$ROM" ]]; then
    echo "usage: $0 /path/to/game.gba" >&2
    exit 1
fi

PORT="${PORT:-8099}"
FRAMES_DIR="${FRAMES_DIR:-/tmp/crabboy_ai_frames}"
PLAY_DIR="${PLAY_DIR:-/tmp/crabboy_ai_play}"

rm -rf "$FRAMES_DIR" "$PLAY_DIR"

echo "→ starting mock vision server on :$PORT"
python3 scripts/mock_vision_server.py --port "$PORT" --outdir "$FRAMES_DIR" &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true' EXIT

for _ in $(seq 1 30); do
    if curl -sf "http://127.0.0.1:$PORT/v1/models" >/dev/null; then break; fi
    sleep 0.2
done
echo "→ server ready"

echo "→ running the agent against $(basename "$ROM")"
GBA_TEST_ROM="$ROM" \
AI_AGENT_ENDPOINT="http://127.0.0.1:$PORT/v1/chat/completions" \
AI_AGENT_DECISIONS="${AI_AGENT_DECISIONS:-4}" \
AI_AGENT_OUTDIR="$PLAY_DIR" \
    cargo test --release --test ai_agent_live -- --ignored --nocapture

echo
echo "screenshots the model received : $FRAMES_DIR"
echo "frames at each decision point  : $PLAY_DIR"
echo "decision transcript            : $(ls -t recordings/ai_session_*.jsonl 2>/dev/null | head -1)"
