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
> **Critical path (all P0) — now COMPLETE (2026-06-15):** `R1` governed MCP-server mode → `R3`
> sidecar/Electron host → `R4` `wrapAction` bolt-on → `R5` the flagship demo (**Actual Budget**,
> decision [D-010](DECISIONS.md), not the earlier POS candidate) — all shipped (see §6). What
> remains is recording the R5 video + P1 (monetize/distribute) and P2 (compliance/polish). §4
> (web/transport) and §6 (mobile/web bindings) stay deliberately deprioritized. Live priority
> order: [ROADMAP.md](ROADMAP.md).

Legend: ✅ done · 🟡 partial / proof-only · ⬜ not started

## 1. Core SDK (`kriya-core`)
- ✅ `registerAction`, typed schemas, MCP-style `getToolSchemas()`
- ✅ Runtime param validation (types/enum/array/required) + 13-test suite
- ✅ Standards-compliant **JSON Schema** export (`packages/core/src/jsonschema.ts` + test suite)
  so any MCP client consumes schemas directly
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
  (compile-verified; live runs pending a local model / API key). **OpenAI cloud backend
  deliberately deferred** (R10 split) — off the on-device wedge (D-009), demand-pulled/demoted.
  **The local/on-device path (ollama, claude-cli) is now thesis-critical** — regulated apps
  can't use cloud agents. ✅ **On-device guarantee shipped (R13, `64b340f`)**: backends declare a
  `NetworkProfile`, and a sealed policy (`on_device: true`) refuses an egressing backend +
  signs an offline-verifiable `kriya.attestation.on_device` receipt — "nothing leaves the device,"
  attested.
- 🟡 **Persistent memory**: episodic log persisted to SQLite (every action across runs,
  newest-first query via `agent_memory_recent`, count surfaced at run start) AND recalled
  into the LLM prompt as a MEMORY section, so prior runs inform decisions. State snapshots
  + vector recall (embeddings) still ⬜.
- ✅ Resume-ability — durable run reconstruction is wired end-to-end. Each
  run mints a stable `run_id` (UUID at `run_task` entry); every audit episode
  in SQLite memory now carries `run_id` + `goal`. `AgentStartRequest.resume:
  true` makes the host call `memory.last_resumable_run(goal)`, reseed the
  in-memory `history` with the prior run's completed steps, and continue from
  there. Backwards-compatible: old episodes (pre-migration) get empty
  `run_id`/`goal` via ALTER TABLE + DEFAULT, indexed on `(goal, id)` for fast
  lookup. ✅ **R9 closed the two gaps (`4873812` + `fede962` + `1a37038`):** a
  durable `pending_approvals` store records a guarded action when the host holds
  it for a human and resolves it once they decide — so a run interrupted
  *mid-approval* leaves the row unresolved, and on resume the host drains the
  prior run's unresolved approvals, re-checks policy, and **re-issues** (re-requests
  + re-dispatches) each held action instead of skipping it. note-app has a "Resume
  last task" button. 64 crate tests (incl. a seeded-crash resume test); clippy clean.
- ✅ **Retry/backoff + graceful escalation (R10 reliability half, `feat/r10-reliability`).** A new
  `inference::retry` unit wraps the host loop's `backend.next_step()` (was a bare `?`) in **bounded
  retry with exponential backoff**, so a *transient* backend error (network blip, rate-limit, parse
  hiccup) is retried instead of failing the whole run; retry count/backoff are configurable via an
  optional `retry:` policy section (default 3 retries / 250ms→5s). Past the budget the host **escalates
  by ending the run gracefully** — a descriptive `AgentDone` + error log, never a hang or panic (the
  degrade-cleanly behavior a regulated workstation host needs). **Deterministic/scripted backends never
  error → zero behavior change.** 8 new tests (5 retry-unit + 1 policy-parse + 2 end-to-end host);
  88 crate tests, clippy clean. ⬜ **The OpenAI cloud inference backend was deliberately NOT built** —
  it is off the on-device wedge (D-009) and explicitly demand-pulled/demoted; the reliability layer is
  backend-agnostic and helps the on-device backends (Ollama, claude-cli) today. "Escalate to a frontier
  model" remains a future, design-partner-gated extension on top of this clean-escalation seam.
