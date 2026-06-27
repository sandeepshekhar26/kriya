# CLAUDE.md — start here every session

This file is auto-loaded at the start of every Claude Code session. It is the **orientation
layer** for the project. The planner (Sandeep) decides direction; the agent (you) builds.
Read the four linked docs below before doing strategic work — never start research or planning
from scratch when a doc already holds the answer.

## What this project is

A framework/runtime for **kriya software**: apps where AI agents are first-class users,
driving the app through **typed actions** (not screenshots, not DOM scraping). Same handler
serves a human button-click and an agent tool-call. Built on **Tauri 2 + Rust agent host +
TypeScript SDK + React**, with a safety layer (permissions, human approval, signed audit,
budget, persistent memory) built in, not bolted on.

The long-term ambition is a **full, hard-to-copy, YC-ready** product — not a toy demo. When
scoping work, bias toward the copy-resistant depth, and don't oversell small wins.

## The source of truth lives in the repo (not in memory)

Claude's machine-local memory is a fast cache only. **These repo docs are canonical** — if
they ever disagree with memory, the repo wins (decision [D-008](docs/DECISIONS.md)).

| Doc | What it holds | Read it when |
|---|---|---|
| [docs/ROADMAP.md](docs/ROADMAP.md) | **What to build next**, prioritized. The checklist the planner points you at. | Starting any build work |
| [docs/strategy/](docs/strategy/) | **Why** — market landscape + the strategic wedge | Any positioning / strategy / "should we change X" question |
| [docs/PRODUCT_GAPS.md](docs/PRODUCT_GAPS.md) | **Feature-completion state** — what's done / partial / missing | Checking what exists before building |
| [docs/DECISIONS.md](docs/DECISIONS.md) | **Decisions + why**, append-only | Before re-opening a settled question |
| [docs/PUBLISHING.md](docs/PUBLISHING.md) | npm/crates.io publish runbook — initial publish **done** (R2, 2026-06-15); now the **republish** runbook | Republishing after changing a published package/crate |
| [docs/LAUNCH.md](docs/LAUNCH.md) | Show HN plan, opening comment, objection answers, Twitter thread | Working roadmap item R12 / launch |
| [architecture.md](architecture.md) | How the bolt-on / in-process **pattern** works end to end | Touching the protocol / host loop |
| [docs/SERVICE-ARCHITECTURE.md](docs/SERVICE-ARCHITECTURE.md) | **The service architecture** (D-016): one governance core + three reach fronts + the shippable `kriya-gateway` product; tech + sequence diagrams + build-over plan | Building the gateway / proxy / fronts (R22–R26) |
| [docs/THREE-FRONTIERS.md](docs/THREE-FRONTIERS.md) | **Local-only & SUPERSEDED (2026-06-27).** The original "three frontiers" positioning essay — premise (desktop has no agent tooling standard) is outdated (desktop has MCP-stdio too). The public positioning is now the **governance / control-plane** thesis in the README. Kept for history; **do not re-surface the three-frontiers framing publicly.** | Positioning history only — never as the current pitch |

## Current direction (as of 2026-06-24)

- **Working thesis (planner-affirmed, not yet market-validated):** the bet is the governed
  **in-process** action layer for capabilities that live only inside a running local app (no API
  to wrap, local/private data) — desktop/in-process is the mechanism, governance the moat, **one
  bet, not two**. Dual GTM: build-time adoption for new local-first AI apps + bolt-on migration
  of existing apps (`wrapAction`) + enterprise governance. ICP: POS (planner is building one),
  CRM, regulated workstations, near-future agentic SaaS. Don't regress to "generic governance" —
  see [docs/strategy/governed-local-first-wedge.md](docs/strategy/governed-local-first-wedge.md)
  and decision D-009.
- **Pivot (D-016, 2026-06-24) — the new front door is a PRODUCT, not the library.** MCP-stdio
  commoditized the *transport*, not the *enforcement* (native MCP has no enforced governance:
  approval is SHOULD-not-MUST + client-side, auth is OPTIONAL + excludes stdio). So ship a
  **standalone `kriya-gateway`** — a zero-change governance **proxy** that wraps any existing MCP
  server with policy + approval + signed audit — and reposition `wrapAction` as enterprise depth.
  Architecture = **one transport-agnostic core + three reach fronts** (proxy / OS-accessibility
  reach-in / computer-use), **build-over not rewrite**. See
  [docs/SERVICE-ARCHITECTURE.md](docs/SERVICE-ARCHITECTURE.md) + D-016. The wedge (D-009) is
  unchanged; only the GTM front door and the reach mechanism are sharpened.
