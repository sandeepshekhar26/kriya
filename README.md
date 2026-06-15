# kriya

> **The governed runtime that lets an AI agent safely drive a desktop app — over MCP, on-device.**
> Agents operate your app through **typed actions, not pixels**, and every call passes through
> permission, human approval, budget, and a signed audit trail before it touches your data.

As every app gets an agent, someone has to stop the agent doing the wrong thing — and prove it
to an auditor. **kriya is that layer, for the apps the cloud can't reach**: local-first, private,
regulated desktop software with no web API to wrap. MCP moves the calls; kriya is the safety the
wire leaves out.

It's not a competitor to MCP — it's the **governed runtime you put behind it**. Expose your app's
real actions to any agent (Claude Desktop, Cursor, …), and kriya enforces policy → approval →
budget → signed audit on-device, where the data and the human are.

## The 50-line proof

[`examples/actual-budget-bolt-on/`](examples/actual-budget-bolt-on/) bolts governed agent access
onto [**Actual Budget**](https://actualbudget.org) — a real, shipped, local-first finance app with
**no HTTP API** — *without changing Actual's code*. The whole integration is ~37 lines:

```ts
import { wrapAction } from "@kriya/core";

// Wrap a function the app already has. The agent can now call it — but the host decides
// whether it's allowed, whether a human must approve it, and signs a receipt when it runs.
wrapAction(actual.updateTransaction, {
  id: "categorize_transaction",
  description: "Assign a category to a transaction.",
  parameters: { id: str, category: str },
  mapParams: (p) => [p.id, { category: p.category }],
});

wrapAction(actual.deleteTransaction, {
  id: "delete_transaction",
  description: "Permanently delete a transaction.",
  parameters: { id: str },
  mapParams: (p) => [p.id],          // policy: require_approval — pauses for a human
});
```

The agent categorizes and reconciles freely; **deleting a transaction or moving money pauses for
your approval**, and every action is signed into an audit log. Run the full governed flow with no
setup via `ACTUAL_FAKE=1` — see the [example README](examples/actual-budget-bolt-on/).

## One app, two doors

```
Human  ──clicks buttons──┐
                         ├──▶  the same registered actions  ──▶  app state ──▶ UI
Agent  ──calls actions───┘        (governed: policy · approval · budget · audit)
```

A developer declares each affordance once — with `registerAction(...)` for a new app, or
`wrapAction(...)` to adopt one you already have. Humans trigger it by clicking; agents trigger it
by calling the typed action. Both run the *exact same* business logic. The agent never simulates a
human — and it can never bypass the gates, because the host (not the agent) owns the policy and the
signing key.

## The governance (the moat)

Every action an agent proposes runs this gauntlet, on-device, before it executes:

1. **Permission** — a deny-by-default YAML policy decides allow / require-approval / deny.
2. **Human approval** — guarded actions pause for an Approve/Deny decision in *your* app's UI (or a
   terminal prompt), then resume the in-flight call.
3. **Budget** — a sliding-window actions-per-minute cap stops a runaway or looping agent.
4. **Signed audit** — an Ed25519 receipt per action → append-only JSONL, verifiable offline.

Plus persistent **memory** (every action across runs, in SQLite), policy **linting**, and
**step-through** debugging. For local/regulated apps this is mandatory — and it can only happen
in-app, on-device, which cloud MCP gateways structurally can't reach.

## Two ways to adopt

- **Bolt onto an app you already have** — `wrapAction(existingFn, …)` (+ an `kriya wrap`
  codemod that scaffolds wrappers from your exported functions), then expose them over MCP with
  the `kriya-mcp` server. Augment, not rewrite. This is the [Actual Budget demo](examples/actual-budget-bolt-on/).
- **Build a new local-first agent app** — `npm create kriya-app@latest` scaffolds a Tauri 2 +
  React + Rust app with the whole safety layer pre-wired.

The runtime is **cross-shell**: it runs in a Tauri backend, or as a standalone sidecar process
(`kriya-host`) that **Electron and plain Node** apps drive over stdio via
[`@kriya/sidecar`](packages/sidecar/) — so governance lives in a process the renderer can't
tamper with.

## What's in the box

| Package / crate | What |
|---|---|
| [`@kriya/core`](packages/core/) | TypeScript SDK — `registerAction`, `wrapAction`, validation, MCP/JSON-Schema export, the `kriya` CLI (`dump`, `wrap`) |
| [`@kriya/sidecar`](packages/sidecar/) | Node/TS binding — host the runtime from Electron or plain Node over stdio |
| [`@kriya/inspector`](packages/inspector/) | Drop-in React dev inspector — step log, approval modal, memory replay |
| [`create-kriya-app`](packages/create-kriya-app/) | Scaffolder for a new local-first agent app |
| [`kriya`](crates/kriya/) | Rust agent host — step loop, swappable inference, permissions, budget, signed audit, memory, **governed MCP-server mode** |

**Binaries:** `kriya-mcp` (governed MCP server — external agents drive your app through the gates) ·
`kriya-host` (the stdio sidecar) · [`tools/verify-receipts`](tools/verify-receipts/) (offline audit-log verifier).

**Reference apps:** [`apps/note-app`](apps/note-app/) and [`apps/task-manager`](apps/task-manager/)
— two domains on the one shared host crate.

## Quick start

Try the governed bolt-on with zero setup (in-memory budget, no real data):

```bash
npm install
npm run build --workspace @kriya/core
cargo build -p kriya --bin kriya-mcp --release
cd examples/actual-budget-bolt-on && npm install && npm run build
# then drive it like an MCP client — see the example README for the full command + Claude Desktop config
```

Or run the reference desktop app:

```bash
npm run build --workspace @kriya/core
npm run tauri dev --workspace note-app   # first run compiles the Rust backend (a few min)
```

Pick the inference backend with `AGENT_BACKEND` (`deterministic` default, or `claude-cli` /
`ollama` / `anthropic`).

## Docs

- [architecture.md](architecture.md) — how the pattern works, end to end
- [docs/ROADMAP.md](docs/ROADMAP.md) — what's built and what's next
- [docs/PRODUCT_GAPS.md](docs/PRODUCT_GAPS.md) — honest feature-completion tracker

## Status

Alpha. The pattern, the cross-shell runtime, and the full safety layer work end-to-end (the wedge
— governed MCP server, sidecar host, `wrapAction` bolt-on, and the Actual Budget flagship — are
all shipped). APIs may still change before the first published release. MIT licensed.
