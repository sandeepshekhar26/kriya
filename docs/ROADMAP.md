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
- ✅ **R3 · Sidecar host + Electron/Node binding** — shipped (`8b3a8c2`). See **Done** below.
- ✅ **R4 · `wrapAction` + codemod** — shipped (`0afc8ca`). See **Done** below.
- ✅ ⭐ **R5 · THE FLAGSHIP DEMO** — shipped (`24ed278`). See **Done** below. The wedge's
  critical path (R1 → R3 → R4 → R5) is complete; what remains is the **video itself** + P1
  (monetize/distribute) and P2 (compliance/polish).

## P0.5 — Cross-shell parity (harden the Electron/Node leg)

> **Why a 0.5 tier (added 2026-06-16).** The thesis (D-009) rests on the in-process layer working
> in Tauri **and** Electron **and** plain Node. Of the three transports, the **embedded sidecar
> (R3)** was the least proven: its convenience helper dropped a message kind, it had no memory-recall
> path, and the only test driving the real binary was CI-skipped. Those gaps actually shipped in the
> **already-published `kriya-sidecar` 0.0.1** (R2 ran 2026-06-15) — so P0.5 closes them and stages a
> **0.0.2** republish, before the dashboard (R6/P1) or any external adoption builds further on the
> sidecar. All additive, none touching the demoable Tauri apps or the R5 MCP bolt-on.

- ✅ **R14 · `runTask` step-mode parity (kriya-sidecar)** — shipped (`93c5a67`). Added an optional
  `onStep` handler so the convenience helper answers `awaitStep` (auto-advancing when none is given,
  so it never hangs). All five host→app message kinds now flow through the helper, not just the
  low-level `SidecarHost`.
- ✅ **R15 · Memory recall over the sidecar protocol** — shipped (`93c5a67`). Inbound `memory_recent`
  → outbound `memory` handled directly in `run_sidecar` (no `HostSink` trait change — zero impact on
  Tauri/MCP), plus `SidecarHost.recentMemory(limit)` (requestId-correlated, with a timeout) and an
  exported `Episode` type in `kriya-core`. Electron now reads the same episodic memory Tauri does.
- ✅ **R16 · Committed end-to-end sidecar proof** — shipped (`93c5a67`). Renamed the stale
  `VERB_HOST_BIN` env var → `KRIYA_HOST_BIN`; strengthened the opt-in integration test to drive the
  **real** `kriya-host` binary through action + held/granted approval + memory recall; added a
  runnable `examples/node-sidecar-host/` that hosts the governed runtime from plain Node (byte-
  identical to an Electron main process) and prints the governance trail. The "works in Electron/
  Node" claim is now demoable, verified against the real binary.

## P1 — Monetize + distribute (after the wedge is proven)

> **Repo split (D-011 / [LICENSING.md](LICENSING.md)):** R6 → 🔒 private **`kriya-console`** (Proprietary/ARR). R2 = 🌐 public (done).

- ⬜ **R6 · Governance dashboard (the paid surface).** Cross-app/agent audit viewer, in-app policy
  editor, approval routing for multiple pending approvals, budget controls. Open-core
  monetization; leans on the audit/budget/approval/policy work already shipped.
- ✅ **R2 · Publish packages** — **done 2026-06-15** (planner ran it). All four npm packages are
  live (`kriya-core` 0.0.1, `kriya-sidecar` 0.0.1, `kriya-inspector` 0.3.0, `create-kriya-app`
  0.2.0) and the `kriya` crate is on crates.io (0.1.0); the scaffolder template already path-swapped
  to the published crate (`kriya = "0.1"`). **Republish pending for the P0.5 API:** today's R14–R16
  added `recentMemory()`/`Episode` to `kriya-core` + `kriya-sidecar` (npm immutable, so a 0.0.2 bump
  is staged) and a `memory_recent` handler to the `kriya` crate (0.1.1 staged) — versions are bumped
  in-repo; the `npm publish`/`cargo publish` commands are the planner's ([D-004](DECISIONS.md),
  [PUBLISHING.md](PUBLISHING.md) → "Republishing for P0.5"). Optional — only external installers need it.

