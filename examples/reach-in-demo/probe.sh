#!/usr/bin/env bash
#
# probe.sh — point kriya-gateway's reach-in (Front 2) at a RUNNING macOS app and report what
# governed tools it can synthesize from that app's accessibility tree — with ZERO changes to the
# app. This is both the "try it on a real app" demo and the coverage-measurement harness.
#
#   ./probe.sh "Calculator"
#   ./probe.sh "Actual Budget"
#   ./probe.sh "Spent"
#
# Prereqs:
#   1. The target app is already OPEN.
#   2. Your terminal has Accessibility permission:
#      System Settings -> Privacy & Security -> Accessibility -> enable your terminal app.
#
# It runs READ-ONLY (initialize + tools/list); it performs no actions. To DRIVE a governed action,
# see the "Drive a governed action" section in this folder's README.md.
set -euo pipefail

APP="${1:?usage: ./probe.sh \"<App Name>\"  (the app must be open)}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"
CRATE="$REPO/crates/kriya"
GW="$CRATE/target/debug/kriya-gateway"
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

echo "==> Building kriya-gateway (reach-in feature)…"
cargo build --manifest-path "$CRATE/Cargo.toml" \
  --no-default-features --features mcp-client,reach-in --bin kriya-gateway

OUT="$(mktemp)"; ERR="$(mktemp)"; AUDIT="$(mktemp)"
trap 'rm -f "$OUT" "$ERR" "$AUDIT"' EXIT

echo "==> Reaching into '$APP' (read-only: initialize + tools/list)…"
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | "$GW" reach-in --app "$APP" --approval deny --policy "$SCRIPT_DIR/probe-policy.yaml" \
      --audit-log "$AUDIT" >"$OUT" 2>"$ERR" || { echo "reach-in failed:"; tail -3 "$ERR"; exit 1; }

OUT="$OUT" APP="$APP" python3 - <<'PY'
import json, os, collections
app, out = os.environ["APP"], os.environ["OUT"]
tools = []
for line in open(out):
    o = json.loads(line)
    if o.get("id") == 2:
        tools = o["result"]["tools"]
verbs = collections.Counter(t["name"].split("_", 1)[0] for t in tools)
press = [t for t in tools if t["name"].startswith("press_")]
print(f"\n=== Reach-in coverage for '{app}' ===")
print(f"total synthesized tools : {len(tools)}")
print(f"directly-pressable       : {len(press)}   (press_* — the operable controls)")
print(f"by action verb           : {dict(verbs.most_common())}")
print("\nsample pressable tools:")
for t in press[:20]:
    print("  ", t["name"], "—", t["description"])
if not tools:
    print("\n(0 tools — likely an Electron/Qt/web-rendered UI whose controls don't expose AX "
          "actions, or the app isn't focused/visible. This is the expected coverage limit.)")
PY
