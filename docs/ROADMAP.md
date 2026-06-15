# Roadmap & Backlog

> **This is the checklist the planner points the agent at.** To start work, say something like
> "do R1" or "do the flagship demo." **R-numbers are stable IDs, not a sequence — priority is the
> tier.** When an item ships, move it to **Done** at the bottom with the commit SHA. Strategic
> *why* lives in [strategy/](strategy/); feature detail in [PRODUCT_GAPS.md](PRODUCT_GAPS.md).
> Direction is set by decision [D-009](DECISIONS.md): the governed **in-process** action layer
> for no-API local capabilities (desktop/in-process = mechanism, governance = moat).

Legend: ⬜ not started · 🟡 in progress · ✅ done · ⭐ flagship

---

## P0 — Critical path to the YC demo (the wedge; do in order)

These four, in sequence, produce the flagship POS video that *is* the pitch. Nothing else
matters until this path is walked.

- ✅ **R1 · Governed MCP-server mode** — shipped (`d1e28e6`). See **Done** below.
- ⬜ **R3 · Sidecar host + Electron/Node binding.** Run `agent-native-host` as a standalone
  process the app's main process talks to over stdio; add a JS/TS binding so **Electron** and
  plain Node apps host the runtime, not just Tauri. On the critical path because the existing app
  we bolt onto (incl. the POS) may not be Tauri. Governance in a process the renderer can't tamper
  with is a feature. (Also closes the §2 "separate-process host" gap.)
- ⬜ **R4 · `wrapAction` + codemod.** `wrapAction(existingFn, { permissions })` to wrap handlers
  an app already has + an optional codemod that scans exported functions and scaffolds wrappers.
  This is what makes the <50-LOC bolt-on in R5 real. Framing: **augment, not migrate.**
- ⬜ ⭐ **R5 · THE FLAGSHIP DEMO (the YC application video).** Bolt verb onto a **real existing app
  WITHOUT rewriting it** — governed MCP access in **<50 lines**. Before/after video: stock local
  app → an on-device agent driving it through typed, permissioned, audited actions. **Strongest
  candidate: the planner's own no-API POS app** — purest proof this only works in-process and that
  governance is intrinsic (a local notes/finance clone is the fallback). **This single demo is the
  entire pitch.** Depends on R1 + R3 + R4.

## P1 — Monetize + distribute (after the wedge is proven)

- ⬜ **R6 · Governance dashboard (the paid surface).** Cross-app/agent audit viewer, in-app policy
  editor, approval routing for multiple pending approvals, budget controls. Open-core
  monetization; leans on the audit/budget/approval/policy work already shipped.
- ⬜ **R2 · Publish packages.** `@agent-native/core` + `@agent-native/inspector` → npm;
  `agent-native-host` → crates.io; `create-agent-app` last. **Runbook:
  [PUBLISHING.md](PUBLISHING.md).** Planner runs the commands (irreversible, needs credentials —
  [D-004](DECISIONS.md)). After publish, swap the scaffolder template to the published crate
  (PUBLISHING.md step 5). Needed for external adoption + before launch — but *after* the demo
  proves the thesis.

## P2 — Compliance & polish

- ⬜ **R7 · Compliance-evidence export.** Audit log → SOC 2 / ISO 42001 / EU AI Act artifacts.
  The willingness-to-pay hook (EU AI Act enforcement opens Aug 2026).
- ⬜ **R8 · Agent + user identity per action.** Who (which agent / which user) took each action;
  ties into the governance category.
- ⬜ **R13 · On-device guarantee.** Local-model-first posture (ollama / claude-cli) + a
  "no network egress" assertion the audit log can attest — the regulated-app selling point that
  *nothing leaves the device*. Thesis-critical for the regulated ICP.
- ⬜ **R9 · Resume-ability UI + persist approval queue.** A reference-app button to trigger
  `resume: true`; persist pending approvals so a run interrupted mid-approval re-issues the
  guarded action instead of skipping it.
- ⬜ **R10 · OpenAI inference backend + retry/backoff + frontier-escalation fallback.**
- ⬜ **R11 · Audit-receipt tamper tests** + finish the budget (api-calls/hr cap).

## Launch (after the wedge + publish; gated on planner's go)

- ⬜ **R12 · Launch.** Full plan in **[LAUNCH.md](LAUNCH.md)** — pre-launch checklist (GIF in
  README, fresh-machine smoke, repo public, CI green), Show HN title + opening comment + objection
  answers, Twitter thread, timing, distribution order.

## Explicitly deprioritized / not doing (per research)

- ❌ Web framework bindings (Vue/Svelte for web) — don't fight WebMCP.
- ❌ Mobile (Flutter/SwiftUI/Compose) — premature.
- ❌ Scaffolder polish beyond demo quality — it's the demo, not the product.
- ⏸️ **Rename off "agent-native"** — Builder.io owns the term; decide a new public name before
  launch ("verb" as the product name is fine).

---

## Done (newest first)

- ✅ **R1 · Governed MCP-server mode** — `d1e28e6` (3 commits: `20305d1` protocol types,
  `f56f1b4` governed dispatch, `d1e28e6` stdio server + `verb-mcp` binary). New `mcp` module
  in `agent-native-host`: an stdio JSON-RPC server (`initialize` / `tools/list` / `tools/call`)
  that routes every external-agent call through the same policy → approval → budget →
  signed-audit gates the in-process host enforces. Execution + approval are traits
  (`ActionExecutor`, `ApprovalGate`) so the same governance serves Tauri, a sidecar (R3), or a
  CLI; `ProcessExecutor` is the dependency-free bolt-on. The thin `verb-mcp` binary takes
  `--tools` + `--policy` + `--exec` + `--approval deny|tty|auto`. 21 unit tests + verified end
  to end against the real binary (allowed action signed, guarded action held, unregistered tool
  refused). **Turns verb from a rewrite into a bolt-on.**
- ✅ CI workflow + CONTRIBUTING + issue templates — `f191618`
- ✅ Step-through (host pause + inspector StepGate) — `6070302`
- ✅ Policy linting at startup — `255e6ce`
- ✅ Resume-ability (run_id + last_resumable_run) — `5401c6d`
- ✅ Inspector v0.3.0 (replay + StepGate) & scaffolder README — `0fb85fc`
- ✅ Action composition + ESM `.js` fix — `edfb898`
- ✅ Second reference app (task-manager) — `cfb797c`
- ✅ Extract Rust host into `agent-native-host` crate — `db30153`
- ✅ `create-agent-app` scaffolder — `4b751b0`
