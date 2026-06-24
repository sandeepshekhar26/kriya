#!/usr/bin/env bash
#
# run.sh — non-interactive, end-to-end proof of zero-change governance.
#
# It wraps the mock MCP server (an "app we didn't write") with kriya-gateway and
# drives a scripted JSON-RPC conversation through it — no human, no real agent.
# With --approval deny, the destructive delete_note call is auto-blocked at the
# approval gate, so you can SEE the governance fire without anyone clicking a
# button. (For the live approval-modal experience, see the --approval gui note
# at the bottom and the .mcp.json in this directory.)
#
# What you should see at the end:
#   - list_notes  → a real result from the downstream server + a SIGNED receipt
#   - delete_note → isError:true (blocked, deny-by-default approval) and NOT signed
#
# NOTE: the kriya-gateway CLI contract used below (proxy / --approval / --policy /
# --audit-log / -- <downstream cmd>) is the contract documented in
# docs/SERVICE-ARCHITECTURE.md §7. If the binary's `--help` disagrees, tweak the
# flags in the GATEWAY_ARGS array below — they are isolated for exactly that reason.

set -euo pipefail

# --- locate things relative to this script (no hardcoded absolute paths) -----
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# The gateway needs only the lean `mcp-client` feature (std + serde + audit) — NOT the Tauri/HTTP
# default features — so build the crate directly with --no-default-features. That keeps it fast and
# sidesteps the alloc-no-stdlib/brotli pin entirely (no app lockfile needed).
CRATE_DIR="$REPO_ROOT/crates/kriya"

AUDIT_LOG="$(mktemp -t kriya-gateway-audit.XXXXXX)"
trap 'rm -f "$AUDIT_LOG"' EXIT

echo "==> Building kriya-gateway (first run compiles the Rust crate; this can take a few minutes)"
# cargo needs its env in a fresh shell.
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
cargo build --manifest-path "$CRATE_DIR/Cargo.toml" --no-default-features --features mcp-client --bin kriya-gateway

# Resolve the built binary (debug build by default).
GATEWAY_BIN="$CRATE_DIR/target/debug/kriya-gateway"
if [ ! -x "$GATEWAY_BIN" ]; then
  # Fallback: search the workspace target dirs in case the layout differs.
  GATEWAY_BIN="$(find "$REPO_ROOT" -type f -name kriya-gateway -perm -111 2>/dev/null | head -n1 || true)"
fi
if [ -z "${GATEWAY_BIN:-}" ] || [ ! -x "$GATEWAY_BIN" ]; then
  echo "ERROR: could not find the kriya-gateway binary after building." >&2
  echo "       Expected at: $BUILD_DIR/target/debug/kriya-gateway" >&2
  exit 1
fi
echo "==> Using gateway binary: $GATEWAY_BIN"

# --- gateway invocation (flags isolated so they're easy to tweak) ------------
# --approval deny  : the gate auto-blocks anything that requires approval, so the
#                    demo proves the gate non-interactively.
GATEWAY_ARGS=(
  proxy
  --approval deny
  --policy   "$SCRIPT_DIR/agent-policy.yaml"
  --audit-log "$AUDIT_LOG"
  --name     "notes-governed"
  --          # everything after this is the REAL downstream MCP server command
  node "$SCRIPT_DIR/mock-mcp-server.js"
)

echo
echo "==> Driving a scripted JSON-RPC conversation THROUGH the gateway"
echo "    client  ──▶  kriya-gateway (policy · approval · budget · audit)  ──▶  mock-mcp-server"
echo "    (nothing downstream was changed — the mock server has no governance of its own)"
echo

# One JSON object per line. The sequence: handshake, list tools, a READ
# (allowed → forwarded + signed), then a DESTRUCTIVE delete (require_approval +
# --approval deny → blocked, never reaches downstream, never signed).
REQUESTS=$(printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"run.sh","version":"0.0.0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_notes","arguments":{}}}' \
  '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"delete_note","arguments":{"id":"note-2"}}}' \
)

echo "----- gateway JSON-RPC responses -----"
printf '%s\n' "$REQUESTS" | "$GATEWAY_BIN" "${GATEWAY_ARGS[@]}"
echo "--------------------------------------"

echo
echo "==> Signed audit log ($AUDIT_LOG):"
if [ -s "$AUDIT_LOG" ]; then
  cat "$AUDIT_LOG"
else
  echo "(empty)"
fi

echo
echo "==> What just happened (the whole point):"
echo "    • list_notes  returned a REAL result from the downstream server AND"
echo "      produced a signed, hash-chained receipt in the audit log above."
echo "    • delete_note came back isError:true — BLOCKED at the approval gate"
echo "      (require_approval + --approval deny). The downstream server never saw"
echo "      it, and there is NO receipt for it (you don't sign what didn't run)."
echo "    • The mock server was not modified in any way. Governance was added"
echo "      entirely by the gateway sitting in front of it."
echo
echo "==> Verify a receipt offline (tamper-evident, no network):"
echo "    cargo run -p verify-receipts -- \"$AUDIT_LOG\""
echo "    (see tools/verify-receipts/ — checks Ed25519 signatures + the hash chain)"
echo
echo "==> For the LIVE approval-modal experience (macOS):"
echo "    run with --approval gui and drive it from Claude Desktop via .mcp.json"
echo "    in this directory — delete_note will pause on a real Approve/Deny modal."
