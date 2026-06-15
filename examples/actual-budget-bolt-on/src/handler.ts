/**
 * The persistent handler `kriya-mcp --persistent --exec` drives. It holds the (expensive)
 * Actual connection open for the whole session and answers one governed action per stdin line:
 *
 *   in   { "action": "categorize_transaction", "params": { "id": "...", "category": "..." } }
 *   out  { "success": true, "data": null }
 *
 * kriya-mcp owns the governance (policy → approval → budget → signed audit); this process only
 * holds the connection and runs the cleared action against Actual's in-process API.
 *
 * Run `node dist/handler.js --dump` to print the MCP tool schemas (for kriya-mcp's --tools)
 * without connecting to a budget.
 */

import { createInterface } from "node:readline";
import { dispatchAction, getToolSchemas } from "kriya-core";
import { loadActual, type ActualApi } from "./actual-api.js";
import { fakeActual } from "./fake-actual.js";
import { registerActualActions } from "./actions.js";

/** A do-nothing ActualApi so `--dump` can build the schemas without a live budget. */
function noopActual(): ActualApi {
  const noop = async () => null as never;
  return new Proxy({}, { get: () => noop }) as ActualApi;
}

async function connect(): Promise<ActualApi> {
  const actual = await loadActual();
  // Standard Actual env: a local data dir, and (optionally) a sync server + password.
  await actual.init({
    dataDir: process.env.ACTUAL_DATA_DIR,
    serverURL: process.env.ACTUAL_SERVER_URL,
    password: process.env.ACTUAL_PASSWORD,
  });
  // Server-backed budget: download it so queries/mutations work. ACTUAL_SYNC_ID is the budget's
  // Sync ID (Actual → Settings → Advanced → "Sync ID"). Omit it for a purely local dataDir.
  const syncId = process.env.ACTUAL_SYNC_ID;
  if (syncId) {
    await actual.downloadBudget(syncId, { password: process.env.ACTUAL_FILE_PASSWORD });
  }
  return actual;
}

async function main(): Promise<void> {
  const dump = process.argv.includes("--dump");

  // --dump: no connection needed. ACTUAL_FAKE: in-memory demo. Otherwise: a real budget.
  const fake = !!process.env.ACTUAL_FAKE;
  const actual = dump ? noopActual() : fake ? fakeActual() : await connect();
  registerActualActions(actual);

  if (dump) {
    process.stdout.write(JSON.stringify(getToolSchemas(), null, 2) + "\n");
    return;
  }

  // Against a real sync server, push changes after each successful write so the open Actual app
  // reflects them live (reads don't need a sync). The mock fund needs none of this.
  const syncAfterWrites = !fake && !!process.env.ACTUAL_SERVER_URL;

  // One request line → one response line, matching kriya-mcp's handler contract.
  const rl = createInterface({ input: process.stdin });
  for await (const line of rl) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    let req: { action?: string; params?: Record<string, unknown> };
    try {
      req = JSON.parse(trimmed);
    } catch {
      process.stdout.write(JSON.stringify({ success: false, error: "bad request JSON" }) + "\n");
      continue;
    }
    const action = req.action ?? "";
    const result = await dispatchAction(action, req.params ?? {}, { caller: "agent" });
    if (syncAfterWrites && result.success && !action.startsWith("list_")) {
      try {
        await actual.sync();
      } catch (err) {
        process.stderr.write(`[actual-bolt-on] sync failed: ${String(err)}\n`);
      }
    }
    process.stdout.write(
      JSON.stringify({ success: result.success, data: result.data ?? null, error: result.error ?? null }) +
        "\n",
    );
  }
}

main().catch((err) => {
  process.stderr.write(`[actual-bolt-on] fatal: ${err instanceof Error ? err.message : String(err)}\n`);
  process.exit(1);
});
