// A deterministic, API-key-free "agent step" for the governed CI lane demo (S4).
//
// It routes a handful of tool calls through the runtime's `kriya-govern` binary (the exact
// per-call govern+sign path the SDK middleware uses), producing real Ed25519-signed, hash-chained
// receipts — no model, no network, no secrets, so it runs anywhere CI runs. `kriya-ci` wraps this
// step, then gates on the resulting receipts under the same policy.
//
// Env (set by `kriya-ci run`, with sensible fallbacks so it also runs standalone):
//   KRIYA_POLICY      path to the policy YAML         (default: ./policy.yaml next to this file)
//   KRIYA_AUDIT_LOG   where receipts are appended     (default: $TMPDIR/kriya-ci-demo.jsonl)
//   KRIYA_GOVERN_BIN  path to the kriya-govern binary (default: "kriya-govern" on PATH)

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";

const here = dirname(fileURLToPath(import.meta.url));
const policy = process.env.KRIYA_POLICY ?? join(here, "policy.yaml");
const auditLog = process.env.KRIYA_AUDIT_LOG ?? join(tmpdir(), "kriya-ci-demo.jsonl");
const bin = process.env.KRIYA_GOVERN_BIN ?? "kriya-govern";

// The agent's plan: read-only, side-effect-free tool calls — all allowed by the demo policy.
const PLAN = [
  ["read_file", { path: "README.md" }],
  ["list_dir", { path: "src" }],
  ["http_get", { url: "https://example.com/health" }],
  ["read_file", { path: "Cargo.toml" }],
];

const child = spawn(bin, ["--policy", policy, "--actor", "ci-agent", "--audit-log", auditLog], {
  stdio: ["pipe", "pipe", "inherit"],
});
const rl = createInterface({ input: child.stdout });
const pending = [];
rl.on("line", (line) => {
  const w = pending.shift();
  if (w) w(JSON.parse(line));
});
const send = (req) =>
  new Promise((resolve) => {
    pending.push(resolve);
    child.stdin.write(JSON.stringify(req) + "\n");
  });

let denied = 0;
for (const [action, params] of PLAN) {
  const gate = await send({ op: "check", action_id: action, params });
  if (gate.decision !== "allow") {
    // Record the blocked attempt as evidence (success:false) — the hook's "attempts are evidence"
    // discipline. This is what lets `kriya-ci` name the denied action in its verdict; an agent that
    // silently swallowed the block would only surface as the step's non-zero exit.
    await send({ op: "record", action_id: action, params, success: false });
    console.error(`agent: kriya blocked "${action}" (${gate.decision}) — blocked-attempt receipt signed`);
    denied++;
    continue;
  }
  // "Run" the tool (a no-op for the demo), then record the outcome as a signed receipt.
  await send({ op: "record", action_id: action, params, success: true });
  console.error(`agent: ${action} — allowed, ran, receipt signed`);
}

child.stdin.end();
child.kill();
// The agent itself exits 0 (it did its job); the governed CI lane's `kriya-ci` gate decides the
// build verdict from the signed receipts under the policy — including any block it recorded.
console.error(`agent: done — ${PLAN.length - denied} action(s) ran, ${denied} blocked. Receipts: ${auditLog}`);
process.exit(0);
