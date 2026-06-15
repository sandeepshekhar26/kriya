# Contributing to kriya

Thanks for poking at this. kriya is alpha — feedback, bug reports, and PRs are all
welcome. The goal is the framework where AI agents are first-class users of
desktop apps, on the Tauri 2 + Rust + TypeScript + React stack.

## Quick start

```bash
git clone https://github.com/sandeepshekhar26/kriya.git
cd kriya
npm install
npm run build --workspace kriya-core
npm run build --workspace kriya-inspector
```

Then either reference app boots in one command:

```bash
npm run tauri dev --workspace note-app          # http://localhost:1420
npm run tauri dev --workspace task-manager      # http://localhost:1421
```

First Rust build takes a few minutes. Subsequent rebuilds are incremental.

## Running the checks CI runs

```bash
# JavaScript side
npm run test --workspace kriya-core
( cd apps/note-app && npx tsc --noEmit )
( cd apps/task-manager && npx tsc --noEmit )

# Rust side
( cd crates/kriya && cargo test --locked )
( cd apps/note-app/src-tauri && cargo check --locked )
( cd apps/task-manager/src-tauri && cargo check --locked )
```

CI runs the same matrix on every push and pull request. A green CI is a
soft prerequisite for merging — if yours is red, say why in the PR.

## Repository layout

```
kriya/
├── packages/
│   ├── core/                 # kriya-core — TS SDK + protocol types
│   ├── inspector/            # kriya-inspector — React inspector + StepGate + MemoryPanel
│   └── create-kriya-app/     # the scaffolder (npm create kriya-app)
├── crates/
│   └── kriya/    # Rust agent host — protocol, audit, budget, memory, permissions, inference backends
├── apps/
│   ├── note-app/             # reference app #1
│   └── task-manager/         # reference app #2 (same crate, different domain)
├── tools/
│   └── verify-receipts/      # offline Ed25519 audit-log verifier (Rust)
├── architecture.md           # how the pattern works end to end
└── docs/PRODUCT_GAPS.md      # living roadmap — demo → full product
```

The split: the host crate and the inspector package both deliberately know
nothing about any particular app domain. App-specific code (planners,
actions, store, UI) lives under `apps/`. New reference apps are the best
way to stress-test the framework's generality.

## What's a good first PR

- **Bug fixes** — always welcome. Include a repro.
- **New reference app under `apps/`** — small (≤ 500 LOC) is fine. Anything
  that exercises a different action shape than notes or tasks (e.g. a
  spreadsheet cell, a CRM contact, a timer) helps prove the framework
  generalizes.
- **New inference backend** under `crates/kriya/src/agent/inference/`
  implementing the `Inference` trait. Mistral, OpenAI, OpenRouter are all
  open.
- **Inspector polish** — clearer empty states, better keyboard nav, theming
  via CSS variables.
- **Tests** for any module currently under-tested. The host crate has more
  surface area than its test count suggests.

For anything larger (a new package, a protocol change, a new top-level
directory), open an issue first so we can sort out the design before you
write code that gets reshaped in review.

## Coding conventions

- **TypeScript**: strict, `noUncheckedIndexedAccess: true`. Prefer narrow
  types over `any`. Keep public exports in `index.ts` files; intra-package
  imports use explicit `.js` suffixes (the SDK is published as ESM and Node
  ESM strict mode requires them — there's a real bug fix in the git log
  that proves this).
- **Rust**: `cargo fmt` clean (`rustfmt.toml` is intentionally absent —
  defaults), `cargo clippy` clean for new code. The `--locked` flag is
  used in CI; if you bump a dep, regenerate the relevant `Cargo.lock` in
  the same commit.
- **Commits**: short imperative subject (`host: extract … crate`,
  `inspector: step-through replay`), body explains *why* the change
  exists. Co-authored-by trailers for AI-pair-programmed work are fine.
- **Comments**: explain *why*, not *what*. The current codebase is
  deliberately light on comments — added only where the next reader will
  ask "wait, why?".

## Reporting bugs

Use the bug template in
[`.github/ISSUE_TEMPLATE/bug.md`](.github/ISSUE_TEMPLATE/bug.md). The
fields are minimal — please fill them. "It doesn't work" without a repro
is hard to act on.

## Reporting security issues

Email `skumar5190@gmail.com` rather than opening a public issue. The audit
log, memory store, and approval queue are all security-relevant surface;
a private channel is better than a race.

## Architecture & roadmap reading list

- [`README.md`](README.md) — the elevator pitch.
- [`architecture.md`](architecture.md) — protocol, threading model,
  permission gate, audit flow.
- [`docs/PRODUCT_GAPS.md`](docs/PRODUCT_GAPS.md) — honest list of what's
  shipped, what's partial, and what's still missing. This is the doc to
  read before proposing a roadmap change.

## License

MIT. By contributing, you agree your contribution is licensed under MIT.
