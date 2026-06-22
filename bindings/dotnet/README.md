# Kriya — the .NET binding (R18)

The C#/.NET binding of [kriya](../../README.md): a **governed in-process action layer** for .NET
desktop apps (WPF / WinForms / Avalonia / MAUI). Declare or wrap your app's real typed actions; an AI
agent drives them through **permission → human approval → budget → a signed audit trail**, on-device,
over the same `kriya-host` stdio protocol the TypeScript and Python bindings speak.

> *The #1 target after Python ([D-012](../../docs/DECISIONS.md)): WPF/WinForms is the bullseye of the
> ICP — regulated Windows desktop (health, finance, manufacturing HMIs, gov). The only existing .NET
> desktop MCP path is accessibility-tree scraping; this gives the typed-action, governed alternative.*

## Install

```bash
dotnet add package Kriya
```

(NuGet publish is the planner's — [D-004](../../docs/DECISIONS.md). Until then, reference the project
or build locally.)

## Quick start — register actions, let an agent drive them

```csharp
using Kriya;
using System.Text.Json.Nodes;

var reg = new Registry();

reg.RegisterAction("create_note", "Create a note with a title.",
    (p, ctx) => ActionResult.Ok(new JsonObject { ["id"] = Db.CreateNote(p["title"]!.GetValue<string>()) }),
    parameters: new() { ["title"] = P.Required(P.Str) },
    permissions: new[] { "write:notes" });          // policy decides: allow / require approval / deny

// Spawn the governed host (built from crates/kriya) and run an agent against your registry.
using var host = Host.Spawn("/path/to/kriya-host", new[] { "--policy", "agent-policy.yaml" },
    new Dictionary<string, string> { ["AGENT_BACKEND"] = "claude-cli" });

var state = new JsonObject { ["notes"] = new JsonArray() };
var done = await Host.RunTask(host, reg, goal: "tidy up the notes", state: state,
    approve: req => /* show a modal in your app's UI */ true,
    onLog: e => Console.WriteLine($"[{e.Level}] {e.Message}"));
```

A human clicks a button; an agent calls `create_note` — both run the *same* handler, and the agent's
call still passes permission → approval → budget → audit on the way in (enforced in the host process
your UI can't tamper with).

## Bolt onto an app you already have — `WrapAction`

```csharp
reg.WrapAction(args => actual.DeleteTransaction((string)args[0]!),
    id: "delete_transaction", description: "Permanently delete a transaction.",
    parameters: new() { ["id"] = P.Required(P.Str) },
    mapParams: p => new object?[] { p["id"]!.GetValue<string>() });   // policy: require_approval
```

Adapt a function the app already exposes — positional args, plain return, throws — into a registered,
governed, agent-callable action in a few lines, no rewrite.

## What's in the box

| Type | What |
|---|---|
| `Registry` | `RegisterAction` / `WrapAction`, validation, composition (`ctx.Call`), `ToolSchemas` (MCP/JSON-Schema) |
| `Host` | spawn `kriya-host`, drive it over stdio; events, `RunTask`, `RecentMemoryAsync` |
| `P`, `ParameterSchema` | typed parameter schemas (`P.Str`, `P.Num`, `P.Required(...)`, `P.Array`, `P.Obj`) |
| `ActionResult` / `ActionContext` | handler return + execution context |

## Build + test

```bash
dotnet build Kriya.slnx -c Release            # builds net8.0 (LTS) + net10.0
dotnet test  Kriya.slnx                        # unit tests
# integration test against the real binary (opt-in):
( cd ../../apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked )
KRIYA_HOST_BIN=../../apps/note-app/src-tauri/target/debug/kriya-host dotnet test Kriya.slnx
```

Targets **net8.0** (LTS — the regulated Windows-desktop ICP) and **net10.0**. The binding is a
*second binding, not a new host*: it speaks the existing NDJSON protocol to the one Rust `kriya-host`.

## Status

Alpha. The binding + protocol are verified end-to-end against the real `kriya-host` (25 tests incl. a
full action → approval → memory-recall integration run). A bolt-on demo against a real high-stars .NET
desktop app is the flagship follow-on (R18, in progress).