- 🟡 Multi-agent orchestration (concurrent agents per app). ✅ **Shared-governance core shipped
  (`6dbe7ec`)**: a `GovernedApp` shares one **budget** (and policy, signer, audit log, channel maps)
  across every agent it runs, so the per-minute/api rate caps are enforced over the *aggregate*, not
  per-agent — `run_task` stays single-agent (own budget), fully additive. The budget is now a
  `SharedBudget` (`Arc<Mutex<BudgetTracker>>`); spawn each `GovernedApp::run` on its own thread for
  concurrency (steps keyed by unique ids → shared maps route correctly). Tested: a 2-actions/min app
  throttles a second agent once the first exhausts the shared budget. Still ⬜: a *scheduler*
  (coordinating/prioritising agents) — governance first, orchestration logic later.
- ✅ Separate-process agent host (don't share the app's main thread) — **shipped as the R3
  sidecar host** (`8b3a8c2`). `run_task` is now transport-agnostic behind a `HostSink` trait;
  the `kriya-host` binary runs the loop as a standalone process over stdio (NDJSON), with
  governance in a process the renderer can't tamper with. Latency profiling (<500ms p50) still
  ⬜ — the current `ProcessExecutor`/sidecar is correctness-first, not yet tuned.
  ✅ **Tauri⇄Electron parity closed (P0.5 R14–R16, `93c5a67`).** The `runTask` helper now answers
  the `awaitStep` message kind (step-mode no longer hangs); durable memory recall is exposed over the
  sidecar protocol (`memory_recent`/`memory` + `SidecarHost.recentMemory()`), the mirror of Tauri's
  `agent_memory_recent`; and a committed integration test plus `examples/node-sidecar-host/` drive the
  **real** `kriya-host` binary through action + held/granted approval + memory recall. The "works in
  Electron/Node" claim is now demoable, not just asserted. Still ⬜: latency profiling (<500 ms p50).

## 3. Permissions, approval & audit
- ✅ YAML policy + deny-by-default + `RequiresApproval` decision
- ✅ **Human-approval queue**: host pauses on a guarded action, emits `agent://approval`,
  blocks on a per-step channel (5-min timeout = deny), frontend modal approve/deny, resumes
- ✅ **MCP-mode approval gates** — for external agents driving over `kriya-mcp`, approval is a
  swappable `ApprovalGate`: `DenyApproval` (safe default), `AutoApprove` (trusted/testing),
  `TtyApproval` (prompt on `/dev/tty`), and **`GuiApproval` (`--approval gui`, native macOS
  dialog via `osascript`)**. The GUI gate exists because `tty` **deadlocks** when kriya-mcp runs
  as a child of a TUI host (e.g. Claude Code) that owns the terminal in raw mode and consumes the
  operator's keystrokes; the dialog is drawn by the window server, out-of-band from any tty, so
  it works under the TUI — the dependable on-camera human-in-the-loop beat for the R5 demo.
  Deny is default+cancel, `giving up after 300` bounds the wait, any failure denies (safe-by-
  default preserved). macOS-only (cfg-gated); `--approval gui` elsewhere exits with a clear
  message. Cross-platform GUI gate (Linux/Windows) still ⬜.
- ⬜ Approval **queue UI** for multiple pending approvals + per-action policy editor in-app
- ✅ Budgets/rate-limits — **two independent sliding-window caps (R11, `44637f5`)**: actions/minute
  bounds bursts against the app (safety), and `budget.max_api_calls_per_hour` bounds inference/API
  calls (cost) so a loop can't run up unbounded model spend even when it dispatches few/no actions.
  The host stops the run on either; both share a reusable `SlidingWindow`.
