# kriya

The Rust agent host behind [**kriya**](https://github.com/sandeepshekhar26/kriya) (MCP for Desktop) — the **governed
runtime** that lets an AI agent safely drive a desktop app. It turns a goal + a registry of typed
actions into a permission-checked, budget-bounded, human-approvable, **cryptographically audited**
sequence of action calls — and exposes that same governance to external agents over MCP.

Pairs with the [`kriya-core`](https://www.npmjs.com/package/kriya-core)
TypeScript SDK, but the governance, signing key, and policy live here in Rust, in a process the
app's UI can't tamper with.

## Two ways to run it

**1. Governed MCP-server mode** — let an external agent (Claude Desktop, Cursor, …) drive your app,
with every `tools/call` routed through the gates. This is what makes kriya a bolt-on, not a rewrite:

```rust
use std::sync::Arc;
use kriya::{audit::Signer, permissions::Policy};
use kriya::mcp::{Governor, Server, ProcessExecutor, DenyApproval};

let governor = Governor::new(
    Arc::new(Policy::load_or_default("agent-policy.yaml".as_ref())),
    Arc::new(Signer::new()),
    Box::new(DenyApproval),                       // human-approval gate
    Box::new(ProcessExecutor::new("node handler.js")), // runs cleared actions against your app
);
let mut server = Server::new("my-app", "0.1.0", tool_schemas, governor);
server.serve(std::io::stdin().lock(), &mut std::io::stdout().lock())?;
```

Or just use the bundled **`kriya-mcp`** binary:
`kriya-mcp --tools tools.json --policy agent-policy.yaml --exec "node handler.js" --persistent`.

**2. In-process agent loop** — host an autonomous agent inside your app and drive it through a
[`HostSink`](src/agent/transport.rs) (Tauri, or the `kriya-host` stdio sidecar for Electron/Node):

```rust
let sink: Arc<dyn HostSink> = Arc::new(TauriSink::new(app));
run_task(sink, pending, approvals, advances, policy, signer, backend, req)?;
```

## The gates (every action passes through these)

1. **Permissions** — deny-by-default YAML policy: allow / require-approval / deny, with `*` and
   `prefix_*` patterns. Startup **linting** warns on dangerous configs.
2. **Human approval** — guarded actions block on a per-step channel until a human decides
   (`DenyApproval`, `AutoApprove`, or `TtyApproval` out of the box).
3. **Budget** — sliding-window actions-per-minute cap stops a runaway agent.
4. **Signed audit** — an Ed25519 receipt per executed action → append-only JSONL, verifiable
   offline. The agent never holds the signing key.
5. **Persistent memory** — every action across runs in SQLite, recalled into the prompt; enables
   resume.

## Inference backends

Swappable behind one `Inference` trait: `deterministic`/`scripted` (zero-cost, for tests & demos),
`claude-cli` (local Claude CLI), `ollama` (local HTTP), and `anthropic` (API). The host loop never
changes when you switch.

## Binaries

- `kriya-mcp` — the governed MCP server (external agents).
- `kriya-host` — the stdio sidecar (Electron/Node host the runtime via `kriya-sidecar`).

## Status

Alpha (`0.1.0`), part of the kriya framework. MIT licensed. The host crate ships **no** committed
`Cargo.lock` — consuming apps own dependency resolution.
