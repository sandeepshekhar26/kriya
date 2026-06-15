# Product Gaps — feature-completion tracker

> Living document tracking **feature state** (done / partial / missing). For *what to build
> next* see [ROADMAP.md](ROADMAP.md); for *why / strategy* see [strategy/](strategy/); for
> *decisions* see [DECISIONS.md](DECISIONS.md).

> **⚠️ Strategic direction (planner-affirmed 2026-06-15, decision [D-009](DECISIONS.md)).** The
> generic "agent-native framework" insight is commoditized (WebMCP, MCP Apps, Builder.io — see
> [strategy/market-landscape-2026.md](strategy/market-landscape-2026.md)). The bet is now the
> governed **in-process** action layer for capabilities that live only inside a running local app
> (no API to wrap, local/private data) — desktop/in-process is the *mechanism*, governance the
> *moat*, **one bet, not two**. See
> [strategy/governed-local-first-wedge.md](strategy/governed-local-first-wedge.md).
> **Critical path (all P0):** `R1` governed MCP-server mode → `R3` sidecar/Electron host → `R4`
> `wrapAction` bolt-on → `R5` the POS flagship demo — *not* more breadth on the "build a new Tauri
> app" path. §4 (web/transport) and §6 (mobile/web bindings) are deliberately deprioritized. Live
> priority order: [ROADMAP.md](ROADMAP.md).

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
  **The local/on-device path (ollama, claude-cli) is now thesis-critical** — regulated apps
  can't use cloud agents; the "nothing leaves the device" guarantee is tracked as **R13**.
- 🟡 **Persistent memory**: episodic log persisted to SQLite (every action across runs,
  newest-first query via `agent_memory_recent`, count surfaced at run start) AND recalled
  into the LLM prompt as a MEMORY section, so prior runs inform decisions. State snapshots
  + vector recall (embeddings) still ⬜.
- 🟡 Resume-ability — durable run reconstruction is wired end-to-end. Each
  run mints a stable `run_id` (UUID at `run_task` entry); every audit episode
  in SQLite memory now carries `run_id` + `goal`. `AgentStartRequest.resume:
  true` makes the host call `memory.last_resumable_run(goal)`, reseed the
  in-memory `history` with the prior run's completed steps, and continue from
  there. Backwards-compatible: old episodes (pre-migration) get empty
  `run_id`/`goal` via ALTER TABLE + DEFAULT, indexed on `(goal, id)` for fast
  lookup. 5 new memory tests cover round-trip, goal isolation, newest-run
  picking, and the limit query (9/9 host tests pass). What's still ⬜: a
  reference-app UI button to trigger `resume: true`, and resume mid-approval
  (the approval queue isn't persisted yet, so a guarded action interrupted
  mid-approval isn't re-issued — it's just skipped from history reseeding).
- ⬜ Retry/backoff, graceful "this is too hard → escalate to frontier model" fallback
- ⬜ Multi-agent orchestration (concurrent agents per app)
- ✅ Separate-process agent host (don't share the app's main thread) — **shipped as the R3
  sidecar host** (`8b3a8c2`). `run_task` is now transport-agnostic behind a `HostSink` trait;
  the `verb-host` binary runs the loop as a standalone process over stdio (NDJSON), with
  governance in a process the renderer can't tamper with. Latency profiling (<500ms p50) still
  ⬜ — the current `ProcessExecutor`/sidecar is correctness-first, not yet tuned.

## 3. Permissions, approval & audit
- ✅ YAML policy + deny-by-default + `RequiresApproval` decision
- ✅ **Human-approval queue**: host pauses on a guarded action, emits `agent://approval`,
  blocks on a per-step channel (5-min timeout = deny), frontend modal approve/deny, resumes
- ⬜ Approval **queue UI** for multiple pending approvals + per-action policy editor in-app
- 🟡 Budgets/rate-limits — actions/minute sliding-window cap enforced (host stops the run);
  api-calls/hr still ⬜
