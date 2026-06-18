#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# R8 demo — agent + user identity per action (the signed-receipt `actor` field)
#
# Drives the REAL kriya-mcp binary as an external agent ("claude-desktop", acting
# for operator "alice") over MCP, then verifies the resulting audit log OFFLINE and
# shows that every receipt is cryptographically attributed to who took the action.
#
# The point: attribution lives INSIDE the signed bytes, so you cannot rewrite
# who-did-what without invalidating the Ed25519 signature.
#
#   ./demo.sh
# ---------------------------------------------------------------------------
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
TAURI="$ROOT/apps/note-app/src-tauri"
MCP="$TAURI/target/debug/kriya-mcp"
VERIFY="$ROOT/tools/verify-receipts/target/debug/verify-receipts"
LOG="$HERE/demo-audit.jsonl"

echo "==> Building binaries if needed"
[ -x "$MCP" ]    || ( cd "$TAURI" && cargo build -p kriya --bin kriya-mcp --locked )
[ -x "$VERIFY" ] || ( cd "$ROOT/tools/verify-receipts" && cargo build )

rm -f "$LOG"

echo
echo "==> An external agent (claude-desktop / alice) drives two governed actions over MCP"
echo "    policy: categorize_* allowed, delete_* requires approval (auto-approved here)"
echo

# One JSON-RPC session: initialize, then two tools/call. stdout (the protocol stream) is
# discarded; the governance decisions kriya-mcp logs to stderr stay visible.
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"claude-desktop","version":"1.0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"categorize_transaction","arguments":{"id":"txn-1"}}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"delete_transaction","arguments":{"id":"txn-2"}}}' \
  | "$MCP" \
      --tools "$HERE/tools.json" \
      --policy "$HERE/agent-policy.yaml" \
      --exec "node $HERE/handler.mjs" \
      --approval auto \
      --actor "claude-desktop" \
      --user "alice" \
      --audit-log "$LOG" \
      --name "actor-demo" \
  >/dev/null

echo
echo "==> Offline verification of the signed audit log"
"$VERIFY" "$LOG"

echo
echo "==> Who took each action (read back from the signed receipts):"
node -e '
  const fs = require("fs");
  const lines = fs.readFileSync(process.argv[1], "utf8").trim().split("\n").filter(Boolean);
  for (const l of lines) {
    const r = JSON.parse(l);
    const who = r.actor ? `${r.actor.agent} / ${r.actor.user}` : "(unattributed)";
    console.log(`   • ${r.action_id.padEnd(24)} ${r.success ? "ok " : "ERR"}  actor=${who}`);
  }
' "$LOG"

echo
echo "==> Tamper check: rewrite the operator on the first receipt, re-verify (must FAIL)"
node -e '
  const fs = require("fs");
  const lines = fs.readFileSync(process.argv[1], "utf8").trim().split("\n").filter(Boolean);
  const r = JSON.parse(lines[0]);
  r.actor.user = "mallory";               // forge who acted — signature no longer matches
  lines[0] = JSON.stringify(r);
  fs.writeFileSync(process.argv[2], lines.join("\n") + "\n");
' "$LOG" "$HERE/demo-tampered.jsonl"

if "$VERIFY" "$HERE/demo-tampered.jsonl"; then
  echo "!! UNEXPECTED: tampered log verified — attribution is NOT protected"
  exit 1
else
  echo "   ✓ tampered attribution rejected — who-did-what is signed, not forgeable"
fi

rm -f "$HERE/demo-tampered.jsonl"
echo
echo "Done. R8: every action is attributed to an agent + operator, tamper-evidently."
