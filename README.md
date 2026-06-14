# agent-native

> The framework where AI agents are **native citizens** of desktop apps.
> Agents see your app's *actions*, not pixels. Ship once — works for humans and machines.

This repository is the **Phase 0 proof** built directly on the **final tech stack** (Tauri 2 +
Rust core + TypeScript SDK + React). It is intentionally small in feature scope, but every layer
is the real one — no throwaway prototype code. Phase 1 generalizes these same layers into the
publishable `@agent-native/*` packages.

## What this proves

A local agent can drive a real desktop note app **without vision, screenshots, or DOM selectors** —
only typed action calls and structured state.

```
Human  ──clicks buttons──┐
                         ├──▶  same business logic (registered actions)
Agent  ──calls tools─────┘
```

The agent:
1. Reads the app's **state** (the list of notes) as structured JSON.
2. Picks a **typed action** (`create_note`, `edit_note`, `delete_note`) from a registry.
3. The Rust **agent host** permission-checks and signs the call, then asks the app to run it.
4. The app executes its own handler, mutates state, and the UI re-renders.
5. The loop repeats until the agent reports the task done.

## Layout

```
experiment1/
├── architecture.md          # how the pattern works, end to end
├── packages/
│   └── core/                # @agent-native/core — TypeScript SDK (action registry + protocol)
└── apps/
    └── note-app/            # reference app: Tauri 2 + React frontend, Rust agent host
        ├── src/             # React UI + action registration (TS)
        └── src-tauri/       # Rust: agent host, inference backends, permissions, audit
```

## Inference backends (Phase 0)

Pluggable via the `Inference` trait in Rust. Two ship now:

- **`deterministic`** — a scripted planner that exercises the *full* protocol with no LLM. Proves
  the architecture is sound today, costs nothing.
- **`claude-cli`** — shells out to the locally-installed `claude` CLI in print mode. A real LLM
  driving the app, no API key/billing required.

`anthropic`/`ollama` API backends slot in behind the same trait later.

## Status

Phase 0 — in progress. See [architecture.md](architecture.md) for the design and
[apps/note-app/README.md](apps/note-app/README.md) for run instructions.
