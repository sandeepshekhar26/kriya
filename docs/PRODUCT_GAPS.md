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
- ⬜ Action **composition** (a parent action calling child actions)
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
- 🟡 Rich dev dashboard/inspector — today an in-app log panel; step-through, replay
  sessions, export traces still ⬜
- 🟡 CLI to dump registered actions as JSON — added in 1.1
- ⬜ Templates, examples gallery, "build an agent app in <2 hours" tutorial
- ⬜ OpenTelemetry traces; CI eval gate ("does my app still work with agents?")
- ⬜ Extract Rust host into a shared crate (`agent-native-host`) so the scaffolder
  template and `apps/note-app` consume one source of truth instead of duplicating
  ~1,400 lines of Rust. Today's template ships a copy of the host code; the next
  pass replaces that copy with a thin `Cargo.toml` dependency.

## 6. Breadth / ecosystem (the copy-resistance)
- ⬜ Electron binding (`@agent-native/electron`) — largest JS audience
- ⬜ Mobile (Flutter, SwiftUI, Jetpack Compose)
- ⬜ Agent-native component library ("shadcn for agent-operable UI")
- ⬜ Agent/skills registry; MCP-server generation (reverse-MCP)
- ⬜ Reference apps beyond notes: task manager, spreadsheet, personal CRM

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
- **Extract the Rust host into a shared crate.** Today the scaffolder ships ~1,400
  lines of duplicated host code. Pulling them into `crates/agent-native-host` (a
  Cargo workspace member, eventually published to crates.io) eliminates the drift
  risk *before* a third party clones the scaffold. This is the prerequisite for
  trustworthy publishing — without it, every host bug fix is a two-place change.
- **Then: second reference app (task manager).** A second app that consumes both
  `@agent-native/core` and the new `agent-native-host` crate is the proof the
  framework is generalized, not note-app-shaped. It also stress-tests the
  scaffolder by being something *not* generated from it.
- After those: the dev inspector (step-through + replay) is the next moat piece.
