# create-agent-app

Scaffold a new local-first agent app — built on [**verb**](https://github.com/sandeepshekhar26/verb),
the governed runtime that lets an AI agent safely drive a desktop app — in one command.

```bash
npm create agent-app@latest my-app
# or
npx create-agent-app my-app
```

> Already have an app? You don't need to start from scratch — bolt verb onto your existing
> handlers with `wrapAction` (see [`@agent-native/core`](https://www.npmjs.com/package/@agent-native/core)).
> This scaffolder is for greenfield apps.

You get a working Tauri 2 + React + TypeScript desktop app with the agent host
(`@agent-native/core` SDK + Rust agent loop) already wired in: typed actions,
permission policy, human-approval queue, action budget, signed audit, persistent
memory, and four swappable inference backends (deterministic, claude-cli, ollama,
anthropic).

The starter is a tiny **counter app**: humans click the buttons; the agent calls
the same actions to reach a target count. `reset_counter` is gated behind human
approval to show the approval flow.

## Next steps

```bash
cd my-app
npm install
npm run tauri dev
```

First run compiles the Rust agent host — a few minutes. After that, click
**Run agent** and watch it operate your app through typed actions.

Pick a backend with `AGENT_BACKEND` (`deterministic` is the default and needs
no setup):

```bash
AGENT_BACKEND=claude-cli npm run tauri dev
AGENT_BACKEND=ollama OLLAMA_MODEL=llama3 npm run tauri dev
AGENT_BACKEND=anthropic ANTHROPIC_API_KEY=... npm run tauri dev
```

See the project README and `architecture.md` once scaffolded.