- ✅ Ed25519 signed receipts → JSONL — works; offline **verifier** CLI ✅ (`tools/verify-receipts/`).
  ✅ **Per-action identity (R8, `ccdb444`)**: an optional `actor` (agent + operator) signed *inside*
  the receipt, threaded through host + MCP governor (`--actor`) + offline verifier + console.
  ✅ **Comprehensive tamper tests (R11, `e2ae449`)**: rewriting any signed field (action_id /
  success / step_id / ts_ms / params / actor), fabricating an actor after signing, a forged
  signature, a substituted public key, or malformed hex all fail to verify (the spine of
  [SECURITY.md](SECURITY.md)).
  ✅ **Durable identity + tamper-evident chaining (R20, `26f750c` + `2163b10`)**: a persisted host
  key (`--signing-key`, stable across runs) **and** `prev_hash` hash-chaining, so whole-receipt
  deletion/truncation/reorder is caught by `verify-receipts` (the on-device *complete-log* guarantee
  a cloud audit sidecar can't produce). Residual: HSM/keychain key custody + external anchoring.
  ✅ **Deterministic canonical bytes (R21, `b51370f`)**: explicit recursive `params` key-sort before
  signing, mirrored in the verifier — reproducible regardless of serde_json `preserve_order`.
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
- ✅ `create-kriya-app` scaffolder — `npm create kriya-app@latest my-app` produces a
  working counter-app starter (Tauri 2 + Rust host + React + SDK + all safety infra,
  locked deps). Verified end-to-end: TS compiles, `cargo check --locked` passes.
  Generated apps now ship a `README.md` with the develop / build commands, a
  per-app file map ("where to add features"), and a documented macOS gotcha —
  `target/release/bundle/macos/<app>.app` shadows `tauri dev` via LaunchServices
  after a prior release build, with a one-liner to kill it.
- ✅ Rich dev dashboard/inspector — extracted into `kriya-inspector`
  (workspace package, v0.2.0). Filterable log (toggle levels + full-text search),
  per-step expand, one-click JSONL export of the current run, and a `MemoryPanel`
  that reads durable past runs from the host's SQLite memory via the
  `agent_memory_recent` Tauri command. **Step-through replay**: clicking an
  episode in MemoryPanel opens its detail, then Prev/Next buttons (or ←/→ keys,
  Esc to close) walk through neighbouring episodes one at a time — keyboard nav
  is suppressed while typing in inputs so the inspector's filter box still works.
  Both reference apps consume the package; styles ship via
  `kriya-inspector/styles.css` and are themable through CSS variables.
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
- ✅ Extract Rust host into a shared crate (`kriya`). `apps/note-app`
  now path-depends on it and is ~110 lines of glue around `run_task` +
  `select_backend_with_default`. Scaffolder template still ships an embedded
  copy — the swap requires either (a) publishing the crate to crates.io and
  using `kriya = "0.1"`, or (b) restructuring the repo as a Cargo
  workspace so the template can use a git dep. See follow-up below.

## 6. Breadth / ecosystem (the copy-resistance)
- ✅ **Governed MCP-server mode** (`R1`, **P0 — critical path**, `d1e28e6`) — new `mcp` module
  in `kriya` exposes registered actions as an stdio MCP server (`initialize` /
  `tools/list` / `tools/call`) so an external agent (Claude Desktop, Cursor) drives the app,
  with every call routed **through** the policy → approval → budget → signed-audit gates
  (governed routing, not raw tool exposure). `Governor` reuses the exact gate sequence from
  `agent::host`; blocked calls never reach the executor and aren't signed (receipts attest only
  to what ran). Execution + approval are traits (`ActionExecutor` / `ApprovalGate`:
  `DenyApproval` default, `AutoApprove`, `TtyApproval` prompting on `/dev/tty`), so the same
  governance serves Tauri, the R3 sidecar, or a CLI; `ProcessExecutor` shells out per call as
  the dependency-free bolt-on. Thin `kriya-mcp` binary (`--tools` / `--policy` / `--exec` /
  `--approval`, the last now `deny|tty|gui|auto` — see §3 for the `gui` native-dialog gate). 21
  unit tests + verified end to end. Turns kriya from a rewrite into a bolt-on.
- ✅ **Sidecar host + Electron/Node binding** (`R3`, **P0 — critical path**, `8b3a8c2`) — the
  agent loop is decoupled from Tauri behind a `HostSink` trait (`TauriSink` is one impl). The
  `kriya-host` binary runs `kriya` as a standalone process over stdio (NDJSON
  protocol mirroring the Tauri event/command names), and `kriya-sidecar` (`SidecarHost`
  + `runTask`) binds it from Electron and plain Node. A generic `ScriptedPlanner` backend
  (`--script`) enables zero-config deterministic runs. Governance lives in a process the
  renderer can't tamper with. The cross-shell decoupling that lets kriya bolt onto an existing
  app whatever its shell. Verified end to end (Node → Rust → Node round-trip).
- ✅ **`wrapAction` + codemod** (`R4`, **P0 — critical path**, `0afc8ca`) — `wrapAction(fn,
  { id, description, parameters, mapParams, mapResult })` in `kriya-core` adapts a
  function an app already has (positional args, plain return, throws) into a registered action,
  normalizing the return/throw into an `ActionResult` and running the full registry path
  (validation, audit, composition). The `kriya wrap <file>` codemod scans exported
  functions via the TypeScript compiler API, infers parameter schemas + required-ness + JSDoc
  descriptions, and scaffolds the `wrapAction(...)` module. Augment, not migrate — the bolt-on
  path that makes the <50-LOC R5 demo real. 16 tests; verified wrap→register→dump round-trip.
