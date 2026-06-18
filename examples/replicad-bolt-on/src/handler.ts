/**
 * The persistent handler `kriya-mcp --persistent --exec` drives. It holds the CAD model + geometry
 * kernel in memory for the whole session and answers one governed action per stdin line:
 *
 *   in   { "action": "set_parameter", "params": { "name": "width", "value": 100 } }
 *   out  { "success": true, "data": { ... } }
 *
 * kriya-mcp owns the governance (policy → approval → budget → signed audit); this process only holds
 * the model and runs the cleared action against the CAD app's in-process functions.
 *
 * Run `node dist/handler.js --dump` to print the MCP tool schemas (for kriya-mcp's --tools).
 */

import { createInterface } from "node:readline";
import { dispatchAction, getToolSchemas } from "kriya-core";
import { CadApp } from "./cad.js";
import { analyticKernel, loadReplicadKernel, type CadKernel } from "./kernel.js";
import { registerCadActions } from "./actions.js";

// kriya-mcp expects line-delimited JSON on stdout. The OpenCascade WASM (and any console.log)
// can write to stdout, which would corrupt the protocol — capture stdout for our responses and
// redirect everything else to stderr.
const stdoutWrite = process.stdout.write.bind(process.stdout);
process.stdout.write = process.stderr.write.bind(process.stderr) as typeof process.stdout.write;
console.log = (...args: unknown[]) => console.error(...args);
console.info = (...args: unknown[]) => console.error(...args);

// kriya-core 0.0.1's getToolSchemas() leaks the per-parameter `required: true` flag INTO each
// property subschema. JSON Schema draft 2020-12 only allows `required` as an array at the object
// level (which getToolSchemas also emits), so a boolean `required` inside a property makes the
// Anthropic API reject the tool ("input_schema ... must match draft 2020-12"). Strip it so the
// dumped tools.json is valid. (The real fix belongs upstream in kriya-core.)
function sanitizeSchemas(schemas: unknown[]): unknown[] {
  for (const tool of schemas as Array<{ inputSchema?: { properties?: Record<string, unknown> } }>) {
    const props = tool.inputSchema?.properties;
    if (!props) continue;
    for (const key of Object.keys(props)) {
      const prop = props[key];
      if (prop && typeof prop === "object") delete (prop as Record<string, unknown>).required;
    }
  }
  return schemas;
}

async function selectKernel(): Promise<CadKernel> {
  if (process.env.CAD_FAKE) return analyticKernel();
  try {
    return await loadReplicadKernel();
  } catch (err) {
    process.stderr.write(
      `[replicad-bolt-on] Replicad kernel unavailable, using analytic fallback: ${String(err).slice(0, 160)}\n`,
    );
    return analyticKernel();
  }
}

async function main(): Promise<void> {
  const dump = process.argv.includes("--dump");
  // --dump needs no geometry; the analytic kernel builds the schemas without loading WASM.
  const kernel = dump ? analyticKernel() : await selectKernel();
  const app = new CadApp(kernel);
  registerCadActions(app);

  if (dump) {
    stdoutWrite(JSON.stringify(sanitizeSchemas(getToolSchemas() as unknown[]), null, 2) + "\n");
    return;
  }

  process.stderr.write(`[replicad-bolt-on] ready · kernel=${kernel.kind}\n`);

  const rl = createInterface({ input: process.stdin });
  for await (const line of rl) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    let req: { action?: string; params?: Record<string, unknown> };
    try {
      req = JSON.parse(trimmed);
    } catch {
      stdoutWrite(JSON.stringify({ success: false, error: "bad request JSON" }) + "\n");
      continue;
    }
    const result = await dispatchAction(req.action ?? "", req.params ?? {}, { caller: "agent" });
    stdoutWrite(
      JSON.stringify({ success: result.success, data: result.data ?? null, error: result.error ?? null }) + "\n",
    );
  }
}

main().catch((err) => {
  process.stderr.write(`[replicad-bolt-on] fatal: ${err instanceof Error ? err.message : String(err)}\n`);
  process.exit(1);
});
