# {{TITLE}}

An **agent-native** desktop app, scaffolded by
[`create-agent-app`](https://github.com/sandeepshekhar26/verb). Humans click
buttons; agents call the same typed actions. The Rust **agent host** gates every
action through a permission policy, a per-minute budget, and a signed audit log,
and persists every action to durable SQLite memory.

## Develop

```bash
npm install
npm run tauri dev    # first run compiles the Rust host (~2–3 min)
```

The dev server lives at `http://localhost:1420` — Vite serves the UI, Tauri
launches a native window pointing at it. Edit `src/*.tsx` for live HMR.

Pick an inference backend with `AGENT_BACKEND`:

```bash
AGENT_BACKEND=claude-cli npm run tauri dev          # uses your local `claude` CLI
AGENT_BACKEND=ollama OLLAMA_MODEL=llama3 npm run tauri dev
AGENT_BACKEND=anthropic ANTHROPIC_API_KEY=... npm run tauri dev
```

Default is `deterministic` — the scripted, zero-cost planner in
`src-tauri/src/deterministic.rs`. Swap in your own planner there, or rely on
the LLM backends.

## Build a release

```bash
npm run tauri build
```

Tauri produces a signed `.dmg` / `.msi` / `.AppImage` in
`src-tauri/target/release/bundle/`.

## ⚠️ Gotcha: release bundle shadows `tauri dev` on macOS

If you've already run `npm run tauri build` once and then go back to
`npm run tauri dev`, **macOS LaunchServices may foreground the old release
bundle** at `src-tauri/target/release/bundle/macos/{{NAME}}.app` instead of the
fresh dev binary — they share the same bundle identifier. The release bundle
ships its frontend embedded, so it can never show your latest UI changes no
matter how many times you reload.

If you see stale UI during `tauri dev`:

```bash
# Make sure no release bundle is in front of the dev binary.
pkill -f "target/release/bundle/.*{{NAME}}" 2>/dev/null
# Or delete the bundle entirely until you build again.
rm -rf src-tauri/target/release/bundle
```

(WKWebView also caches per-bundle-id at
`~/Library/WebKit/{{IDENTIFIER}}/` and `~/Library/Caches/{{IDENTIFIER}}/`. If
the bundle isn't the culprit, try clearing those too.)

## Where to add features

| Want to… | File |
|---|---|
| Add a new typed action humans + agents can call | [`src/actions.ts`](src/actions.ts) |
| Change app state shape | [`src/store.ts`](src/store.ts) |
| Change UI | [`src/App.tsx`](src/App.tsx) |
| Tighten/loosen permissions or approval gates | [`src-tauri/agent-policy.yaml`](src-tauri/agent-policy.yaml) |
| Plug in your own scripted planner | [`src-tauri/src/deterministic.rs`](src-tauri/src/deterministic.rs) |

## How the loop works

1. The app hands the agent its **state** (as JSON) and a **menu of typed
   actions** (`getToolSchemas()`).
2. The agent picks one action with parameters.
3. The Rust host gates it: permission check → human approval (if required) →
   rate limit → only then dispatch.
4. The app runs the *same* handler a human button would.
5. The host writes a **cryptographically signed receipt** to a JSONL log and
   records the action to durable SQLite memory.
6. Repeat until the agent reports done.

No vision. No DOM selectors. Just structured state and typed action calls.

## Read more

- [`@agent-native/core`](https://github.com/sandeepshekhar26/verb/tree/main/packages/core) — the SDK
- [`@agent-native/inspector`](https://github.com/sandeepshekhar26/verb/tree/main/packages/inspector) — the dev inspector you see on the right side of the app
- [`agent-native-host`](https://github.com/sandeepshekhar26/verb/tree/main/crates/agent-native-host) — the Rust agent host (audit, budget, memory, permissions, inference backends)
