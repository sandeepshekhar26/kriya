# @kriya/sidecar

Host the **kriya** agent runtime from **Electron or plain Node** — not just Tauri.

This package spawns the `kriya-host` sidecar (the Rust agent host from
[`crates/kriya`](../../crates/kriya)) and speaks its newline-delimited
JSON protocol over stdio. The agent loop, the inference backend, and the **entire safety layer
— policy, human approval, budget, signed audit — run inside that separate process**, which the
renderer can't tamper with. Your main process just executes the typed actions the host asks
for, the same handlers a button click would call.

```
 Electron / Node main process                 kriya-host (separate process)
 ───────────────────────────                  ────────────────────────────
   SidecarHost.spawn() ──spawn──▶  kriya-host
   host.start(goal,state,tools) ──stdin────▶  agent loop + governance
   run action, return state    ◀──stdout───  "run create_note(...)"   ← policy/approval/budget
   sendActionResult(...)        ──stdin────▶  signs an audit receipt
                                ◀──stdout───  "done"
```

## Install

```bash
npm install @kriya/sidecar
```

You also need the `kriya-host` binary (build it from the Rust crate, or ship it with your app):

```bash
cargo build -p kriya --bin kriya-host --release
```

## Quick start

The high-level `runTask` drives one run to completion: it sends the goal, executes each action
the host requests against your own state, answers approvals, and resolves with the summary.

```ts
import { SidecarHost, runTask } from "@kriya/sidecar";

const host = SidecarHost.spawn({
  binaryPath: "/path/to/kriya-host",
  args: ["--policy", "agent-policy.yaml"],
  env: { ...process.env, AGENT_BACKEND: "claude-cli" }, // or ollama / anthropic
});

// Your app's state + the same typed actions you'd register in @kriya/core.
let state = { notes: [] as { id: number; title: string }[] };

const done = await runTask(
  host,
  {
    goal: "Add a note titled 'Groceries'",
    state,
    tools: [
      { name: "create_note", description: "Create a note", inputSchema: { type: "object" } },
    ],
  },
  {
    // Execute the action and return the refreshed state. This is the SAME handler a
    // human button-click would invoke — the agent just calls it directly.
    dispatch: (req) => {
      if (req.actionId === "create_note") {
        const note = { id: state.notes.length + 1, title: String(req.params.title) };
        state = { notes: [...state.notes, note] };
      }
      return { stepId: req.stepId, success: true, state };
    },
    // Guarded actions (per your policy) come here. Default is to deny.
    approve: (req) => {
      console.log(`Agent wants to ${req.actionId} — approving`);
      return true;
    },
    onLog: (entry) => console.log(`[${entry.level}] ${entry.message}`),
  },
);

console.log(done.summary);
host.close();
```

### Deterministic runs (no LLM, no API key)

Pass `--script` to replay a recorded sequence of decisions — ideal for demos and CI:

```ts
const host = SidecarHost.spawn({
  binaryPath: "/path/to/kriya-host",
  args: ["--script", "demo-run.json"],
});
```

where `demo-run.json` is an array of the same decision objects an LLM emits:

```json
[
  { "action": "create_note", "params": { "title": "Demo" }, "reasoning": "seed a note" },
  { "done": true, "summary": "done" }
]
```

## Lower-level API

For finer control (multiple runs, step-mode, custom approval routing) use `SidecarHost`
directly and subscribe to events:

```ts
const host = SidecarHost.spawn({ binaryPath });
host.on("action", (req) => {
  /* run it, then: */ host.sendActionResult({ stepId: req.stepId, success: true, state });
});
host.on("approval", (req) => host.sendApproval({ stepId: req.stepId, approved: false }));
host.on("awaitStep", (ev) => host.sendStepAdvance({ gateId: ev.gateId, proceed: true }));
host.on("log", (entry) => console.log(entry.message));
host.on("done", (done) => console.log(done.summary));
host.on("exit", (code) => console.log(`sidecar exited: ${code}`));

host.start({ goal, state, tools });
```

### Events (host → app)

| Event | Payload | You respond with |
|---|---|---|
| `action` | `AgentActionRequest` | `sendActionResult` |
| `approval` | `AgentApprovalRequest` | `sendApproval` |
| `awaitStep` | `AgentAwaitStep` | `sendStepAdvance` |
| `done` | `AgentDone` | — |
| `log` | `AgentLog` | — |
| `parseError` | raw line `string` | — |
| `exit` | exit code `number \| null` | — |

The protocol types are re-exported from [`@kriya/core`](../core).

## License

MIT
