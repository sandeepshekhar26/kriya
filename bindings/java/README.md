# kriya — the JVM binding (R19)

The Java/Kotlin/Scala binding of [kriya](../../README.md): a **governed in-process action layer** for
JVM desktop apps (Swing / JavaFX, and any JVM service). Declare or wrap your app's real typed actions;
an AI agent drives them through **permission → human approval → budget → a signed audit trail**,
on-device, over the same `kriya-host` stdio protocol the TypeScript, Python, and .NET bindings speak.

> *The JVM half of regulated enterprise desktop ([D-012](../../docs/DECISIONS.md)): EU public sector,
> banks, hospitals, industrial control dashboards — the second-largest desktop surface after .NET. A
> second binding, not a new host: it speaks the existing NDJSON protocol to the one Rust `kriya-host`.*

**Zero runtime dependencies** — pure Java SE (a tiny built-in JSON codec; no Jackson/Gson). The Rust
`kriya-host` binary is the only external piece, and it's spawned over stdio, not imported.

## Install

```xml
<dependency>
  <groupId>com.kriyanative</groupId>
  <artifactId>kriya</artifactId>
  <version>0.0.1</version>
</dependency>
```

(Maven Central publish is the planner's — [D-004](../../docs/DECISIONS.md). Until then, build locally
with `mvn install`.)

## Quick start — register actions, let an agent drive them

```java
import com.kriyanative.kriya.*;
import java.util.*;

Registry reg = new Registry();

Map<String, ParameterSchema> params = P.params();
params.put("title", P.required(P.str()));
reg.registerAction("create_note", "Create a note with a title.",
    (p, ctx) -> {
        long id = db.createNote((String) p.get("title"));
        return ActionResult.ok(Map.of("id", id));
    },
    params, List.of("write:notes"), 1);          // policy decides: allow / require approval / deny

// Spawn the governed host (built from crates/kriya) and run an agent against your registry.
try (Host host = Host.spawn("/path/to/kriya-host",
        List.of("--policy", "agent-policy.yaml"),
        Map.of("AGENT_BACKEND", "claude-cli"))) {

    Map<String, Object> state = new LinkedHashMap<>();
    state.put("notes", new ArrayList<>());

    Done done = Host.runTask(host, reg, "tidy up the notes", state,
        req -> /* show a modal in your app's UI */ true,           // approve
        e -> System.out.println("[" + e.level + "] " + e.message)  // onLog
    ).get();
}
```

A human clicks a button; an agent calls `create_note` — both run the *same* handler, and the agent's
call still passes permission → approval → budget → audit on the way in (enforced in the host process
your UI can't tamper with).

### Kotlin

```kotlin
val reg = Registry()
reg.registerAction("create_note", "Create a note.", { p, _ ->
    ActionResult.ok(mapOf("id" to db.createNote(p["title"] as String)))
}, mapOf("title" to P.required(P.str())))
```

## Bolt onto an app you already have — `wrapAction`

```java
reg.wrapAction(args -> actual.deleteTransaction((String) args[0]),
    "delete_transaction", "Permanently delete a transaction.",
    Map.of("id", P.required(P.str())),
    p -> new Object[] { p.get("id") },   // mapParams
    r -> r);                              // mapResult  →  policy: require_approval
```

Adapt a function the app already exposes — positional args, plain return, throws — into a registered,
governed, agent-callable action in a few lines, no rewrite.

## What's in the box

| Type | What |
|---|---|
| `Registry` | `registerAction` / `wrapAction`, validation, composition (`ctx.call`), `toolSchemas` (MCP/JSON-Schema) |
| `Host` | spawn `kriya-host`, drive it over stdio; listeners, `runTask`, `recentMemory` |
| `P`, `ParameterSchema` | typed parameter schemas (`P.str()`, `P.num()`, `P.required(...)`, `P.array(...)`, `P.obj(...)`) |
| `ActionResult` / `ActionContext` | handler return + execution context |
| `Json` | the dependency-free JSON codec (so the binding has zero runtime deps) |

JSON values are plain Java: `Map<String,Object>` / `List<Object>` / `String` / `Long` / `Double` /
`Boolean` / `null`.

## Build + test

```bash
mvn -q test                                    # 44 unit tests, Java 11+
# integration test + example against the real binary (opt-in):
( cd ../../apps/note-app/src-tauri && cargo build -p kriya --bin kriya-host --locked )
export KRIYA_HOST_BIN=../../apps/note-app/src-tauri/target/debug/kriya-host
mvn -q test                                    # now also runs IntegrationTest
mvn -q compile exec:java                        # runs the bundled NoteAppHost example
```

Targets **Java 11** (a broad LTS floor for the enterprise-desktop ICP); compiles + runs on newer JDKs.
`mvn -Prelease package` attaches the source + javadoc jars for Maven Central.

## Status

Alpha. The binding + protocol are verified end-to-end against the real `kriya-host` (44 unit tests +
a full action → approval → memory-recall integration run, plus a runnable `NoteAppHost` example that
drives the governed flow: two signed creates, a `delete_note` held for human approval, signed memory
recalled). A bolt-on demo against a real JVM desktop app is the flagship follow-on.
