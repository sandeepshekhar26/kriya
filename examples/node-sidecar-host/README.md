# node-sidecar-host — host the governed runtime from Node / Electron

The flagship demo ([`../actual-budget-bolt-on`](../actual-budget-bolt-on)) proves the **MCP**
path: an *external* agent (Claude Desktop) drives an app over `kriya-mcp`. This example proves the
other cross-shell path — the **embedded sidecar** (roadmap R3, hardened to Tauri parity in P0.5):
your **own app process hosts the agent runtime** by spawning the `kriya-host` binary and driving
it over stdio.

It is the Electron answer to "we support Tauri." Electron's main process **is** Node, so the code
in [`run.mjs`](run.mjs) is byte-identical to what you'd put in an Electron `main.ts`.

## What it shows

One governed run, end to end, with **nothing trusted to the renderer**:

1. **`create_note` ×2** — allowed by policy, run directly against the app's own state.
2. **`delete_note`** — `delete_*` is policy-guarded, so the in-flight run **pauses for a human**;
   the example grants it. Deny it and the action never reaches your handler.
3. **Signed audit** — every executed action gets an Ed25519 receipt (`sig=…`) the agent can't
   forge, appended to `kriya-audit.jsonl`.
4. **`recentMemory()`** — durable episodic memory read back **over the sidecar protocol** (the
   P0.5 parity feature: the sidecar equivalent of Tauri's `agent_memory_recent`). Note the list
   includes episodes from *previous* runs — memory persists across process restarts.

All of policy, approval, budget, and audit run inside the **separate `kriya-host` process**, which
the UI can't tamper with. Your process only ever executes actions the host has already cleared.

## Run it

```bash
# 1. build the TS packages (kriya-core, kriya-sidecar)
npm run build --workspace kriya-core
npm run build --workspace kriya-sidecar

# 2. build the host binary (via note-app's lockfile — see the brotli pin in CLAUDE.md)
( cd apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked )

# 3. run the example (resolves the binary at the path from step 2; override with KRIYA_HOST_BIN)
node examples/node-sidecar-host/run.mjs
```

By default it runs a deterministic, no-API-key script (`demo-script.json`). To drive it with a
real model instead, set `AGENT_BACKEND=claude-cli` (or `ollama` / `anthropic`) in the environment.

## Mapping to a real Electron app

| In this Node example | In an Electron app |
|---|---|
| `SidecarHost.spawn({ binaryPath })` in `run.mjs` | the same call in your `main.ts` (ship the binary as an [extraResource](https://www.electronbuilder.org/configuration/contents#extraresources)) |
| `dispatch` mutates an in-memory `Map` | `dispatch` calls your real action handlers (the same ones your buttons call) |
| `approve` returns `true` at the console | forward the `approval` request to a renderer modal over `ipcMain`/`ipcRenderer`, resolve with the user's answer |
| `onLog` prints to stdout | pipe into the [`kriya-inspector`](../../packages/inspector) log view |
| `await host.recentMemory()` prints a table | feed into the inspector `<MemoryPanel/>` |

Nothing about the governed core changes between Tauri, Electron, and plain Node — only how you
surface the approval prompt and the inspector. That is the whole point of the `HostSink` seam.
