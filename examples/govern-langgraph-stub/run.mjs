// govern-langgraph-stub — a STUB agent (no LLM, no API keys) that drives kriya-agents' governed
// tools exactly the way a LangGraph ToolNode would `invoke` them, so you can see the whole path end
// to end: policy → (approval) → budget → a signed receipt per tool call, produced by the runtime's
// `kriya-govern` binary (kriya-agents signs nothing itself).
//
// Run (after `cargo build -p kriya --bin kriya-govern` and `npm -w kriya-agents run build`):
//   node examples/govern-langgraph-stub/run.mjs
//
// Then re-verify the receipts it wrote, offline, with the runtime's verifier or the kriya Console.

import { existsSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { GovernClient, GovernDenied } from "kriya-agents";
import { governTool } from "kriya-agents/langgraph";

const here = dirname(fileURLToPath(import.meta.url));

// Locate the built kriya-govern binary (override with KRIYA_GOVERN_BIN).
const BIN =
  process.env.KRIYA_GOVERN_BIN ??
  [
    join(here, "../../apps/note-app/src-tauri/target/debug/kriya-govern"),
    join(here, "../../crates/kriya/target/debug/kriya-govern"),
    join(here, "../../target/debug/kriya-govern"),
  ].find(existsSync);

if (!BIN || !existsSync(BIN)) {
  console.error("kriya-govern not found — build it: cargo build -p kriya --bin kriya-govern");
  process.exit(1);
}

// Point the audit log at ~/.kriya/audit so the Console tails + re-verifies it live (falls back to a
// temp file if ~/.kriya isn't set up).
const auditDir = existsSync(join(homedir(), ".kriya")) ? join(homedir(), ".kriya", "audit") : tmpdir();
const auditLog = join(auditDir, "langgraph-stub.jsonl");

const client = new GovernClient({
  binaryPath: BIN,
  policyPath: join(here, "agent-policy.yaml"),
  actor: "langgraph",
  user: process.env.USER ?? "demo",
  auditLog,
});

// Two "tools", wrapped so LangChain's `tool(fn, …)` would make them governed StructuredTools.
const webSearch = governTool(client, "web_search", async ({ q }) => {
  return [`(stub) top result for "${q}"`, "(stub) second result"];
});
const deleteFiles = governTool(client, "delete_files", async ({ glob }) => {
  return `deleted ${glob}`; // never reached — policy denies this action
});

// The "agent loop": a fixed plan a real LLM would otherwise choose.
console.log(`audit log → ${auditLog}\n`);

const found = await webSearch({ q: "kriya governance" });
console.log("web_search →", found);

try {
  await deleteFiles({ glob: "/**" });
  console.log("delete_files → (unexpectedly ran)");
} catch (err) {
  if (err instanceof GovernDenied) {
    console.log(`delete_files → DENIED by policy (${err.decision}) — no receipt signed, agent adapts`);
  } else {
    throw err;
  }
}

const again = await webSearch({ q: "signed receipts" });
console.log("web_search →", again);

client.close();
console.log(`\nDone. ${2} signed receipts in ${auditLog} — re-verify them in the Console or with tools/verify-receipts.`);
