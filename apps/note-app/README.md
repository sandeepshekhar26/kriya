# note-app — kriya reference app

A Tauri 2 note app where a **local agent** organizes notes by calling typed actions —
no vision, no screenshots, no DOM automation. The Phase 0 proof for the framework.

## Prerequisites

- **Node ≥ 20** and npm
- **Rust** via [rustup](https://rustup.rs). The crate pins its toolchain in
  [`src-tauri/rust-toolchain.toml`](src-tauri/rust-toolchain.toml) (1.90.0) — rustup installs it
  automatically on first build. (Rust 1.96 miscompiles a transitive Tauri dependency; see that
  file for details.)
- macOS system WebView (built in). Linux/Windows: the usual
  [Tauri prerequisites](https://tauri.app/start/prerequisites/).
- Optional: the **`claude` CLI**, logged in, to run the LLM backend.

## Run

From the repo root (installs the workspace, builds the core SDK, launches the app):

```bash
npm install
npm run build --workspace kriya-core
npm run tauri dev --workspace note-app
```

The first launch compiles the Rust backend (a few minutes); later launches are fast.

## Try it

1. Click **Seed 5 notes** — five intentionally uncategorized notes appear.
2. Click **Run agent: organize**.
3. Watch the **agent inspector** (right panel): each step shows the action the agent chose,
   its parameters and reasoning, and a signed audit receipt. Category badges fill in live on
   each note as the agent calls `edit_note`.

You can also use the app as a human: add a note with the form, and it flows through the exact
same `create_note` action the agent would call.

## Choosing the inference backend

Set `AGENT_BACKEND` before launching:

| Value | Backend | Notes |
|---|---|---|
| _(unset)_ / `deterministic` | Scripted keyword organizer | Default. Zero cost, fully deterministic. |
| `claude-cli` | Local `claude` CLI in print mode | A real LLM picks each action. Requires the CLI logged in. Override the binary with `CLAUDE_BIN`. |

```bash
AGENT_BACKEND=claude-cli npm run tauri dev --workspace note-app
```

## Audit trail

Every executed action is signed with an Ed25519 key the agent never holds, and appended to
`kriya-audit.jsonl` in your temp dir. The inspector shows a short signature per step;
the file holds the full signed receipts (verifiable offline with the public key).

## Where things live

- `src/actions.ts` — the registered note affordances (humans and agent share these)
- `src/agent.ts` — frontend ↔ host bridge
- `src-tauri/src/agent/` — the Rust agent host: step loop + inference backends
- `src-tauri/src/permissions.rs`, `audit.rs` — policy checks and signed receipts
- `src-tauri/agent-policy.yaml` — the editable per-app policy

See [`../../architecture.md`](../../architecture.md) for the full design.