- 🟡 Ed25519 signed receipts → JSONL — works; **verifier** CLI delegated (see prompt) + tamper tests ⬜
- ✅ Policy linting — `Policy::warnings()` reports on `*` rules that allow
  everything, destructive-named patterns (delete/remove/destroy/drop/purge/wipe)
  without `require_approval`, missing explicit catch-all, and missing
  `budget.max_actions_per_minute`. Each concern is logged as `warn` at
  `run_task` startup so devs see it the first time they hit Run. 4 new unit
  tests cover wildcard allow, destructive-named without approval, missing
  budget/wildcard, and a clean policy producing zero warnings.

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
  ✅ Live in-host pause-between-steps shipped: `AgentStartRequest.stepMode:
  true` makes the host pause before *each* decision and emit an
  `agent://await_step` event with the upcoming step number + last action +
  last outcome. A new `<StepGate>` component in the inspector renders the
  pause card with `step →` / `stop` buttons (Space/Enter advances, Esc
  stops, both ignore typing in inputs). Both apps wire the
  `agent_step_advance` Tauri command. note-app surfaces a "step mode"
  checkbox in the header. 5-minute timeout treats a wandering-off dev as a
  stop.
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
- ✅ **Governed MCP-server mode** (`R1`, **P0 — critical path**, `d1e28e6`) — new `mcp` module
  in `agent-native-host` exposes registered actions as an stdio MCP server (`initialize` /
  `tools/list` / `tools/call`) so an external agent (Claude Desktop, Cursor) drives the app,
  with every call routed **through** the policy → approval → budget → signed-audit gates
  (governed routing, not raw tool exposure). `Governor` reuses the exact gate sequence from
  `agent::host`; blocked calls never reach the executor and aren't signed (receipts attest only
  to what ran). Execution + approval are traits (`ActionExecutor` / `ApprovalGate`:
  `DenyApproval` default, `AutoApprove`, `TtyApproval` prompting on `/dev/tty`), so the same
  governance serves Tauri, the R3 sidecar, or a CLI; `ProcessExecutor` shells out per call as
  the dependency-free bolt-on. Thin `verb-mcp` binary (`--tools` / `--policy` / `--exec` /
  `--approval`). 21 unit tests + verified end to end. Turns verb from a rewrite into a bolt-on.
- ✅ **Sidecar host + Electron/Node binding** (`R3`, **P0 — critical path**, `8b3a8c2`) — the
  agent loop is decoupled from Tauri behind a `HostSink` trait (`TauriSink` is one impl). The
  `verb-host` binary runs `agent-native-host` as a standalone process over stdio (NDJSON
  protocol mirroring the Tauri event/command names), and `@agent-native/sidecar` (`SidecarHost`
  + `runTask`) binds it from Electron and plain Node. A generic `ScriptedPlanner` backend
  (`--script`) enables zero-config deterministic runs. Governance lives in a process the
  renderer can't tamper with. The cross-shell decoupling that lets verb bolt onto an existing
  app whatever its shell. Verified end to end (Node → Rust → Node round-trip).
- ⬜ **`wrapAction` + codemod** (`R4`, **P0 — critical path**) — wrap an existing app's handlers
  without a rewrite (augment, not migrate). The bolt-on path that makes the <50-LOC R5 demo real.
- ❌ Mobile (Flutter, SwiftUI, Jetpack Compose) — **deprioritized** (premature).
- ❌ Web framework bindings (Vue/Svelte for web) — **not doing** (don't fight WebMCP).
- 🟡 Reference apps beyond notes: **task manager ✅** (apps/task-manager — six
  typed actions including two approval-gated; its own `TaskPlanner` plugged into
  the shared `agent-native-host` crate via `select_backend_with_default`).
  Next reference target is the **flagship bolt-on demo** (`R5`), not another from-scratch app.

## 7. Product / business
- ⬜ **Governance dashboard** (`R6`, **P1**, the paid surface) — cross-app/agent audit viewer,
  in-app policy editor, approval routing, budget controls. Open-core monetization; builds on
  the audit/budget/approval/policy work already shipped.
- ⬜ **Compliance-evidence export** (`R7`, **P2**) — audit log → SOC 2 / ISO 42001 / EU AI Act
  artifacts. Willingness-to-pay hook (EU AI Act enforcement opens Aug 2026).
- ⬜ **Agent + user identity per action** (`R8`, **P2**).
- ⬜ Hosted agent cloud / integrations marketplace — later phases.

## Near-term focus

Superseded by the live, prioritized [ROADMAP.md](ROADMAP.md). The **critical path to the YC
demo is R1 → R3 → R4 → R5** (all P0, in order); **R5** (the POS flagship demo) is the
YC-defining artifact. Publishing (R2) and the governance dashboard (R6) come *after* the wedge
is proven. §4 (web/transport) and §6 (mobile/web bindings) stay deprioritized under decision
[D-009](DECISIONS.md).
