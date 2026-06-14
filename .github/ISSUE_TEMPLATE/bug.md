---
name: Bug report
about: Something that should work doesn't.
title: ""
labels: bug
---

## What happened

<!-- One paragraph. What did you see vs. what did you expect? -->

## Reproduction

<!--
Smallest steps that reproduce. Ideally a fresh scaffold:

  npm create agent-app@latest repro && cd repro
  npm install && npm run tauri dev
  # then …

If it needs custom code, link a minimal repo or paste the diff against
the scaffolder template.
-->

## Environment

- OS + version:
- Node version (`node -v`):
- Rust version (`rustc --version`):
- npm package versions:
  - `@agent-native/core`:
  - `@agent-native/inspector`:
  - `create-agent-app`:
- `agent-native-host` (crate) version:
- `AGENT_BACKEND` (deterministic / claude-cli / ollama / anthropic):

## Logs

<!--
Inspector log (you can copy with the export button), Rust host stderr, or
relevant `tauri dev` output. Redact API keys.
-->

```
(paste logs here)
```

## Anything else

<!-- Hunches, related issues, screenshots, etc. -->
