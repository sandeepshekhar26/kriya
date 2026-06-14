# verb

> The framework where AI agents are **first-class users** of desktop apps.
> Agents operate your app through **typed actions, not pixels** — no screenshots, no vision,
> no brittle clicking. Ship once; it works for humans and machines.

Built on the final stack — **Tauri 2 + Rust + TypeScript + React** — with the safety layer
(permissions, human approval, signed audit, memory) built in, not bolted on.

## The idea: one app, two doors

```
Human  ──clicks buttons──┐
                         ├──▶  the same registered actions  ──▶  app state ──▶ UI
Agent  ──calls actions───┘
```

A developer declares each app affordance once with `registerAction(...)`. Humans trigger it by
clicking; agents trigger it by calling the typed action. Both run the exact same business logic.
The agent never simulates a human — it calls the affordance directly.

## How the loop works

The reference app is a note-taking app. Click **Run agent: organize** and a local agent sorts
the notes into categories by itself. Each step:

1. The app hands the agent its **state** (the notes, as JSON) and a **menu of typed actions**
   (`create_note`, `edit_note`, `delete_note`).
2. The agent picks one action with parameters.
3. The Rust **agent host** gates it: permission check → human approval (if required) → rate
   limit → only then dispatch.
4. The app runs the *same* handler a human button would, and the UI re-renders.
5. The host writes a **cryptographically signed receipt** and records the action to durable
   **memory**.
6. Repeat until the agent reports done.

No vision. No DOM selectors. Just structured state and typed action calls.

## What's built

**Core SDK — `@agent-native/core`** (TypeScript)
- `registerAction()` with typed, permission-scoped actions and runtime parameter validation
- `getToolSchemas()` / `getMcpToolSchemas()` — MCP-compatible + standards-JSON-Schema output
- The agent-loop protocol types; a `agent-native dump` CLI

**Agent host** (Rust, in the Tauri backend)
- The step loop and a swappable `Inference` trait with four backends:
  `deterministic` (scripted, free), `claude-cli`, `ollama`, `anthropic`
- **Permissions** — deny-by-default YAML policy
- **Human-approval queue** — guarded actions pause for an Approve/Deny decision
- **Budget** — sliding-window actions-per-minute cap stops a runaway agent
- **Signed audit trail** — Ed25519 receipt per action → JSONL log
- **Persistent memory** — every action stored in SQLite across runs, recalled into the prompt

**Tooling**
- `tools/verify-receipts` — standalone CLI that verifies the signed audit log offline

## Layout

```
verb/
├── architecture.md            # how the pattern works, end to end
├── docs/PRODUCT_GAPS.md        # honest roadmap: demo → full product
├── packages/
│   └── core/                   # @agent-native/core — the TypeScript SDK
└── apps/
│   └── note-app/               # reference app: Tauri 2 + React + Rust agent host
│       ├── src/                # React UI + action registration (TS)
│       └── src-tauri/          # Rust: host loop, inference, permissions, audit, budget, memory
└── tools/
    └── verify-receipts/        # offline Ed25519 audit-log verifier (Rust)
```

## Quick start

```bash
npm install
npm run build --workspace @agent-native/core
npm run tauri dev --workspace note-app   # first run compiles the Rust backend (a few min)
```

In the app: **Seed 5 notes** → **Run agent: organize** (watch the inspector), then **Run agent:
remove ideas** to see the approval modal pause a guarded delete. Pick the AI backend with the
`AGENT_BACKEND` env var (`deterministic` default, or `claude-cli` / `ollama` / `anthropic`).

See [apps/note-app/README.md](apps/note-app/README.md) for details (and the toolchain note —
Rust is pinned to 1.90 there).

## Status

Early / alpha. The pattern and the safety layer work end-to-end; the framework is still being
generalized and hardened. MIT licensed. Roadmap and what's still missing:
[docs/PRODUCT_GAPS.md](docs/PRODUCT_GAPS.md). Design: [architecture.md](architecture.md).
