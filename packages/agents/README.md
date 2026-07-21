# kriya-agents

Govern an in-process agent framework's tool calls with kriya: **policy → approval → budget → a
signed receipt on every tool call** — without an MCP hop and without inverting the framework's
control flow. Your agent keeps driving its own loop; kriya governs and signs each call.

Works with any tool **function**, plus thin adapters for **LangGraph / LangChain.js** (and, on the
same core, the OpenAI Agents SDK and the Claude Agent SDK — see *Status* below).

## How it works (the one-Signer law)

This package signs **nothing** itself. It spawns the runtime's `kriya-govern` binary and speaks a
tiny two-op stdio protocol to it (`check`, then `record`). Every policy decision and every Ed25519,
hash-chained signature is made by `kriya-govern`, which reuses the *exact* `Policy` / `BudgetTracker`
/ `ApprovalGate` / `Signer` primitives the in-process host and `kriya-mcp` use. No crypto, no policy
engine, no chain writer lives here — it is a thin, honest transport.

Because the tool runs in **your** process, the middleware calls `check` first (policy/approval/
budget), runs the real tool only on `allow`, then calls `record` with the outcome — so a signed
receipt is produced without ever exposing your tools as an MCP server.

## Install

```bash
npm install kriya-agents
```

You also need the `kriya-govern` binary on `PATH` (or pass `binaryPath`). Build it from the runtime:

```bash
cargo build -p kriya --bin kriya-govern    # → target/debug/kriya-govern
```

## Quickstart — any tool function

```ts
import { GovernClient, govern } from "kriya-agents";

const client = new GovernClient({
  policyPath: "agent-policy.yaml",              // omit for the safe built-in default
  actor: "my-agent",
  auditLog: `${process.env.HOME}/.kriya/audit/my-agent.jsonl`, // the Console tails + re-verifies this
});

const search = govern(client, "web_search", async ({ q }: { q: string }) => {
  return await realSearch(q);
});

await search({ q: "kriya" });   // policy/approval/budget gated; signs a receipt if it runs
// A denied/approval-refused/over-budget call throws `GovernDenied` — surface it as the tool's error.
```

## Quickstart — LangGraph / LangChain.js (≤10 lines)

```ts
import { tool } from "@langchain/core/tools";
import { GovernClient } from "kriya-agents";
import { governTool } from "kriya-agents/langgraph";

const client = new GovernClient({ actor: "langgraph", auditLog: "…/.kriya/audit/langgraph.jsonl" });

const search = tool(
  governTool(client, "web_search", async ({ q }: { q: string }) => realSearch(q)),
  { name: "web_search", description: "Search the web", schema: /* your zod schema */ },
);
// Hand `search` to your ToolNode / agent exactly as before — now every call is governed + signed.
```

Already built the tool? Wrap the instance (its `.invoke` is governed; name/schema/description are
preserved so the model sees an identical tool):

```ts
import { governLangGraphTool } from "kriya-agents/langgraph";
const governed = governLangGraphTool(client, existingTool);
```

## The honest ceiling — read this

- **In-process governance is cooperative.** A hostile agent *process* can simply not call this
  middleware. If you need governance a compromised agent cannot skip, launch the agent under
  **containment** (`kriya-gateway run -- <agent>`, B14), which forces its traffic through the
  governed lane at the OS boundary.
- **This lane governs the action tier and signs the receipt** — it does **not** see the tool's own
  outbound network calls. Egress-tier governance (destination allowlists, secret redaction, DLP) is
  the gateway/containment lanes' job, not this one.
- **Approvals are fail-closed.** In a headless run, a `require_approval` tool is **denied** by
  default (no human is attached). Set `approval: "auto"` only for trusted/testing contexts, or
  `"tty"` / `"gui"` (macOS) to prompt a person.

## Status

- ✅ **Core + LangGraph/LangChain.js** — shipped here, tested against the real `kriya-govern` binary,
  and the emitted receipts verify in the kriya Console.
- 🧭 **OpenAI Agents SDK · CrewAI · Claude Agent SDK** and the **Python** sibling package — the same
  `kriya-govern` core; adapters land as each framework's current tool seam is verified. See
  `kriya-console/docs/ideas/BREADTH-EXECUTE-PROMPT.md` (S2).

MIT.
