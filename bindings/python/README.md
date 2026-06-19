# kriya (Python)

> **The governed in-process action layer for Python apps.** Your app's capabilities become typed
> **actions** an AI agent calls directly — through permission → human approval → budget → a signed
> audit trail, on-device — not by screenshotting your window and guessing where to click.

This is the **Python binding** of [kriya](https://github.com/sandeepshekhar26/kriya) (roadmap R17).
It speaks the same stdio/NDJSON protocol to the same Rust `kriya-host` binary that the Node
[`kriya-sidecar`](../../packages/sidecar/) does — *a second binding, not a new host*. So a
PyQt/PySide/Tk desktop app, a **FreeCAD/Blender** plugin, a Jupyter/data tool, or a quant/finance
workstation gains a governed, agent-callable surface in a few lines per handler.

## Why a Python binding

The biggest unserved *in-process* surface for governed agents is Python: CAD (FreeCAD, Blender),
data/ML notebooks, scientific & engineering desktop tools, and quant/finance apps — all local,
often with no clean cloud API, frequently over private data. They're exactly the apps that need an
on-device agent that **can't move money, delete a model, or touch a record without permission +
approval + a verifiable audit trail**. kriya is that layer.

## Install

```bash
pip install kriya          # the Python package (registry + host driver)
```

You also need the **`kriya-host`** binary (the Rust agent host — the governance lives in that
separate process so your UI can't tamper with it). Build it from the kriya repo:

```bash
(cd apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked)
# → target/debug/kriya-host   (point KRIYA_HOST_BIN at it, or pass the path to Host.spawn)
```

## Quick start — build a new agent-drivable app

Declare a capability once. A human triggers it by clicking; an agent triggers it by calling — the
*same* handler underneath.

```python
import kriya
from kriya import string, required, ok

notes = []

kriya.register_action(
    id="create_note",
    description="Create a note with a title.",
    parameters={"title": required(string)},
    permissions=["write:notes"],           # the policy decides: allow / require approval / deny
    handler=lambda p, ctx: (notes.append(p["title"]), ok({"id": len(notes)}))[1],
)

# Spawn the governed host (deterministic --script run shown; set AGENT_BACKEND=claude-cli|ollama|
# anthropic to drive it with a real model). The agent's proposals pass policy → approval → budget →
# signed audit *inside that process* before they ever reach your handler.
host = kriya.Host.spawn("/path/to/kriya-host", ["--script", "demo.json"])
done = kriya.run_task(
    host,
    goal="tidy up the notes",
    state=lambda: {"notes": list(notes)},
    registry=kriya.default_registry(),     # tools + dispatch derived from the registry
    approve=lambda req: True,              # a guarded action pauses here for a human
    on_log=lambda e: print(f"[{e.level}] {e.message}"),
)
print(done.summary, done.steps)
host.close()
```

## Bolt onto an app you already have — `wrap_action`

Wrap a function the app *already exposes*; no rewrite. A returned value becomes
`ActionResult(success=True, data=...)`; a raised exception becomes a failed result.

```python
import kriya
from kriya import string, required

kriya.wrap_action(
    actual.delete_transaction,             # a function your app already has
    id="delete_transaction",
    description="Permanently delete a transaction.",
    parameters={"id": required(string)},
    map_params=lambda p: [p["id"]],        # params dict → positional args
)
# policy: delete_* → require_approval, so this pauses for a human before it runs.
```

## Governance — built in, enforced in the host process

Every action an agent proposes runs this gauntlet **on-device**, before it executes:

1. **Permission** — a deny-by-default policy: allow / require-approval / deny.
2. **Human approval** — guarded actions pause for your `approve` callback (wire it to a modal).
3. **Budget** — a per-minute cap stops a runaway or looping agent.
4. **Signed audit** — an Ed25519 receipt per action → append-only log, verifiable offline.

Plus durable **memory** across runs (`host.recent_memory()`), policy **linting**, and
**step-through** (`step_mode=True` + an `on_step` gate). A jailbroken agent still can't get past the
gates: the *host* owns the policy and the signing key, not the agent.

## API at a glance

| | |
|---|---|
| **Registry** | `register_action`, `wrap_action`, `dispatch_action`, `tool_schemas`, `mcp_tool_schemas`, `Registry`, `default_registry`, `clear_registry` |
| **Schema helpers** | `string`, `number`, `boolean`, `required(...)`, `array(...)`, `obj(...)`, `ParameterSchema`, `ok(...)`, `err(...)` |
| **Host driver** | `Host.spawn(binary_path, args, env)` · `.on(event, fn)` · `.start/.send_action_result/.send_approval/.send_step_advance` · `.recent_memory(limit)` · `.close()` |
| **High-level** | `run_task(host, *, goal, state, registry=…, approve=…, on_step=…, on_log=…)` |
| **Protocol types** | `ActionRequest`, `ApprovalRequest`, `AwaitStep`, `Done`, `LogEntry`, `Episode`, … |

## Develop / test

The package is pure standard library (zero runtime dependencies). Tests run with `unittest` (no
install needed) or `pytest`:

```bash
cd bindings/python
PYTHONPATH=src python -m unittest discover -s tests        # unit tests (no binary)
# Integration test vs the real host (opt-in):
KRIYA_HOST_BIN=/path/to/kriya-host PYTHONPATH=src python -m unittest tests.test_integration
```

The integration test drives the **real** `kriya-host` through action dispatch + a held/granted
approval + durable memory recall — the same end-to-end proof the Node sidecar ships.

## Status

Alpha, MIT-licensed. Mirrors `kriya-core` (registry/schema/validation) + `kriya-sidecar` (host
driver) for Python. Handlers are synchronous (they run on the host reader thread); async handler
support is a follow-up. The PyPI distribution name is finalized by the planner at publish time
([D-004](../../docs/PUBLISHING.md)); the import name is always `kriya`.