## P2 — Compliance & polish

> **Repo split (D-011):** R7 → 🔒 `kriya-console`; **R8 splits** (signed-receipt `actor` field 🌐 public, identity-mgmt 🔒 private); R13 / R9 / R10 / R11 → 🌐 public.

- ✅ **R7 · Compliance-evidence export** — shipped in 🔒 `kriya-console` (`a7e9d68`): audit log →
  SOC 2 / ISO 42001 / EU AI Act evidence bundle (integrity + R8 attribution + R13 on-device +
  control mapping), Markdown/JSON export. The willingness-to-pay hook (EU AI Act enforcement opens
  Aug 2026).
- 🟡 **R8 · Agent + user identity per action.** Public **signed-receipt `actor` field** shipped
  (`ccdb444` runtime + `57784fb` console) — agent + operator stamped *inside* the signed bytes
  (tamper-evident), threaded through the in-process host, MCP governor (`--actor`), the offline
  verifier, and the console audit table. Still ⬜: the 🔒 identity-**management** half (SSO/OIDC,
  RBAC, per-user dashboards).
- ✅ **R13 · On-device guarantee** — shipped (`64b340f`). A sealed policy (`on_device: true`) makes
  the in-process host refuse an egressing inference backend and sign a `kriya.attestation.on_device`
  receipt (verifiable offline). Backends declare an honest `NetworkProfile` (scripted=none,
  Ollama=localhost-only/remote, claude-cli + Anthropic=remote). The regulated-ICP "nothing leaves
  the device" posture, attested.
- ⬜ **R9 · Resume-ability UI + persist approval queue.** A reference-app button to trigger
  `resume: true`; persist pending approvals so a run interrupted mid-approval re-issues the
  guarded action instead of skipping it.
- ⬜ **R10 · OpenAI inference backend + retry/backoff + frontier-escalation fallback.**
- ⬜ **R11 · Audit-receipt tamper tests** + finish the budget (api-calls/hr cap).

## P3 — Ecosystem reach (after the paid surface is proven; pull forward with a design partner)

