import { describe, expect, it } from "vitest";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { GovernClient } from "./govern-client.js";
import { governExecute } from "./openai-agents.js";

const here = dirname(fileURLToPath(import.meta.url));

function findBin(): string | null {
  const env = process.env.KRIYA_GOVERN_BIN;
  if (env && existsSync(env)) return env;
  return (
    [
      join(here, "../../../apps/note-app/src-tauri/target/debug/kriya-govern"),
      join(here, "../../../crates/kriya/target/debug/kriya-govern"),
      join(here, "../../../target/debug/kriya-govern"),
    ].find((p) => existsSync(p)) ?? null
  );
}
const BIN = findBin();

// The live acceptance runs against the REAL @openai/agents SDK — governing an actual FunctionTool's
// execute and driving it through the SDK's own `tool.invoke`. If the SDK isn't installed (a bare
// checkout), the suite skips with a clear reason rather than passing vacuously. `govern()` already
// covers this framework generically; this adapter + test prove it end to end through the SDK.
let oai: any = null;
let zmod: any = null;
try {
  oai = await import("@openai/agents");
  zmod = await import("zod");
} catch {
  /* framework not installed — the describe below skips */
}

function makeClient(policyYaml: string) {
  const dir = mkdtempSync(join(tmpdir(), "kriya-oai-"));
  const policyPath = join(dir, "policy.yaml");
  writeFileSync(policyPath, policyYaml);
  const log = join(dir, "audit.jsonl");
  const client = new GovernClient({
    binaryPath: BIN!,
    policyPath,
    actor: "openai-agents",
    user: "ci",
    auditLog: log,
  });
  return { client, log };
}

describe.skipIf(!BIN || !oai || !zmod)(
  "OpenAI Agents SDK adapter — governed tool through the real @openai/agents",
  () => {
    it("governs an allowed tool's execute via tool.invoke and signs a receipt", async () => {
      const { tool } = oai;
      const { z } = zmod;
      const { client, log } = makeClient(
        'rules:\n  - { action: "add", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
      );
      const add = tool({
        name: "add",
        description: "Add two numbers",
        parameters: z.object({ a: z.number(), b: z.number() }),
        execute: governExecute(client, "add", async ({ a, b }: { a: number; b: number }) => `${a + b}`),
      });
      // Drive it the way the SDK's runner does: invoke(runContext, argsJSON).
      const out = await add.invoke({ context: {} }, JSON.stringify({ a: 2, b: 3 }));
      expect(out).toBe("5");
      const receipt = JSON.parse(readFileSync(log, "utf8").trim());
      expect(receipt.action_id).toBe("add");
      expect(receipt.success).toBe(true);
      expect(receipt.actor.agent).toBe("openai-agents");
      client.close();
    });

    it("denies a blocked tool — the SDK surfaces it to the model, and no receipt is signed", async () => {
      const { tool } = oai;
      const { z } = zmod;
      const { client, log } = makeClient('rules:\n  - { action: "*", allow: false }\n');
      const wipe = tool({
        name: "wipe_db",
        description: "danger",
        parameters: z.object({}),
        execute: governExecute(client, "wipe_db", async () => "wiped"),
      });
      // The SDK catches the thrown GovernDenied and returns it as the tool's error output.
      const out = await wipe.invoke({ context: {} }, JSON.stringify({}));
      expect(String(out).toLowerCase()).toContain("denied");
      expect(!existsSync(log) || readFileSync(log, "utf8").trim() === "").toBe(true);
      client.close();
    });
  },
);
