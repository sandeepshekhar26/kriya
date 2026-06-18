#!/usr/bin/env bash
# Screen-recordable demo of governed agent access to a Replicad CAD model. No API key, no setup —
# the OpenCascade kernel is bundled. Just: ./demo.sh
#
#   DEMO_SPEED=0 ./demo.sh       # fast dry-run (no pauses), to test before recording
#   CAD_FAKE=1   ./demo.sh       # skip the WASM kernel (analytic geometry) for a faster run
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"

echo "Preparing the demo (first run builds the SDK + governor)…"

# 1. SDK + this example (TypeScript)
( cd "$ROOT" && npm install >/dev/null 2>&1 && npm run build --workspace kriya-core >/dev/null 2>&1 ) || true
npm install >/dev/null 2>&1
npm run build >/dev/null 2>&1            # dist/handler.js for the --exec handler
node dist/handler.js --dump > tools.json 2>/dev/null   # refresh the tool manifest

# 2. The governor + the offline verifier (Rust, release). Prefer an installed binary.
KRIYA_MCP="$(command -v kriya-mcp || true)"
if [ -z "$KRIYA_MCP" ]; then
  ( cd "$ROOT/crates/kriya" && cargo build --release --bin kriya-mcp >/dev/null 2>&1 )
  KRIYA_MCP="$ROOT/crates/kriya/target/release/kriya-mcp"
fi

VERIFY="$(command -v verify-receipts || true)"
if [ -z "$VERIFY" ]; then
  CAND="$ROOT/tools/verify-receipts/target/release/verify-receipts"
  [ -x "$CAND" ] && VERIFY="$CAND" || VERIFY=""
fi

DEMO_SPEED="${DEMO_SPEED:-1.4}" python3 demo.py "$KRIYA_MCP" "$VERIFY"
