# Architecture

This document explains the pattern the reference apps prove: **a local agent driving a
real desktop app through typed actions and structured state — no vision, no screenshots, no
DOM selectors.** It is built on the final stack (Tauri 2 + Rust + TypeScript + React), kept
deliberately small so the pattern is legible.

> **Note (updated 2026-06-15):** the *pattern* below is current and accurate, but two things have
> moved on since this was first written. (1) The Rust agent host is now a **shared crate**
> (`crates/kriya`) consumed by both reference apps, not code inside one app — decision
> [D-002](docs/DECISIONS.md). (2) Most items in the old *"deliberately leaves out"* list have since
> shipped. For current feature state see [docs/PRODUCT_GAPS.md](docs/PRODUCT_GAPS.md); for what's
> next and the strategic direction see [docs/ROADMAP.md](docs/ROADMAP.md) and [CLAUDE.md](CLAUDE.md).

## The core idea

A traditional app has one entry point: a human clicking UI. An kriya app has **two
entry points into the same business logic**.

```
   Human  ──click "Add"──────────┐
                                 ├──▶  registered action handler  ──▶  app state  ──▶  UI
   Agent  ──call edit_note(...)──┘
```

Both a button click and an agent tool call land on the *same* registered action. The agent
never simulates a human — it calls the affordance directly.

## The three layers

```
┌──────────────────────────────────────────────────────────────────────┐
│  FRONTEND  (React + TypeScript, in the Tauri webview)                 │
│                                                                      │
│   @kriya/core                                                 │
│     registerAction({ id, description, parameters, permissions,       │
│                       handler })          ← declare an affordance     │
│     getToolSchemas()  → MCP-style tool schemas (no handlers)         │
│     dispatchAction(id, params, ctx) → runs the handler, mutates store │
│                                                                      │
│   store.ts   notes = single source of truth (observable)             │
│   actions.ts create_note / edit_note / delete_note                    │
│   agent.ts   bridges host events ↔ the registry                      │
└──────────────────────────────────────────────────────────────────────┘
        │  invoke("agent_start", { goal, state, tools })   (app → host)
        │  invoke("agent_action_result", { result })       (app → host)
        ▲  event "agent://action"  { actionId, params, reasoning }  (host → app)
        ▲  event "agent://done" / "agent://log"                     (host → app)
        │
┌──────────────────────────────────────────────────────────────────────┐
│  AGENT HOST  (Rust, in the Tauri backend)                            │
│                                                                      │
│   host.rs         the step loop (init → step → result → step → done)  │
│   inference/      swappable backends behind one trait:               │
│       deterministic   scripted planner, zero cost (default)          │
│       claude_cli      shells out to the local `claude` CLI            │
│   permissions.rs  policy check before every action (deny by default) │
│   audit.rs        Ed25519-signed receipt per action → JSONL log      │
└──────────────────────────────────────────────────────────────────────┘
```

## One step of the loop, end to end

The agent's task: *"organize every note by assigning each a sensible category."*

1. **Start.** The frontend calls `invoke("agent_start", { goal, state, tools })`. `state` is
   `{ notes: [...] }`; `tools` is the output of `getToolSchemas()` — the JSON contract for
   `create_note`, `edit_note`, `delete_note`. The host spawns the loop on its own thread.

2. **Decide.** The inference backend reads the goal, current state, tool schemas, and history,
   and returns one decision: `Call { action_id, params, reasoning }` or `Done { summary }`.
   The deterministic backend finds the first uncategorized note, keyword-classifies it, and
   returns `edit_note(id, category)`. An LLM backend returns the same shape.

3. **Gate.** `permissions.rs` checks the action against the policy. `edit_*` is allowed;
   `delete_*` would require human approval; anything unmatched is denied. The **host** owns this
   decision — the agent cannot bypass it.

4. **Dispatch.** The host registers a one-shot channel keyed by a fresh `stepId`, emits
   `agent://action`, and blocks waiting for the result.

5. **Execute.** The frontend's `agent.ts` receives the event and calls
   `dispatchAction(actionId, params, { caller: "agent", stepId })` — the **same** registry path
   a human button uses. The handler mutates the store; React re-renders the note with its new
   category badge.

6. **Report.** The frontend sends `invoke("agent_action_result", { stepId, success, state })`
   carrying the refreshed state. The host's channel unblocks.

7. **Sign.** `audit.rs` signs a receipt `{ stepId, actionId, params, success, ts }` with an
   Ed25519 key the agent never holds, appends it to `kriya-audit.jsonl`, and emits a log
   line for the inspector.

8. **Loop.** The host feeds the new state back to the backend and repeats from step 2 until the
   backend returns `Done`, which fires `agent://done`.

No pixels are read. No coordinates are clicked. The agent's entire view of the app is structured
JSON, and its entire vocabulary is the typed tool schema.

## Why each piece is the way it is

- **State lives in the frontend.** The app owns its data as usual; the host stays stateless
  about app contents and just relays decisions. Each result carries a fresh snapshot, so the
  backend always reasons over current truth.
- **Inference is a trait, not a hardcoded model.** `deterministic` proves the wiring with zero
  cost and full determinism; `claude_cli` proves a real LLM drops into the identical loop. API
  backends (Anthropic, Ollama) implement the same `Inference` trait later — the loop never changes.
- **The host, not the agent, enforces policy and signs receipts.** Permission checks and the
  signing key sit on the Rust side. An agent can *propose* `delete_note`; it cannot *approve* it
  or forge a receipt.
- **The protocol mirrors JSON-RPC request/response**, keyed by `stepId`, so it ports cleanly off
  Tauri IPC to WebSocket/HTTP later (dev inspector, hosted cloud) without reshaping messages.

## Beyond Phase 0 (mostly shipped)

The seams Phase 0 stubbed have since landed: the **human-approval queue** (host blocks on a
per-step channel, frontend modal), **persistent agent memory** (SQLite, across runs), the
**offline receipt verifier** (`tools/verify-receipts`), the **`create-kriya-app` scaffolder**,
plus **action composition**, **resume-ability**, **step-through**, and **policy linting**. Two
reference apps (notes + tasks) now share the extracted `kriya` crate, each plugging
in its own scripted planner. Still open: hot-reload of the registry, and the strategic next
builds — governed MCP-server mode, sidecar/Electron host, `wrapAction` bolt-on (the critical
path to the YC demo). See [docs/ROADMAP.md](docs/ROADMAP.md) and
[docs/PRODUCT_GAPS.md](docs/PRODUCT_GAPS.md).

## File map

| Concern | File |
|---|---|
| Action registry + validation | `packages/core/src/registry.ts` |
| Action / schema / result types | `packages/core/src/types.ts` |
| Agent loop protocol (TS) | `packages/core/src/protocol.ts` |
| Note affordances | `apps/note-app/src/actions.ts` |
| Frontend ↔ host bridge | `apps/note-app/src/agent.ts` |
| Step loop | `crates/kriya/src/agent/host.rs` |
| `Inference` trait + LLM backends (claude-cli / ollama / anthropic) | `crates/kriya/src/agent/inference/` |
| Permission policy + linting | `crates/kriya/src/permissions.rs` |
| Signed audit trail | `crates/kriya/src/audit.rs` |
| Budget + persistent memory | `crates/kriya/src/{budget,memory}.rs` |
| Protocol (Rust) | `crates/kriya/src/protocol.rs` |
| App-specific deterministic planner + Tauri glue | `apps/note-app/src-tauri/src/{deterministic,lib}.rs` (task-manager mirrors this) |
