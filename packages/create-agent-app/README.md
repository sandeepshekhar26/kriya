# create-agent-app

Scaffold a new agent-native desktop app in one command.

```bash
npm create agent-app@latest my-app
# or
npx create-agent-app my-app
```

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
