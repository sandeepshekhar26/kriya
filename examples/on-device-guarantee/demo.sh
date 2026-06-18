#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# R13 demo — the on-device guarantee ("nothing leaves the device", attested)
#
# A sealed policy (on_device: true) makes the in-process host:
#   • REFUSE to run with an inference backend that egresses to a remote service, and
#   • sign an ATTESTATION receipt that the run stayed on-device — verifiable offline.
#
# Two runs, same sealed policy:
#   A) on-device backend  (scripted / local model) → proceeds + attests
#   B) egressing backend  (AGENT_BACKEND=anthropic) → refused before any action runs
#
#   ./demo.sh
# ---------------------------------------------------------------------------
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
TAURI="$ROOT/apps/note-app/src-tauri"
HOST="$TAURI/target/debug/kriya-host"
VERIFY="$ROOT/tools/verify-receipts/target/debug/verify-receipts"

echo "==> Building binaries if needed"
[ -x "$HOST" ]   || ( cd "$TAURI" && cargo build -p kriya --bin kriya-host --locked )
[ -x "$VERIFY" ] || ( cd "$ROOT/tools/verify-receipts" && cargo build )

# ── Case A: on-device backend under the seal ────────────────────────────────
LOG_A="$HERE/demo-ondevice.jsonl"
rm -f "$LOG_A"
echo
echo "==> A) Sealed policy + on-device (scripted) backend — should PROCEED and ATTEST"
node "$HERE/drive.mjs" "$HOST" \
  --policy "$HERE/sealed-policy.yaml" \
  --script "$HERE/script.json" \
  --audit-log "$LOG_A"

echo
echo "   offline verification of the sealed run's audit log:"
"$VERIFY" "$LOG_A" | sed 's/^/   /'

echo "   attestation receipt:"
node -e '
  const fs=require("fs");
  const lines=fs.readFileSync(process.argv[1],"utf8").trim().split("\n").filter(Boolean).map(JSON.parse);
  const att=lines.find(r=>r.action_id==="kriya.attestation.on_device");
  if(!att){console.error("   !! no attestation found");process.exit(1);}
  console.log(`   • ${att.action_id}  egress=${att.params.egress}  profile=${att.params.network_profile}  signer=${att.public_key.slice(0,16)}…`);
' "$LOG_A"

# ── Case B: egressing backend under the same seal ───────────────────────────
LOG_B="$HERE/demo-egress.jsonl"
rm -f "$LOG_B"
echo
echo "==> B) Same sealed policy + egressing backend (AGENT_BACKEND=anthropic) — should be REFUSED"
# No API key is needed: the refusal happens at run start, before any inference call.
OUT_B="$(AGENT_BACKEND=anthropic node "$HERE/drive.mjs" "$HOST" \
  --policy "$HERE/sealed-policy.yaml" \
  --audit-log "$LOG_B" 2>&1)"
echo "$OUT_B" | sed 's/^/   /'

echo
if echo "$OUT_B" | grep -q "on-device guarantee violated"; then
  echo "   ✓ egressing backend refused under the seal"
else
  echo "   !! UNEXPECTED: egressing backend was not refused"
  exit 1
fi
if [ -s "$LOG_B" ]; then
  echo "   !! UNEXPECTED: a sealed-but-refused run wrote receipts"
  exit 1
else
  echo "   ✓ nothing signed — the refused run never acted"
fi

rm -f "$LOG_A" "$LOG_B"
echo
echo "Done. R13: a sealed run stays on-device and proves it; an egressing one cannot start."
