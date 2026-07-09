#!/usr/bin/env bash
# verify-hook-firing.sh — empirical check of two real, disputed-upstream Claude Code behaviors
# that can't be unit-tested (no `claude` binary in CI):
#
#   1. Does PreToolUse/PostToolUse fire reliably in headless mode (`claude -p`)?
#      (Upstream reports of unreliability: anthropics/claude-code#40506, #36071.)
#   2. Does it fire for a tool call made from WITHIN a subagent (the Task tool)?
#      (Upstream dispute: anthropics/claude-code#34692 — reported broken on some platforms as
#      recently as days before this check was written; possibly fixed in newer CLI versions.)
#
# This is deliberately NOT a `cargo test` under crates/kriya/tests/ — it drives the real `claude`
# CLI, isn't hermetic, costs a couple of real API calls, and has no `claude` binary available in
# CI. Run it locally, record the result (whichever way it comes out) in docs/SECURITY.md — an
# honest boundary is a boundary either way, not a thing to leave silently unverified.
#
# Uses a LOGGING-ONLY hook (never blocks, always exits 0) — deliberately NOT kriya-hook itself, to
# isolate "does Claude Code invoke the hook at all" from "does kriya-hook's own logic work" (the
# latter is covered by crates/kriya/tests/kriya_hook_smoke.rs). Does not touch $HOME or any real
# settings.json — injects the diagnostic hook for one invocation via `--settings <file>`, which
# Claude Code merges with the user's real settings (so real auth/credentials still apply).
set -euo pipefail

MODEL="${VERIFY_HOOK_MODEL:-claude-sonnet-5}"
TMPDIR=$(mktemp -d)
LOGFILE="$TMPDIR/hook-fired.log"
trap 'rm -rf "$TMPDIR"' EXIT

cat > "$TMPDIR/log-hook.sh" <<'HOOKEOF'
#!/usr/bin/env bash
payload=$(cat)
agent_id=$(printf '%s' "$payload" | grep -o '"agent_id":"[^"]*"' || printf 'agent_id:none')
tool_name=$(printf '%s' "$payload" | grep -o '"tool_name":"[^"]*"' || printf 'tool_name:none')
printf '%s event=%s %s %s\n' "$(date +%s.%N)" "${1:-unknown}" "$agent_id" "$tool_name" >> "$HOOK_LOG_FILE"
exit 0
HOOKEOF
chmod +x "$TMPDIR/log-hook.sh"

cat > "$TMPDIR/settings.json" <<EOF
{
  "hooks": {
    "PreToolUse": [{"hooks": [{"type": "command", "command": "HOOK_LOG_FILE=$LOGFILE $TMPDIR/log-hook.sh pre"}]}],
    "PostToolUse": [{"hooks": [{"type": "command", "command": "HOOK_LOG_FILE=$LOGFILE $TMPDIR/log-hook.sh post"}]}]
  }
}
EOF

echo "=== claude --version ==="
claude --version
echo "model under test: $MODEL"

echo
echo "=== Test 1: headless (claude -p) — does PreToolUse/PostToolUse fire for a plain Bash call? ==="
: > "$LOGFILE"
claude -p "Run exactly this command via Bash and nothing else: echo hook-firing-test-1" \
  --settings "$TMPDIR/settings.json" --allowedTools "Bash" --model "$MODEL" >/dev/null || true
echo "--- hook log ---"
cat "$LOGFILE" 2>/dev/null || echo "(empty)"
if grep -q 'event=pre' "$LOGFILE" 2>/dev/null && grep -q 'tool_name":"Bash"' "$LOGFILE" 2>/dev/null; then
  echo "RESULT 1: PASS — headless PreToolUse fired for a plain Bash call."
  RESULT1="PASS"
else
  echo "RESULT 1: FAIL — headless PreToolUse did NOT fire for a plain Bash call (matches issue #40506)."
  RESULT1="FAIL"
fi

echo
echo "=== Test 2: headless (claude -p) — does the hook fire for a tool call made from WITHIN a Task subagent? ==="
: > "$LOGFILE"
claude -p "You have a Task tool for delegating work to a subagent. For this request you MUST use the Task tool rather than running Bash yourself: spawn exactly one subagent and instruct it to run this Bash command: echo hook-firing-test-2-subagent" \
  --settings "$TMPDIR/settings.json" --allowedTools "Bash Task" --model "$MODEL" >/dev/null || true
echo "--- hook log ---"
cat "$LOGFILE" 2>/dev/null || echo "(empty)"
if grep -q 'event=pre' "$LOGFILE" 2>/dev/null && grep -q 'tool_name":"Bash"' "$LOGFILE" 2>/dev/null; then
  if grep 'tool_name":"Bash"' "$LOGFILE" | grep -qv 'agent_id:none'; then
    echo "RESULT 2: PASS — subagent-originated Bash call fired the hook, with agent_id populated."
    RESULT2="PASS (agent_id populated)"
  else
    echo "RESULT 2: PARTIAL — a Bash call fired the hook, but agent_id was not populated (can't confirm it originated inside the subagent vs. the main thread declining to delegate)."
    RESULT2="PARTIAL"
  fi
else
  echo "RESULT 2: FAIL — no subagent-originated Bash call was observed (matches issue #34692, OR the model didn't use Task — inconclusive without a transcript check)."
  RESULT2="FAIL"
fi

echo
echo "=== Summary (record this in docs/SECURITY.md) ==="
echo "claude --version: $(claude --version)"
echo "headless PreToolUse firing:            $RESULT1"
echo "subagent-originated tool call firing:  $RESULT2"