- **Next builds (PG tier — the current critical path, in order):** `R22` upstream MCP client +
  `McpProxyExecutor` → `R23` Front-1 transparent proxy loop (+ conformance test) → `R24` ⭐ the
  shippable `kriya-gateway` product → `R25` Front-2 reach-in (coverage-gated) → `R26` Front-3
  computer-use (deferred). The P0/P0.5 critical path (R1→R5, cross-shell parity) is **done**. See the roadmap.
- **Flagship goal (`R24`, ⭐):** a downloadable `kriya-gateway` that wraps a real existing MCP server
  with the approval modal + signed receipt in zero integration code — the new YC demo. The prior
  flagship (`R5` Actual Budget in-process bolt-on) is shipped and remains the enterprise-depth proof.

## How a session works (the loop)

1. **Orient** — read this file + `docs/ROADMAP.md`. Check `docs/DECISIONS.md` if the task
   touches a past decision.
2. **Pick** — work the item the planner named (e.g. "do R1"), or the top unblocked roadmap
   item if told "continue."
3. **Build** — implement in small, verifiable steps. Use the in-session task list for
   multi-step work.
4. **Verify before claiming done** — run the checks (below). Never report green without
   running them. For UI changes, run the app and look (see `/run`).
5. **Document** — move the shipped item to **Done** in `docs/ROADMAP.md` with the commit SHA;
   update `docs/PRODUCT_GAPS.md`; log any decision in `docs/DECISIONS.md`.
6. **Ship** — commit (co-author trailer) and push **only when the planner asks**. One feature
   per commit; verify each before committing.

## Verify commands (what CI runs)

```bash
# JavaScript
npm run test --workspace kriya-core
( cd apps/note-app && npx tsc --noEmit )
( cd apps/task-manager && npx tsc --noEmit )

# Rust — run host tests through note-app's lockfile (has the alloc-no-stdlib/brotli pin)
( cd apps/note-app/src-tauri && cargo test -p kriya --locked )
( cd apps/note-app/src-tauri && cargo check --locked )
( cd apps/task-manager/src-tauri && cargo check --locked )
```

Rust toolchain is pinned to 1.90.0. Cargo needs `source $HOME/.cargo/env` in fresh shells.

## Repo layout

```
packages/core/                 kriya-core — TS SDK + protocol types
packages/inspector/            kriya-inspector — React inspector (StepGate, MemoryPanel, ApprovalModal)
packages/create-kriya-app/     the scaffolder (npm create kriya-app)
crates/kriya/      Rust agent host — protocol, audit, budget, memory, permissions, inference backends
apps/note-app/                 reference app #1 (notes)
apps/task-manager/             reference app #2 (tasks) — same crate, different domain
tools/verify-receipts/         offline Ed25519 audit-log verifier
docs/                          strategy, roadmap, decisions, product gaps
```

## Conventions

- **TypeScript**: strict, `noUncheckedIndexedAccess`. Intra-package imports use explicit `.js`
  suffixes (published ESM; Node strict mode requires them — there's a real bug fix proving this).
- **Rust**: `cargo fmt` clean, `clippy` clean for new code, `--locked` in CI. The host crate
  is a library and ships **no** committed `Cargo.lock` (apps own resolution).
- **Commits**: short imperative subject, body explains *why*. End with
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Comments**: explain *why*, not *what*. Match the surrounding density.

## How the planner hands off work (templates)

- **Continue:** "continue" → take the top unblocked item in `docs/ROADMAP.md`.
- **Specific:** "do R3" / "do the flagship demo" → work that roadmap item.
- **Strategy:** "should we X?" → answer from `docs/strategy/` + `docs/DECISIONS.md` first;
  research only the genuinely new part; write findings back into the relevant doc.
- **Publishing / irreversible:** the agent prepares and hands the planner exact commands; the
  planner runs them (decision [D-004](docs/DECISIONS.md)).