> **Why a new tier (added 2026-06-17).** The governed in-process layer is currently **JS/TS-only**
> (`kriya-core` / `wrapAction`). The biggest unserved *in-process* surface — CAD (FreeCAD / Blender /
> Fusion 360), data/ML, scientific & engineering desktop tools, much enterprise scripting, and a lot
> of accounting/ERP (e.g. Tally is C++ + the TDL DSL + a local XML/HTTP gateway) — is reachable today
> only via the MCP + `ProcessExecutor` **bridge** (governed at the process boundary, not in-process).
> A second-language binding upgrades that whole class to first-class, in-process `wrapAction` targets.
>
> **The language ladder (research 2026-06-19, decision [D-012](DECISIONS.md), full ranking in
> [strategy/language-bindings.md](strategy/language-bindings.md)).** R3 made the host
> binding-agnostic (NDJSON over stdio), so each new language is *a second binding, not a new host* —
> the cost is roughly flat, and the order is set by **on-thesis app surface**, not popularity:
> **R17 Python** (CAD / data-ML / scientific / quant) → **R18 C#/.NET** ⭐ (the bullseye: WPF/WinForms
> LOB — regulated health/finance/manufacturing/gov Windows desktop) → **R19 Java/Kotlin** (the JVM
> half of regulated enterprise desktop). C++ is design-partner-gated; **Go and Ruby are explicit
> traps** (server/web/CLI ecosystems where a good cloud API already exists — where the thesis says
> kriya isn't needed). All demand-pulled: ship a binding when a real design partner in that ecosystem
> appears.

- ✅ **R17 · Python SDK binding** — shipped (`fae5909` + `5f1b67a`). See **Done** below. A Python
  `register_action` / `wrap_action` mirror of `kriya-core` + the `kriya-sidecar` host driver, in one
  `pip install kriya` package under `bindings/python/`. Unlocks the Python in-process ICP: CAD
  automation (FreeCAD/Blender), data/ML, scientific/engineering, quant/accounting. *Pulled forward
  ahead of R6/R7 this session as a deliberate "stay-ahead / breadth" build, not a design-partner
  trigger.* The near-term CAD demo (**JSCAD / Replicad**) is **JS/TS already** and needs none of this.

- ⬜ **R18 · C#/.NET SDK binding** ⭐ (🌐 public, MIT) — *the #1 second binding after Python
  ([D-012](DECISIONS.md))*. A `registerAction`/`wrapAction` mirror shipped on **NuGet** that speaks the
  same stdio/NDJSON protocol to `kriya-host`, with a **WPF or WinForms** sample. Unlocks the largest
  and highest-willingness-to-pay slab of the ICP: regulated/LOB **Windows desktop** (health, finance,
  manufacturing HMIs, gov), plus the SolidWorks/Revit/Unity add-in world (also C#). The only existing
  .NET desktop MCP path today is **accessibility-tree scraping**; **Windows 11's On-Device Agent
  Registry is actively pushing every local app toward typed, signed, on-device MCP** — creating demand
  while leaving the governance batteries (in-app approval, budget, signed audit, memory) for kriya to
  own. Low-effort binding (clean NuGet + stdio child process, no FFI). **Pull forward the instant a
  concrete .NET desktop design partner appears.**
- ⬜ **R19 · Java/Kotlin (JVM) SDK binding** (🌐 public, MIT) — one binding (Java, usable from
  Kotlin/Scala) on **Maven Central**, with a Swing or JavaFX sample. Captures the JVM half of
  regulated enterprise desktop (EU public sector, banks, hospitals, industrial control dashboards) —
  the same seam, second-largest surface after .NET.
- ❌ **C++ binding — deferred (design-partner-gated).** Highest stakes (EDA, medical imaging, CAD
  kernels) but its marquee apps are reachable via the .NET/Python add-in SDKs anyway, and it has no
  universal package manager. Build only behind a specific embedded/medical-device partner.
- ❌ **Go / Ruby bindings — not doing (traps).** Server/web/CLI ecosystems where a good cloud API
  usually exists — exactly where the thesis says kriya isn't needed. See [D-012](DECISIONS.md).

## Launch (after the wedge + publish; gated on planner's go)

- ⬜ **R12 · Launch.** Full plan in **[LAUNCH.md](LAUNCH.md)** — pre-launch checklist (GIF in
  README, fresh-machine smoke, repo public, CI green), Show HN title + opening comment + objection
  answers, Twitter thread, timing, distribution order.

## Explicitly deprioritized / not doing (per research)

- ❌ Web framework bindings (Vue/Svelte for web) — don't fight WebMCP.
- ❌ Mobile (Flutter/SwiftUI/Compose) — premature.
- ❌ Scaffolder polish beyond demo quality — it's the demo, not the product.
- ✅ **Renamed off "agent-native" → `kriya`** — Builder.io owns the term "agent-native"; the
  public name is now **kriya** and the packages/crate/binaries were renamed accordingly.

---

## Done (newest first)

- ✅ **R17 · Python SDK binding** — `fae5909` (library + unit tests) + `5f1b67a` (integration +
  example). [`bindings/python/`](../bindings/python/): one `pip install kriya` package mirroring
  `kriya-core` (registry, schema, validation, draft-clean JSON-Schema export, `register_action` /
  `wrap_action` with composition + cycle/depth guards) and `kriya-sidecar` (the `Host` stdio NDJSON
  driver + `run_task`, camelCase wire, `recent_memory()` correlation). Zero runtime deps. **51 unit
  tests** green with `python -m unittest` (no binary), plus an opt-in **integration test that drives
  the real `kriya-host`** through action dispatch + a held/granted approval + memory recall, and a
  runnable `examples/note_app_host.py` (the Python mirror of `node-sidecar-host`). Verified end to
  end: two signed creates, `delete_note` held for approval then signed, episodes recalled. *A second
  binding, not a new host* — the first non-JS language on the in-process layer; sets the `bindings/`
  convention for R18 (.NET) / R19 (JVM).
- ✅ **R8 (public half) · Signed-receipt `actor` field** — `ccdb444` (runtime) + `57784fb` (console).
  Records *who* took each action (agent + operator) **inside** the Ed25519-signed receipt, so the
  attribution is tamper-evident. Threaded through the in-process host (resolves from the run
  request, else backend name + OS user), the MCP `Governor` (`kriya-mcp --actor/--user`), the
  offline `verify-receipts` verifier, and the console's TS verifier + audit table — all
  cross-checked against real Rust-signed receipts. Additive (`skip_if_none`): pre-R8 receipts sign
  byte-identically. Demo: `examples/actor-identity/`. The identity-**management** half (SSO/RBAC)
  stays in 🔒 `kriya-console`.
- ✅ **R13 · On-device guarantee** — `64b340f`. `NetworkProfile` + `Inference::network_profile()`
  (defaults to `remote` — a backend is never *silently* on-device); a sealed policy
  (`on_device: true`) makes the host refuse an egressing backend before any step and sign a
  `kriya.attestation.on_device` receipt, verifiable offline alongside the action receipts. Honest
  classification (Ollama on loopback = on-device, claude-cli + Anthropic = remote). Demo:
  `examples/on-device-guarantee/`. The regulated-ICP "nothing leaves the device" posture, attested.
- ✅ **R2 · Publish packages** — **2026-06-15** (planner). `kriya-core` 0.0.1 · `kriya-sidecar`
  0.0.1 · `kriya-inspector` 0.3.0 · `create-kriya-app` 0.2.0 on npm; `kriya` 0.1.0 on crates.io;
  scaffolder template path-swapped to the published crate. Republish for the P0.5 API (core/sidecar
  0.0.2, crate 0.1.1) is staged — see PUBLISHING.md "Republishing for P0.5"; commands are the planner's.
- ✅ **P0.5 · Cross-shell parity (R14 + R15 + R16)** — `93c5a67`. Hardened the embedded-sidecar leg
  (the Electron/Node half of the thesis) to Tauri parity, all additive: **R14** the `runTask` helper
  now answers `awaitStep` (step-mode no longer hangs); **R15** durable memory recall over the sidecar
  protocol (`memory_recent`/`memory` + `SidecarHost.recentMemory()` + exported `Episode`), the
  sidecar mirror of Tauri's `agent_memory_recent`; **R16** renamed `VERB_HOST_BIN`→`KRIYA_HOST_BIN`,
  an integration test that drives the **real** `kriya-host` binary through action + held/granted
  approval + memory recall, and a runnable `examples/node-sidecar-host/` (Node = an Electron main
  process). Verified end to end against the real binary. Closes the Tauri⇄Electron gaps flagged in
  PRODUCT_GAPS §2 before P1 (publish/dashboard) builds on the sidecar.
- ✅ ⭐ **R5 · THE FLAGSHIP DEMO** — `24ed278` (2 commits: `853aa8b` persistent-handler executor
  for kriya-mcp, `24ed278` the bolt-on). [`examples/actual-budget-bolt-on/`](../examples/actual-budget-bolt-on/):
  governed agent access to **Actual Budget** (real, local-first, no-HTTP-API finance app) in a
  **~37-line** `wrapAction` file — no rewrite. An external agent (Claude Desktop) calls actions
  over MCP; `kriya-mcp` routes each through policy → approval → budget → signed audit, then a
  persistent Node handler holds Actual's in-process connection and runs only cleared actions.
  Policy: reads + categorize/budget allow; `delete_transaction` + `close_account` require human
  approval; deny-by-default; 30/min budget. Verified end to end (incl. an `ACTUAL_FAKE=1` no-setup
  demo mode): categorize runs + is signed, delete/close are held for approval, unlisted actions
  refused. **What's left is recording the before/after video** (the artifact for the YC app).
- ✅ **R4 · `wrapAction` + codemod** — `0afc8ca` (2 commits: `a830ab0` the `wrapAction` runtime,
  `0afc8ca` the codemod). `wrapAction(fn, { id, description, parameters, mapParams, mapResult })`
  adapts a function an app already has — positional args, plain return, throws — into a
  registered action, normalizing the return/throw into an `ActionResult`. The `kriya
  wrap <file>` codemod scans a source file's exported functions (TypeScript compiler API),
  infers each parameter's schema + required-ness, pulls the description from JSDoc, and prints
  a `wrapAction(...)` module to review and import. Verified end to end: wrap a typed file → the
  generated module registers and emits correct MCP tool schemas via `dump`. 16 new tests
  (8 runtime + 8 codemod). **Augment, not migrate — the bolt-on that makes R5 real.**
- ✅ **R3 · Sidecar host + Electron/Node binding** — `8b3a8c2` (3 commits: `0832cd1` decouple
  the loop from Tauri via a `HostSink` trait, `5df3c4b` stdio sidecar + `kriya-host` binary,
  `8b3a8c2` the `kriya-sidecar` Node/TS binding). The agent loop is now transport-
  agnostic: `run_task` emits through `HostSink` (Tauri is just `TauriSink`); the new `kriya-host`
  binary runs the loop as a standalone process speaking newline-delimited JSON over stdio, with
  governance in a process the renderer can't tamper with. `kriya-sidecar` (`SidecarHost`
  + `runTask`) spawns it from Electron/Node. A generic `ScriptedPlanner` backend (`--script`)
  gives zero-config, no-API-key deterministic runs for demos/CI. Verified end to end: a Node
  client drove the real binary through a full run (action dispatch + approval gate + done).
  Closes the §2 "separate-process host" gap. **Cross-shell decoupling — bolt onto any shell.**
- ✅ **R1 · Governed MCP-server mode** — `d1e28e6` (3 commits: `20305d1` protocol types,
  `f56f1b4` governed dispatch, `d1e28e6` stdio server + `kriya-mcp` binary). New `mcp` module
  in `kriya`: an stdio JSON-RPC server (`initialize` / `tools/list` / `tools/call`)
  that routes every external-agent call through the same policy → approval → budget →
  signed-audit gates the in-process host enforces. Execution + approval are traits
  (`ActionExecutor`, `ApprovalGate`) so the same governance serves Tauri, a sidecar (R3), or a
  CLI; `ProcessExecutor` is the dependency-free bolt-on. The thin `kriya-mcp` binary takes
  `--tools` + `--policy` + `--exec` + `--approval deny|tty|auto`. 21 unit tests + verified end
  to end against the real binary (allowed action signed, guarded action held, unregistered tool
  refused). **Turns kriya from a rewrite into a bolt-on.**
- ✅ CI workflow + CONTRIBUTING + issue templates — `f191618`
- ✅ Step-through (host pause + inspector StepGate) — `6070302`
- ✅ Policy linting at startup — `255e6ce`
- ✅ Resume-ability (run_id + last_resumable_run) — `5401c6d`
- ✅ Inspector v0.3.0 (replay + StepGate) & scaffolder README — `0fb85fc`
- ✅ Action composition + ESM `.js` fix — `edfb898`
- ✅ Second reference app (task-manager) — `cfb797c`
- ✅ Extract Rust host into `kriya` crate — `db30153`
- ✅ `create-kriya-app` scaffolder — `4b751b0`
