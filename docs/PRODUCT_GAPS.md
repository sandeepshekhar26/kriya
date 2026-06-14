# Product Gaps — from Phase 0 demo to YC-ready framework

> Living document. The Phase 0 note app proves the *pattern*. It is **not** the product.
> This file tracks the distance to a full, defensible, hard-to-copy framework. Update it as
> things land. The moat is depth + DX taste + breadth + being first — not the demo.

Legend: ✅ done · 🟡 partial / proof-only · ⬜ not started

## 1. Core SDK (`@agent-native/core`)
- ✅ `registerAction`, typed schemas, MCP-style `getToolSchemas()`
- ✅ Runtime param validation (types/enum/array/required) + 13-test suite
- ⬜ Standards-compliant **JSON Schema** export (delegated — see prompt) so any MCP client
  consumes schemas directly
- ✅ Action **composition** — handlers receive `ctx.call(childId, params)` and
  can invoke any other registered action. The child runs through the same
  validation + audit path; cycles and depth-cap violations surface as failed
  `ActionResult`s (not throws). Capped at `MAX_COMPOSE_DEPTH = 8`. Demonstrated
  in `apps/task-manager` via `bulk_create_tasks`. Tested with 6 new unit tests
  (34/34 SDK tests green).
- ⬜ Action **versioning + migrations** (v1→v2 with guides) — field exists, no machinery
- ⬜ Framework-agnostic bindings (Vue/Svelte/Solid), not just React
- ⬜ Hot-reload of the registry in dev (change an action, agent sees it instantly)

## 2. Agent runtime (Rust host)
- ✅ Step loop, swappable `Inference` trait, deterministic + claude-cli backends
- 🟡 Real inference backends: Ollama (HTTP) + Anthropic API added behind the trait
  (compile-verified; live runs pending a local model / API key). OpenAI still ⬜.
- 🟡 **Persistent memory**: episodic log persisted to SQLite (every action across runs,
  newest-first query via `agent_memory_recent`, count surfaced at run start) AND recalled
  into the LLM prompt as a MEMORY section, so prior runs inform decisions. State snapshots
  + vector recall (embeddings) still ⬜.
- ⬜ Resume-ability (crash/pause → resume from last completed action with full context)
- ⬜ Retry/backoff, graceful "this is too hard → escalate to frontier model" fallback
- ⬜ Multi-agent orchestration (concurrent agents per app)
- ⬜ Separate-process agent host (don't share Tauri's main thread) + latency profiling (<500ms p50)

## 3. Permissions, approval & audit
- ✅ YAML policy + deny-by-default + `RequiresApproval` decision
- ✅ **Human-approval queue**: host pauses on a guarded action, emits `agent://approval`,
  blocks on a per-step channel (5-min timeout = deny), frontend modal approve/deny, resumes
- ⬜ Approval **queue UI** for multiple pending approvals + per-action policy editor in-app
- 🟡 Budgets/rate-limits — actions/minute sliding-window cap enforced (host stops the run);
  api-calls/hr still ⬜
- 🟡 Ed25519 signed receipts → JSONL — works; **verifier** CLI delegated (see prompt) + tamper tests ⬜
- ⬜ Policy linting + dev-mode "your policy is too permissive" warnings

## 4. State sync & protocol
- 🟡 Request/response over Tauri IPC, full-state snapshots per step
- ⬜ **Incremental state patches** (Immer/JSON-Patch) instead of whole-state each step
- ⬜ Versioned protocol + adapters for WebMCP / AG-UI churn
- ⬜ Transport portability proven off-IPC (WebSocket dev inspector, gRPC cloud)

## 5. Developer experience
- ✅ `create-agent-app` scaffolder — `npm create agent-app@latest my-app` produces a
  working counter-app starter (Tauri 2 + Rust host + React + SDK + all safety infra,
  locked deps). Verified end-to-end: TS compiles, `cargo check --locked` passes.
  Generated apps now ship a `README.md` with the develop / build commands, a
  per-app file map ("where to add features"), and a documented macOS gotcha —
  `target/release/bundle/macos/<app>.app` shadows `tauri dev` via LaunchServices
  after a prior release build, with a one-liner to kill it.
- ✅ Rich dev dashboard/inspector — extracted into `@agent-native/inspector`
  (workspace package, v0.2.0). Filterable log (toggle levels + full-text search),
  per-step expand, one-click JSONL export of the current run, and a `MemoryPanel`
  that reads durable past runs from the host's SQLite memory via the
  `agent_memory_recent` Tauri command. **Step-through replay**: clicking an
  episode in MemoryPanel opens its detail, then Prev/Next buttons (or ←/→ keys,
  Esc to close) walk through neighbouring episodes one at a time — keyboard nav
  is suppressed while typing in inputs so the inspector's filter box still works.
  Both reference apps consume the package; styles ship via
  `@agent-native/inspector/styles.css` and are themable through CSS variables.
  Real *live* in-host pause-between-steps (so a dev can break/inspect mid-run)
  still ⬜ — pending a host-side pause channel.
- 🟡 CLI to dump registered actions as JSON — added in 1.1
- ⬜ Templates, examples gallery, "build an agent app in <2 hours" tutorial
- ⬜ OpenTelemetry traces; CI eval gate ("does my app still work with agents?")
- ✅ Extract Rust host into a shared crate (`agent-native-host`). `apps/note-app`
  now path-depends on it and is ~110 lines of glue around `run_task` +
  `select_backend_with_default`. Scaffolder template still ships an embedded
  copy — the swap requires either (a) publishing the crate to crates.io and
  using `agent-native-host = "0.1"`, or (b) restructuring the repo as a Cargo
  workspace so the template can use a git dep. See follow-up below.

## 6. Breadth / ecosystem (the copy-resistance)
- ⬜ Electron binding (`@agent-native/electron`) — largest JS audience
- ⬜ Mobile (Flutter, SwiftUI, Jetpack Compose)
- ⬜ Agent-native component library ("shadcn for agent-operable UI")
- ⬜ Agent/skills registry; MCP-server generation (reverse-MCP)
- 🟡 Reference apps beyond notes: **task manager ✅** (apps/task-manager — six
  typed actions including two approval-gated; its own `TaskPlanner` plugged into
  the shared `agent-native-host` crate via `select_backend_with_default`).
  Spreadsheet and personal CRM still ⬜.

## 7. Product / business (Phase 4)
- ⬜ Open-core: Pro (multi-agent, cloud sync, audit dashboard), Enterprise (SAML, compliance)
- ⬜ Hosted agent cloud (remote inference, persistent memory, scaling)
- ⬜ Integrations marketplace (Stripe/Slack/GitHub/Salesforce via credential vaults)

## Near-term focus (what actually builds the moat next)
1. ✅ Real inference backends (Ollama + Anthropic) — usable for real tasks.
2. ✅ Human-approval queue + budget enforcement — safety story enterprises pay for.
3. 🟡 `create-agent-app` ✅ + a real inspector ⬜ — DX that drives GitHub virality.
4. ⬜ Second reference app (task manager) — proves generality, not a one-off.

**Next up (in order):**
- ✅ **Second reference app (task manager).** Shipped — two apps now share the
  same crate, each plugging in their own scripted planner. Generality proven.
- **Publish `agent-native-host` to crates.io + `@agent-native/core` to npm.**
  Until both are public, the scaffolder template still embeds the host code.
  Publishing unblocks the template swap and is the precondition for going
  public on GitHub.
- After publishing unblocks: real step-through (pause-between-steps in the host)
  and live replay of a stored run inside the inspector — building on the new
  `@agent-native/inspector` shipped today.
