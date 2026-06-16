/**
 * examples/node-sidecar-host — host the kriya **governed agent runtime from plain Node**.
 *
 * This is the *embedded sidecar* path (roadmap R3, hardened to Tauri parity in P0.5): your app's
 * own process spawns the `kriya-host` binary and drives a governed run over stdio. The agent
 * loop, the policy engine, human approval, the budget, and the signed audit log all live inside
 * that separate process — the UI/renderer can't tamper with them. Your process only ever runs the
 * typed actions the host has already cleared, the same handlers a button click would call.
 *
 * Every line here is byte-identical to what an **Electron main process** runs — Electron's main
 * IS Node. The two differences in a real Electron app are noted inline (search "In Electron").
 *
 *   node run.mjs            # deterministic, no API key (uses demo-script.json)
 *
 * Prereqs: build the host binary and the TS packages (see README.md).
 */

import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { existsSync } from "node:fs";

import { SidecarHost, runTask } from "kriya-sidecar";

const here = dirname(fileURLToPath(import.meta.url));

// 1. Locate the kriya-host binary (built from crates/kriya). Override with KRIYA_HOST_BIN.
const binaryPath =
  process.env.KRIYA_HOST_BIN ??
  join(here, "..", "..", "apps", "note-app", "src-tauri", "target", "debug", "kriya-host");

if (!existsSync(binaryPath)) {
  console.error(
    `\nkriya-host not found at:\n  ${binaryPath}\n\n` +
      `Build it first:\n` +
      `  (cd apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked)\n` +
      `or set KRIYA_HOST_BIN to your own built binary.\n`,
  );
  process.exit(1);
}

// 2. The app's own state + typed actions — the SAME handlers a human button would call. The
//    agent never touches this object directly; it only proposes an action id + params, and the
//    governed host decides whether that proposal is allowed to reach these handlers.
const notes = new Map();
let nextId = 1;
const actions = {
  create_note: ({ title }) => {
    const id = nextId++;
    notes.set(id, { id, title });
    return { id };
  },
  delete_note: ({ id }) => {
    notes.delete(id);
    return { deleted: id };
  },
};

// 3. Spawn the governed host. `--script` gives a deterministic, no-API-key run for the demo;
//    set AGENT_BACKEND=claude-cli|ollama|anthropic (env) to drive it with a real model instead.
const host = SidecarHost.spawn({
  binaryPath,
  args: ["--script", join(here, "demo-script.json")],
});

console.log("\n=== hosting kriya-host from Node — governance runs in the child process ===\n");

const done = await runTask(
  host,
  { goal: "tidy up the notes", state: { notes: [] }, tools: [] },
  {
    // Execute a *cleared* action against the app's real state; return the refreshed snapshot.
    dispatch: (req) => {
      const handler = actions[req.actionId];
      const data = handler ? handler(req.params) : undefined;
      console.log(`  [run]      ${req.actionId}(${JSON.stringify(req.params)})`);
      return {
        stepId: req.stepId,
        success: Boolean(handler),
        data,
        state: { notes: [...notes.values()] },
        error: handler ? undefined : `unknown action: ${req.actionId}`,
      };
    },
    // A policy-guarded action (delete_*) pauses the in-flight run HERE for a human. We grant it.
    // In Electron: forward this request to a renderer modal over IPC and resolve with its answer.
    approve: (req) => {
      console.log(`  [APPROVE]  "${req.actionId}" needs a human — granting (reason: ${req.reasoning})`);
      return true;
    },
    // Host telemetry + governance decisions. In Electron: pipe into the kriya-inspector log view.
    onLog: (entry) => console.log(`             - [${entry.level}] ${entry.message}`),
  },
);

console.log(`\n=== done: "${done.summary}" (${done.steps} step(s)) ===`);
console.log(`    final notes: ${JSON.stringify([...notes.values()])}\n`);

// 4. Durable memory over the sidecar protocol — the P0.5 Tauri-parity feature. This reads the
//    same episodic log Tauri reads via the agent_memory_recent command. In Electron: feed these
//    straight into the kriya-inspector <MemoryPanel/>.
const episodes = await host.recentMemory(5);
console.log(`=== recentMemory(): ${episodes.length} newest episode(s) (persists across runs) ===`);
for (const e of episodes) {
  const when = new Date(e.tsMs).toISOString();
  console.log(`    ${when}  ${e.actionId.padEnd(12)} ${e.success ? "ok " : "err"}  sig=${e.signature.slice(0, 12)}…`);
}
console.log("");

host.close();
