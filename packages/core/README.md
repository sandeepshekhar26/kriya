# kriya-core

The TypeScript SDK for [**kriya**](https://github.com/sandeepshekhar26/kriya) (MCP for Desktop) — the governed runtime
that lets an AI agent safely drive a desktop app. Declare your app's affordances once as typed,
permission-scoped **actions**; an agent calls them directly (over MCP), and the host gates every
call. No vision, no screenshots, no DOM selectors.

Adopt it greenfield with `registerAction`, or bolt it onto a function you already have with
`wrapAction` — augment, not rewrite.

```ts
import { registerAction, getToolSchemas, dispatchAction } from "kriya-core";

registerAction({
  id: "create_note",
  description: "Create a new note with a title and optional content.",
  parameters: {
    title: { type: "string", required: true },
    content: { type: "string" },
    tags: { type: "array", items: "string" },
  },
  permissions: ["write:notes"],
  handler: async (params) => {
    const id = db.insert(params);
    return { success: true, data: { id } };
  },
});

// Hand these MCP-style schemas to the agent host (no handlers leave the app):
const tools = getToolSchemas();

// When the agent decides to call an action, run it through the same path a human uses:
const result = await dispatchAction("create_note", { title: "Hi" }, { caller: "agent" });
```

## What you get

- **`registerAction(def)`** — declare a typed, permission-scoped action. Returns a callable
  handle for direct/testing use.
- **`wrapAction(fn, opts)`** — bolt the action layer onto a function you *already* have
  (positional args, plain return, throws) without a rewrite. Maps the agent's params onto the
  function's arguments and normalizes its return/throw into an `ActionResult`. Augment, not migrate.
- **`getToolSchemas()`** — MCP-compatible tool schemas for every registered action (handlers
  excluded), for an agent host to ingest.
- **`dispatchAction(id, params, ctx)`** — validate params against the schema and run the handler.
- **`validateParams(params, schemas)`** — standalone runtime validation (types, enums, arrays,
  required).
- **Protocol types** — `AgentStartRequest`, `AgentActionRequest`, `AgentActionResult`,
  `AgentDone`, `AgentLog`, plus the Tauri command/event names. The transport-agnostic contract
  between your app and the agent host.

## CLI

```bash
# Print an app's registered actions as MCP tool schemas
npx kriya dump ./dist/register-actions.js

# Scaffold wrapAction(...) registrations for a file's exported functions (the codemod)
npx kriya wrap ./src/actions.ts --import ./actions.js > src/register-actions.ts
```

## Status

`v0.0.1` — alpha. Part of the [kriya](https://github.com/sandeepshekhar26/kriya) framework
(governed MCP-server mode, the `kriya-sidecar` Electron/Node host, and the `wrapAction`
bolt-on all ship alongside this). MIT licensed.
