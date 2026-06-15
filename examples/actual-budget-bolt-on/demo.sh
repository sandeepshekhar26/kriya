#!/usr/bin/env bash
# Screen-recordable demo of governed agent access to Actual Budget — on the MOCK fund.
# No real money, no Actual install, no API key. Just: ./demo.sh
#
#   DEMO_SPEED=0 ./demo.sh     # fast dry-run (no pauses), to test before recording
#   DEMO_SPEED=1.6 ./demo.sh   # slower pacing for a calmer recording
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"

echo "Preparing the demo (first run builds the SDK + governor — a minute or two)…"

# 1. SDK + this example (TypeScript)
( cd "$ROOT" && npm install >/dev/null 2>&1 && npm run build --workspace kriya-core >/dev/null 2>&1 )
npm run build >/dev/null 2>&1   # builds dist/handler.js for the --exec handler

# 2. The governor + the offline verifier (Rust, release). Prefer an installed binary.
KRIYA_MCP="$(command -v kriya-mcp || true)"
if [ -z "$KRIYA_MCP" ]; then
  ( cd "$ROOT/crates/kriya" && cargo build --release --bin kriya-mcp >/dev/null 2>&1 )
  KRIYA_MCP="$ROOT/crates/kriya/target/release/kriya-mcp"
fi

VERIFY="$(command -v verify-receipts || true)"
if [ -z "$VERIFY" ]; then
  ( cd "$ROOT/tools/verify-receipts" && cargo build --release >/dev/null 2>&1 ) || true
  CAND="$ROOT/tools/verify-receipts/target/release/verify-receipts"
  [ -x "$CAND" ] && VERIFY="$CAND" || VERIFY=""
fi

DEMO_SPEED="${DEMO_SPEED:-1.4}" python3 demo.py "$KRIYA_MCP" "$VERIFY"
