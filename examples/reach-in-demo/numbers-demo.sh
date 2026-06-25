#!/usr/bin/env bash
#
# numbers-demo.sh — watch an agent operate Apple Numbers through kriya-gateway's reach-in (Front 2),
# with governance VISIBLE: one allowed action, one that pauses for your approval, one denied — every
# performed action signed, the audit log opened by an on-device attestation.
#
# PREREQS (both required):
#   1. Numbers is OPEN with a blank sheet (the default table selected → Format panel on the right).
#      Quick way:  open -a Numbers   (then pick "Blank")
#   2. Your terminal has Accessibility permission:
#      System Settings → Privacy & Security → Accessibility → enable your terminal app.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"
CRATE="$REPO/crates/kriya"
GW="$CRATE/target/debug/kriya-gateway"
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

echo "==> Building kriya-gateway (reach-in)…"
cargo build --manifest-path "$CRATE/Cargo.toml" \
  --no-default-features --features mcp-client,reach-in --bin kriya-gateway

KEY="$(mktemp)"; AUDIT="$(mktemp)"
rm -f "$KEY"   # let the gateway create+persist a fresh signing key here (pinned → attestation written)
trap 'rm -f "$KEY" "$AUDIT"' EXIT

cat <<'EOF'

You will see three governed agent actions against Numbers:
  1. press_radio_button_cell  → ALLOWED  (switches the Format inspector tab; no prompt) → SIGNED
  2. press_check_box_title     → GATED    (toggles the table Title) → a macOS APPROVAL MODAL pops:
        click Approve to let it run (then it's signed), or Deny to block it.
  3. press_button_add          → DENIED   (deny-by-default; never touches Numbers, never signed)

EOF
read -r -p "Press Return when Numbers is open with a blank sheet… " _

printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"press_radio_button_cell","arguments":{}}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"press_check_box_title","arguments":{}}}' \
  '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"press_button_add","arguments":{}}}' \
  | "$GW" reach-in --app "Numbers" \
      --approval gui \
      --policy "$SCRIPT_DIR/numbers-demo-policy.yaml" \
      --signing-key "$KEY" --audit-log "$AUDIT" \
      --actor claude-agent

echo
echo "=== Signed audit log (genesis = on-device attestation, then a receipt PER PERFORMED action) ==="
cat "$AUDIT"
echo
echo "=== Verify every receipt offline (no network) ==="
( cd "$REPO" && cargo run -q -p verify-receipts -- "$AUDIT" )
echo
echo "Note: the two blocked actions (approval-denied / policy-denied) are ABSENT — you don't sign what didn't run."
