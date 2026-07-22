import { afterEach, describe, expect, it } from "vitest";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { GovernClient, GovernDenied, govern } from "./govern-client.js";
import { governLangGraphTool } from "./langgraph.js";

const here = dirname(fileURLToPath(import.meta.url));

/** Locate the built `kriya-govern` binary — set KRIYA_GOVERN_BIN to override. Built with:
 *  `( cd apps/note-app/src-tauri && cargo build -p kriya --locked --bin kriya-govern )`. */
function findBin(): string | null {
  const env = process.env.KRIYA_GOVERN_BIN;
  if (env && existsSync(env)) return env;
  const candidates = [
    join(here, "../../../apps/note-app/src-tauri/target/debug/kriya-govern"),
    join(here, "../../../crates/kriya/target/debug/kriya-govern"),
    join(here, "../../../target/debug/kriya-govern"),
  ];
  return candidates.find((p) => existsSync(p)) ?? null;
}

const BIN = findBin();

function tempPolicy(yaml: string): string {
  const dir = mkdtempSync(join(tmpdir(), "kriya-agents-"));
  const p = join(dir, "policy.yaml");
  writeFileSync(p, yaml);
  return p;
}

// The acceptance runs against the REAL kriya-govern binary — the whole point is that this package
// signs NOTHING itself. Without the built binary (a bare `npm test` on a fresh checkout) the suite
// skips with a clear reason rather than passing vacuously.
describe.skipIf(!BIN)("GovernClient against the real kriya-govern binary", () => {
  let clients: GovernClient[] = [];
  const make = (policyYaml: string, extra: Record<string, string> = {}) => {
    const dir = mkdtempSync(join(tmpdir(), "kriya-agents-log-"));
    const auditLog = join(dir, "audit.jsonl");
    const c = new GovernClient({
      binaryPath: BIN!,
      policyPath: tempPolicy(policyYaml),
      actor: "langgraph",
      user: "ci",
      approval: (extra.approval as "deny" | "auto") ?? "deny",
      auditLog,
    });
    clients.push(c);
    return { client: c, auditLog };
  };
  afterEach(() => {
    for (const c of clients) c.close();
    clients = [];
  });

  it("allows a permitted tool, runs it, and signs a verifiable receipt", async () => {
    const { client, auditLog } = make(
      'rules:\n  - { action: "web_search", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
    );
    let ran = 0;
    const search = govern(client, "web_search", async (p: { q: string }) => {
      ran++;
      return `results for ${p.q}`;
    });

    const out = await search({ q: "kriya" });
    expect(out).toBe("results for kriya");
    expect(ran).toBe(1);

    const line = readFileSync(auditLog, "utf8").trim().split("\n")[0]!;
    const receipt = JSON.parse(line);
    expect(receipt.action_id).toBe("web_search");
    expect(receipt.success).toBe(true);
    expect(receipt.actor).toEqual({ agent: "langgraph", user: "ci" });
    // A real Ed25519 signature + public key — produced by the runtime Signer, not this package.
    expect(receipt.signature).toMatch(/^[0-9a-f]{128}$/);
    expect(receipt.public_key).toMatch(/^[0-9a-f]{64}$/);
  });

  it("denies a policy-blocked tool as GovernDenied and never runs it or signs a receipt", async () => {
    const { client, auditLog } = make('rules:\n  - { action: "*", allow: false }\n');
    let ran = 0;
    const del = govern(client, "delete_account", async () => {
      ran++;
      return "gone";
    });

    await expect(del({})).rejects.toBeInstanceOf(GovernDenied);
    expect(ran).toBe(0);
    // An action-tier deny signs no receipt — the decision is the record.
    expect(!existsSync(auditLog) || readFileSync(auditLog, "utf8").trim() === "").toBe(true);
  });

  it("records a failure receipt when the tool throws, then re-throws", async () => {
    const { client, auditLog } = make(
      'rules:\n  - { action: "flaky", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
    );
    const flaky = govern(client, "flaky", async () => {
      throw new Error("upstream 500");
    });
    await expect(flaky({})).rejects.toThrow("upstream 500");
    const receipt = JSON.parse(readFileSync(auditLog, "utf8").trim());
    expect(receipt.action_id).toBe("flaky");
    expect(receipt.success).toBe(false); // the attempt is recorded even though it failed
  });

  it("enforces the budget cap across the govern session", async () => {
    const { client } = make(
      'rules:\n  - { action: "tick", allow: true }\nbudget:\n  max_actions_per_minute: 2\n',
    );
    const tick = govern(client, "tick", async () => "ok");
    expect(await tick({})).toBe("ok");
    expect(await tick({})).toBe("ok");
    await expect(tick({})).rejects.toMatchObject({ decision: "budget_exceeded" });
  });

  it("require_approval denies headlessly (fail-closed), auto-approves when told to", async () => {
    const deny = make('rules:\n  - { action: "send_email", allow: true, require_approval: true }\n');
    const send = govern(deny.client, "send_email", async () => "sent");
    await expect(send({})).rejects.toMatchObject({ decision: "not_approved" });

    const auto = make(
      'rules:\n  - { action: "send_email", allow: true, require_approval: true }\nbudget:\n  max_actions_per_minute: 60\n',
      { approval: "auto" },
    );
    const send2 = govern(auto.client, "send_email", async () => "sent");
    expect(await send2({})).toBe("sent");
  });

  it("the LangGraph adapter governs a tool instance's invoke, preserving its metadata", async () => {
    const { client, auditLog } = make(
      'rules:\n  - { action: "adder", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
    );
    // A minimal LangChain-tool-like object (name + invoke) — no @langchain dependency needed.
    const rawTool = {
      name: "adder",
      description: "add two numbers",
      invoke: (input: { a: number; b: number }) => input.a + input.b,
    };
    const governed = governLangGraphTool(client, rawTool);
    expect(governed.name).toBe("adder"); // metadata preserved for the model
    expect(governed.description).toBe("add two numbers");
    expect(await governed.invoke({ a: 2, b: 3 })).toBe(5);
    const receipt = JSON.parse(readFileSync(auditLog, "utf8").trim());
    expect(receipt.action_id).toBe("adder");
    expect(receipt.success).toBe(true);
  });

  it("stamps run correlation (run_id per client, parent_step_id for a nested call)", async () => {
    const { client, auditLog } = make(
      'rules:\n  - { action: "*", allow: true }\nbudget:\n  max_actions_per_minute: 60\n',
      { approval: "auto" },
    );
    // A top-level call: the client's run_id groups the invocation; no parent.
    const parent = govern(client, "outer", async () => "ok");
    await parent({ q: "x" });
    // A nested call under the first one's step_id (threaded via the hooks bag).
    const lines1 = readFileSync(auditLog, "utf8").trim().split("\n");
    const outerReceipt = JSON.parse(lines1[0]!);
    const nested = govern(client, "inner", async () => "ok", {
      parentStepId: outerReceipt.step_id,
    });
    await nested({});

    const lines = readFileSync(auditLog, "utf8").trim().split("\n");
    const outer = JSON.parse(lines[0]!);
    const inner = JSON.parse(lines[1]!);

    // Both share the client's run_id — the whole invocation is one run.
    expect(outer.params["kriya.corr"].run_id).toBe(client.runId);
    expect(inner.params["kriya.corr"].run_id).toBe(client.runId);
    // The top-level call carries no parent; the nested call points at the outer step.
    expect(outer.params["kriya.corr"].parent_step_id).toBeUndefined();
    expect(inner.params["kriya.corr"].parent_step_id).toBe(outer.step_id);
    // The tool's own params are preserved alongside the reserved key.
    expect(outer.params.q).toBe("x");
    // Still a real signature (correlation rides the one-Signer path, changes no bytes' validity).
    expect(outer.signature).toMatch(/^[0-9a-f]{128}$/);
  });

  it("honors a caller-supplied runId and gives distinct clients distinct runs", async () => {
    const a = new GovernClient({ binaryPath: BIN!, runId: "run-external-42" });
    const b = new GovernClient({ binaryPath: BIN! });
    clients.push(a, b);
    expect(a.runId).toBe("run-external-42");
    expect(b.runId).not.toBe(a.runId); // a fresh UUID per client by default
  });
});
