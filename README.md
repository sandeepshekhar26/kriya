# kriya

> **Build desktop apps that AI agents can understand and operate** — directly, through your app's
> real typed actions, not by screenshotting the screen and guessing where to click.

Every app today was built for humans clicking buttons. Now AI agents need to use those same apps —
and they shouldn't have to squint at pixels to do it. With kriya you declare each of your app's
capabilities once as a typed **action**: a human triggers it by clicking, an agent triggers it by
calling it — the *same* code underneath. The agent's entire view of your app is structured state
and a typed menu of what it can do. No screenshots, no DOM scraping, no guessing.

*Think: React/Electron, but for the age of AI agents.* Build a new app this way, or **bolt it onto
an app you already have** — and expose those actions to any agent (Claude Desktop, Cursor, …) over
MCP.

And because an agent operating a real app needs guardrails, kriya bakes them in: every action runs
through **permission → human approval → budget → a signed audit trail**, on-device, before it
touches your data.

<p align="center">
  <img src="examples/actual-budget-bolt-on/demo.gif" alt="an AI agent operating Actual Budget through kriya — routine actions run and are signed, money-moving ones are blocked pending approval, every receipt verifiable" width="760">
  <br><em>An agent operating <a href="https://actualbudget.org">Actual Budget</a> through kriya: routine actions run and are signed; money-moving ones are blocked pending approval; every action verifies offline.</em>
</p>

## One app, two doors

```
Human  ──clicks buttons──┐
                         ├──▶  the same registered actions  ──▶  app state ──▶ UI
Agent  ──calls actions───┘        (governed: permission · approval · budget · audit)
```

A developer declares each affordance once — with `registerAction(...)` for a new app, or
`wrapAction(...)` to adopt one you already have. The agent never simulates a human: it calls the
typed action directly, and it can never bypass the gates, because the host (not the agent) owns the
policy and the signing key.

## What you get

- **Typed actions, not pixels.** Declare a capability once; humans click it, agents call it, both
  run the same handler. The agent reasons over structured state and a typed tool schema — fast and
  reliable, and it doesn't break when you restyle a button.
- **Governance, built in.** Every action an agent proposes runs this gauntlet on-device, before it
  executes:
  1. **Permission** — a deny-by-default policy: allow / require-approval / deny.
  2. **Human approval** — guarded actions pause for an Approve/Deny decision in *your* app's UI.
  3. **Budget** — a per-minute cap stops a runaway or looping agent.
  4. **Signed audit** — an Ed25519 receipt per action → append-only log, verifiable offline.

  Plus persistent **memory** across runs, policy **linting**, and **step-through** debugging.
- **Speaks MCP.** Your actions become MCP tools; the governed `kriya-mcp` server lets any external
  agent drive your app — with every call routed *through* the gates, not around them.
- **Cross-shell.** Runs in a Tauri backend, or as a standalone `kriya-host` sidecar that Electron
  and plain Node apps drive over stdio — governance in a process the renderer can't tamper with.

## Two ways to adopt

**Build a new local-first agent app:**
```bash
npm create kriya-app@latest my-app    # Tauri 2 + React + Rust host, safety layer pre-wired
```

**Bolt onto an app you already have** — wrap a function it *already exposes*, no rewrite:
```ts
import { wrapAction } from "kriya-core";

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

That snippet is the demo above: [`examples/actual-budget-bolt-on/`](examples/actual-budget-bolt-on/)
gives a frontier agent governed access to [Actual Budget](https://actualbudget.org) — a real,
local-first finance app with **no HTTP API** — in ~37 lines, without changing Actual's code.
(`kriya wrap <file>` scaffolds the wrappers from your exported functions.)

## What's in the box

| Package / crate | What |
|---|---|
| [`kriya-core`](packages/core/) | TypeScript SDK — `registerAction`, `wrapAction`, validation, MCP/JSON-Schema export, the `kriya` CLI (`dump`, `wrap`) |
| [`kriya-sidecar`](packages/sidecar/) | Node/TS binding — host the runtime from Electron or plain Node over stdio |
| [`kriya-inspector`](packages/inspector/) | Drop-in React dev inspector — step log, approval modal, memory replay |
| [`create-kriya-app`](packages/create-kriya-app/) | Scaffolder for a new local-first agent app |
| [`kriya`](crates/kriya/) | Rust agent host — step loop, swappable inference, permissions, budget, signed audit, memory, **governed MCP-server mode** |

**Binaries:** `kriya-mcp` (governed MCP server — external agents drive your app through the gates) ·
`kriya-host` (the stdio sidecar) · [`tools/verify-receipts`](tools/verify-receipts/) (offline audit-log verifier).

**Reference apps:** [`apps/note-app`](apps/note-app/) and [`apps/task-manager`](apps/task-manager/)
— two domains on the one shared host crate.

## Quick start

Try the governed bolt-on with zero setup (in-memory budget, no real data):

```bash
cd examples/actual-budget-bolt-on && ./demo.sh   # builds everything on first run, then plays it
```

Or run the reference desktop app:

```bash
npm install
npm run build --workspace kriya-core
npm run tauri dev --workspace note-app   # first run compiles the Rust backend (a few min)
```

Pick the inference backend with `AGENT_BACKEND` (`deterministic` default, or `claude-cli` /
`ollama` / `anthropic`).

## Docs

- [architecture.md](architecture.md) — how the pattern works, end to end
- [docs/ROADMAP.md](docs/ROADMAP.md) — what's built and what's next
- [docs/PRODUCT_GAPS.md](docs/PRODUCT_GAPS.md) — honest feature-completion tracker

## Status

Alpha. The pattern, the cross-shell runtime, and the full safety layer work end-to-end — typed
actions, governed MCP-server mode, the Electron/Node sidecar, the `wrapAction` bolt-on, and the
Actual Budget flagship are all shipped. APIs may still change before a stable release. MIT licensed.
