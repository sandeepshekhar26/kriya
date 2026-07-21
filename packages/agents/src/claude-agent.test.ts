import { describe, expect, it } from "vitest";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { GovernClient } from "./govern-client.js";
import { governClaudeHandler, type ClaudeToolResult } from "./claude-agent.js";

const here = dirname(fileURLToPath(import.meta.url));

function findBin(): string | null {
  const env = process.env.KRIYA_GOVERN_BIN;
  if (env && existsSync(env)) return env;
  return [
    join(here, "../../../apps/note-app/src-tauri/target/debug/kriya-govern"),
    join(here, "../../../crates/kriya/target/debug/kriya-govern"),
    join(here, "../../../target/debug/kriya-govern"),
  ].find((p) => existsSync(p)) ?? null;
}
const BIN = findBin();

function makeClient(policyYaml: string, approval: "deny" | "auto" = "deny") {
  const dir = mkdtempSync(join(tmpdir(), "kriya-claude-"));
  const policyPath = join(dir, "policy.yaml");
  writeFileSync(policyPath, policyYaml);
  const auditLog = join(dir, "audit.jsonl");
  const client = new GovernClient({ binaryPath: BIN!, policyPath, actor: "claude-agent", user: "ci", approval, auditLog });
  return { client, auditLog };
}

// A stub Claude-Agent-SDK-shaped handler: (args, extra) => CallToolResult. No @anthropic-ai dep.
const okHandler = async (args: { q: string }): Promise<ClaudeToolResult> => ({
  content: [{ type: "text", text: `results for ${args.q}` }],
});

describe.skipIf(!BIN)("governClaudeHandler against the real kriya-govern binary", () => {
  it("governs an allowed tool, runs the handler, signs a receipt, returns its CallToolResult", async () => {
    const { client, auditLog } = makeClient(
      'rules:\n  - { action: "search", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
    );
    const handler = governClaudeHandler(client, "search", okHandler);
    const result = await handler({ q: "kriya" }, undefined);
    expect(result.isError).toBeFalsy();
    expect(result.content).toEqual([{ type: "text", text: "results for kriya" }]);
    const receipt = JSON.parse(readFileSync(auditLog, "utf8").trim());
    expect(receipt.action_id).toBe("search");
    expect(receipt.success).toBe(true);
    client.close();
  });

  it("returns an isError CallToolResult on a policy deny (the model adapts) and signs no receipt", async () => {
    const { client, auditLog } = makeClient('rules:\n  - { action: "*", allow: false }\n');
    const handler = governClaudeHandler(client, "search", okHandler);
    const result = await handler({ q: "x" }, undefined);
    expect(result.isError).toBe(true);
    expect(JSON.stringify(result.content)).toContain("denied");
    expect(!existsSync(auditLog) || readFileSync(auditLog, "utf8").trim() === "").toBe(true);
    client.close();
  });

  it("records success:false when the handler itself returns isError", async () => {
    const { client, auditLog } = makeClient(
      'rules:\n  - { action: "search", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
    );
    const failing = governClaudeHandler(client, "search", async (): Promise<ClaudeToolResult> => ({
      content: [{ type: "text", text: "upstream error" }],
      isError: true,
    }));
    const result = await failing({ q: "x" }, undefined);
    expect(result.isError).toBe(true); // the handler's own error result is preserved
    const receipt = JSON.parse(readFileSync(auditLog, "utf8").trim());
    expect(receipt.success).toBe(false); // recorded as a failed call
    client.close();
  });
});