- ✅ **Python SDK binding** (`R17`, **P3**, `fae5909` + `5f1b67a`) — [`bindings/python/`](../bindings/python/):
  one `pip install kriya` package mirroring `kriya-core` (`register_action`/`wrap_action`, schema,
  validation, composition, draft-clean JSON-Schema export) **and** `kriya-sidecar` (the `Host` stdio
  NDJSON driver + `run_task` + `recent_memory()`). Zero runtime deps; 51 unit tests + an opt-in
  integration test against the real `kriya-host` (action + held/granted approval + memory recall) + a
  runnable example. The first non-JS language on the in-process layer — *a second binding, not a new
  host* — and the `bindings/` convention for **R18 (.NET)** / **R19 (JVM)** (decision [D-012]). Async
  handlers are the only follow-up (handlers run on the host reader thread today).
- 🟡 **.NET SDK binding** (`R18`, **P3**, `fa61b0a` + `9380299`) — [`bindings/dotnet/`](../bindings/dotnet/):
  the C# port (`Registry` RegisterAction/WrapAction + validation + composition + MCP/JSON-Schema;
  `Host` spawn-kriya-host/RunTask/RecentMemoryAsync; `P.*` schemas), net8.0 (LTS, regulated
  Windows-desktop ICP) + net10.0, NuGet metadata staged. 25 tests incl. a real-`kriya-host`
  integration test + a runnable console example (macOS-verified). Built without a design partner.
  Remaining: the third-party flagship bolt-on (target research disqualified Mnemo — it went
  MCP-native; MyMoney.Net is the Windows finance fallback). A WPF/WinForms/Avalonia GUI sample is the
  other follow-up.
- ❌ Mobile (Flutter, SwiftUI, Jetpack Compose) — **deprioritized** (premature).
- ❌ Web framework bindings (Vue/Svelte for web) — **not doing** (don't fight WebMCP).
- 🟡 Reference apps beyond notes: **task manager ✅** (apps/task-manager — six
  typed actions including two approval-gated; its own `TaskPlanner` plugged into
  the shared `kriya` crate via `select_backend_with_default`).
- ✅ **Flagship bolt-on demo** (`R5`, `24ed278`) — [`examples/actual-budget-bolt-on/`](../examples/actual-budget-bolt-on/):
  governed agent access to Actual Budget (real local-first no-HTTP-API finance app) in a ~37-line
  `wrapAction` file, driven by an external agent over `kriya-mcp` (persistent-handler executor),
  policy gating delete/close behind human approval. The whole wedge proved in one runnable demo.

## 7. Product / business
- 🟡 **Governance dashboard** (`R6`, **P1**, the paid surface) — in progress in 🔒 private
  `kriya-console`: cross-app/agent audit viewer, org policy editor, multi-approval routing, live
  budget controls, and per-user/agent + RBAC dashboards built; the remaining surface is hosted-tier
  (SSO/OIDC sign-in). Open-core monetization; builds on the audit/budget/approval/policy primitives
  this repo ships. (D-011 keeps the build private.)
- ⬜ **Compliance-evidence export** (`R7`, **P2**) — audit log → SOC 2 / ISO 42001 / EU AI Act
  artifacts. Willingness-to-pay hook (EU AI Act enforcement opens Aug 2026).
- ⬜ **Agent + user identity per action** (`R8`, **P2**).
- ⬜ Hosted agent cloud / integrations marketplace — later phases.

## Near-term focus

Superseded by the live, prioritized [ROADMAP.md](ROADMAP.md). The **P0 critical path
(R1 → R3 → R4 → R5) is complete** — the wedge is proven in code (R5 = the Actual Budget bolt-on,
`examples/actual-budget-bolt-on/`). **R2 publish is done** (all npm packages + the `kriya` crate
live since 2026-06-15; P0.5 republish at 0.0.2/0.1.1 staged) and **P0.5 cross-shell parity is
shipped** (R14–R16, Tauri⇄Electron). Immediate next: **record the R5 before/after video** (the
YC-defining artifact), then the rest of P1 — governance dashboard (R6, the paid surface).
P2 (compliance export R7, identity R8, on-device guarantee R13) follows. §4 (web/transport) and
§6 (mobile/web bindings) stay deprioritized under decision [D-009](DECISIONS.md).
