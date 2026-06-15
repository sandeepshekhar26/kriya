# @agent-native/core

Action registry + agent-loop protocol for **agent-native desktop apps** — apps where a local
AI agent is a first-class user. Declare your app's affordances once as typed actions; a bundled
agent calls them directly. No vision, no screenshots, no DOM selectors.

```ts
import { registerAction, getToolSchemas, dispatchAction } from "@agent-native/core";

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
npx agent-native dump ./dist/register-actions.js

# Scaffold wrapAction(...) registrations for a file's exported functions (the codemod)
npx agent-native wrap ./src/actions.ts --import ./actions.js > src/register-actions.ts
```

## Status

`v0.0.1` — early. Part of the [agent-native](https://github.com/sandeepshekhar26/verb) framework.
MIT licensed.
