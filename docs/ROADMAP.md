# Roadmap & Backlog

> **This is the checklist the planner points the agent at.** To start work, say something like
> "do R1" or "do the flagship demo." **R-numbers are stable IDs, not a sequence — priority is the
> tier.** When an item ships, move it to **Done** at the bottom with the commit SHA. Strategic
> *why* lives in [strategy/](strategy/); feature detail in [PRODUCT_GAPS.md](PRODUCT_GAPS.md).
> Direction is set by decision [D-009](DECISIONS.md): the governed **in-process** action layer
> for no-API local capabilities (desktop/in-process = mechanism, governance = moat).

Legend: ⬜ not started · 🟡 in progress · ✅ done · ⭐ flagship

---

## PG — The governance-gateway pivot (D-016) — 🟡 THE CURRENT FRONT DOOR

> **Why this tier (added 2026-06-24, decision [D-016](DECISIONS.md); full build doc:
> [SERVICE-ARCHITECTURE.md](SERVICE-ARCHITECTURE.md)).** MCP-stdio commoditized the *transport*, not
> the *enforcement* — native MCP has **no enforced governance** (approval is SHOULD-not-MUST and
> client-side; auth is OPTIONAL and excludes stdio). So a **zero-change governance proxy** that wraps
> any existing MCP server, **shipped as a downloadable product** (not a library), is the new adoption
> front door; the `wrapAction` library is repositioned as enterprise depth. This is **build-over, not
> rewrite**: `Governor::dispatch()` is already transport-agnostic behind the `ActionExecutor` trait —
> each front is a new executor + a tool-discovery source; the core (policy/approval/budget/signed
> audit/attestation) is reused unchanged. Critical path: **R22 → R23 → R24** (ship the product), then
> R25 (Front 2), R26 (Front 3, deferred).

> **✅ Shipped & verified (`feat/gateway-front1`, 2026-06-24): R22 + R23 + the R24 core.** The
> `kriya-gateway proxy` binary wraps any stdio MCP server with **zero changes**, governing every
> `tools/call` through the unchanged `Governor`. Proven end-to-end against a mock MCP server
> (`examples/gateway-proxy-demo/`, runnable via `run.sh`): a read (`list_notes`) is forwarded +
> **signed**, a destructive call (`delete_note`) is **blocked at the approval gate, never forwarded,
> never signed**, and the receipt **verifies offline** (`verify-receipts`: verified 1, failed 0).
> Build-over confirmed: **+112 tests (mcp-client) / 96 (default) pass, clippy + fmt clean**, no new
> deps, default build untouched. The new `mcp-client` feature is off-by-default so the library stays
> lean. R24 *remaining* (installers/config/attestation) is itemized below.

- ✅ **R22 · Upstream MCP client + `McpProxyExecutor` (Front-1 engine).** Build the missing half:
  kriya is MCP-*server*-only today (seam audit confirmed zero client code). `mcp/client.rs`
  (`McpClient`: spawn downstream server as a subprocess — the proven `PersistentProcessExecutor`
  spawn pattern — with id-correlated `initialize`/`tools/list`/`tools/call` over its stdio, reusing
  `jsonrpc.rs` types as-is) + `mcp/proxy_executor.rs` (`McpProxyExecutor: ActionExecutor` that
  forwards a cleared call to the downstream and maps `CallToolResult → ActionOutcome`). New
  off-by-default `mcp-client` feature (lean lib, mirrors `tauri-host`/`http-inference`). The
  `Governor`/`ApprovalGate`/`Signer`/`Budget`/`Policy` are reused unchanged.
- ✅ **R23 · Front-1 transparent proxy serve loop.** Make kriya govern *in front of* any MCP server
  with zero target changes. The four seam-audit fixes (current `Server` blocks all of these):
  (1) forward `notifications/initialized` + notifications **both ways**; (2) **pass through unknown
  methods** (`resources/*`, `prompts/*`, server-initiated `sampling/*`/`elicitation/*`) verbatim;
  (3) **dynamic `tools/list`** cached from the downstream (not a static file); (4) **policy-filter
  `tools/list`** so denied tools never appear. Ship a **passthrough conformance test** (the D-016
  open question) before any "wraps any MCP server" claim. MVP = synchronous request/response;
  full-lifecycle = two reader threads + `Mutex<Governor>` for server-initiated traffic.
- 🟡 **R24 · ⭐ The shippable `kriya-gateway` product.** **Done:** the `proxy` subcommand
  (zero-config), the **default deny-by-default policy generator** (reads allow / destructive→approval
  / else deny), policy warnings, path validation before `Signer`, the full flag set, the runnable
  `examples/gateway-proxy-demo/`, **`.kriya.yaml` config-file discovery** (`--config`/auto-discover;
  CLI > config > default), and the **on-startup on-device attestation receipt** (R13 mechanism;
  written as the genesis log line under a pinned `--signing-key`, skipped + noted for ephemeral keys).
  **Remaining:** cross-platform GUI approval (Windows/Linux; macOS Gui + Tty done), and signed
  installers (dmg/Homebrew, msi/winget) with a pinned public key. Original full scope: Turn the dev-facing
  `kriya-mcp --tools … --policy … --exec …` into a zero-config download a non-author drops into a
  client's MCP config: `{"command":"kriya-gateway","args":["proxy","--","node","server.js"]}`. One
  binary, subcommands (`proxy` = Front 1; `serve` = the existing bolt-on; `reach-in` = Front 2 later).
  Build: zero-config **default policy generator** (reads allow / `delete_*|remove_*|destroy_*` →
  approval / else deny), **`.kriya.yaml` config** discovery, **cross-platform approval** (Tty
  everywhere + macOS Gui; validate audit-log/signing-key paths before `Signer` so a bad path is a
  clean error not a crash), **on-startup on-device attestation** (R13 receipt when `--signing-key`
  set), **warn on policy/tool mismatch**, and **signed installers** (macOS `.dmg`/Homebrew + codesign;
  Windows `.msi`/winget + Authenticode; Linux self-contained) with a pinned public key for offline
  receipt verification. Keep it **local-only, per-host, no credential aggregation** (the answer to
  "MCP gateways are a bad idea").
- ✅ **R25 · Front-2 reach-in adapter (no-MCP / no-API apps)** — shipped (macOS). `mcp::reachin`
  (off-by-default `reach-in` feature): an `AxBackend` trait (testability seam) with a `MacAxBackend`
  FFI impl (`accessibility-sys` + `core-foundation`, `AXUIElementCopyActionNames`/`AXUIElementPerformAction`,
  `AXIsProcessTrusted` gate), pure tool **synthesis** from the AX tree, `AxExecutor: ActionExecutor`,
  and `ReachInServer` (same `Governor` gates as the proxy; policy-filtered `tools/list`). Exposed as
  `kriya-gateway reach-in --app "<App>"`. **23 unit tests via a fake backend** (no permission needed)
  + 1 ignored real-AX test; verified the subcommand runs to the TCC gate. **Honest scope:** macOS
  only, needs user-granted Accessibility permission, degrades on Electron/Qt/custom-drawn UIs — the
  "any macOS app" claim was research-refuted. **Still ⬜:** Windows UIA backend; the **coverage-ratio
  measurement on 5 real ICP apps** before this goes in a pitch.
- ✅ **R26 · Front-3 governed computer-use (D-017)** — SHIPPED (macOS; was deferred, pulled in by
  D-017 "support everything / sell governance"). `mcp::computeruse` (off-by-default `computer-use`
  feature): a fixed system-wide tool set (`computer_screenshot/click/move/scroll/type/key`,
  `list_apps`) that drives **any** app via pixels (CGEvent + `screencapture`, base64 PNG result),
  every action through the unchanged `Governor`. The **universal governed floor** — no app is
  unsupported. `kriya-gateway computer-use`. **Honest caveat:** pixel-tier policy is coarse (gate
  clicks/keystrokes, not named actions); richest governance stays the instrumented fronts.
- ✅ **R26.1 · Router v2 — auto-tier multiplexer (D-017)** — SHIPPED (`b7cadb7`). One MCP endpoint
  (`kriya-gateway router [--reach-in "App,…"]`) multiplexing the computer-use floor + per-app reach-in
  under ONE `Governor` (one policy/signer/audit/actor); tools served namespaced `<ns>__<tool>`, routed
  per call. 14 router tests; verified live (`cu`(7) + `numbers`(1354) under one audit).

---

## CP — Console / control-plane app + distribution (D-018) — 🔜 NEXT SESSION

> The shippable, demoable form of the paid Console + how it's sold. The Console rebuild lives in the
> **private `kriya-console` repo**; the standard-log-location + bundle/onboarding bits touch the
> **public gateway**. Build next session (a kickoff prompt was handed off). See D-018.

- ✅ **R27 · Standard on-device audit-log location + auto-discovery** *(public gateway)* — shipped
  (`ea24602`). The gateway **defaults** its signed-receipt log to the standard `~/.kriya/audit/` dir
  (`audit::default_audit_dir()`, the shared writer/reader convention; created on demand, temp-dir
  fallback). Each front writes a stable, hash-chain-continuing file there (`proxy → <server>.jsonl`,
  `reach-in → reach-in-<app>.jsonl`, `computer-use → computer_use.jsonl`, `router → router.jsonl`);
  `--audit-log` still overrides. `doctor` surfaces the location; `--signing-key` no longer needs
  `--audit-log` (the default log is always present). Verified live: a proxy with **no** `--audit-log`
  wrote `~/.kriya/audit/mock_mcp_server.jsonl`, receipt verifies offline. 97 crate + 4 new gateway
  unit tests green. The Console's **auto-discover + tail** of this dir lands in R28.
- ✅ **R28 · Tauri control-plane desktop app** *(private Console + public bundle/onboarding)* — shipped
  in 🔒 `kriya-console` (Tauri rebuild). A compiled **Rust backend** holds the paid value + the license
  check; the existing React views are the thin viewer. The backend **auto-discovers + tails**
  `~/.kriya/audit/` (R27) and streams **Rust-verified** receipts live; the **sample-log loader is
  removed** (manual import demoted to a secondary "open a file"). One download bundles the **gateway
  binary as a Tauri sidecar** + an **onboarding GUI** (detect gateway, open Accessibility/Screen-Recording
  panes, merge the MCP client config — reuses `doctor` logic) + the **live signed-audit cockpit**.
  Freemium in one app: free = live monitor + verify + onboarding; paid = license-gated **fleet
  correlation + compliance export**, both generated in compiled Rust. Verified end-to-end via
  `tauri dev` (live receipts shown + verified, incl. an R20-chained receipt — fixed a `prev_hash` gap
  in the browser verifier too) and the `.app` builds. Stable self-signed signing identity: see the
  related task below.
- ✅ **R29 · Offline-license distribution (paid tier)** *(private)* — shipped in 🔒 `kriya-console`. The
  Rust backend verifies a **signed Ed25519 license fully offline** against an embedded issuer public
  key (reuses the receipts canonicalization). No runtime server / no upload / no accounts; the free
  tier needs none. Paid features call a `require_pro()` gate. The **issuer is the documented deferred
  stub** (a dev `issue-license` minter behind a gitignored seed proves the verify path; checkout →
  offline signer is wired when a buyer exists). Verified live: a minted license flips the app free→pro
  and unlocks the gated views. Cloud/self-host **fleet** console still deferred.

---

## P0 — Critical path to the YC demo — ✅ COMPLETE (R1 → R3 → R4 → R5 shipped)

The wedge's critical path is walked: governed MCP mode → sidecar host → `wrapAction` bolt-on → the
Actual Budget flagship. The one artifact still outstanding is the **video** — now tracked as
`R5-video` (below) instead of hiding as prose inside done-R5.

- ✅ **R1 · Governed MCP-server mode** — shipped (`d1e28e6`). See **Done** below.
- ✅ **R3 · Sidecar host + Electron/Node binding** — shipped (`8b3a8c2`). See **Done** below.
- ✅ **R4 · `wrapAction` + codemod** — shipped (`0afc8ca`). See **Done** below.
- ✅ ⭐ **R5 · THE FLAGSHIP DEMO** — shipped (`24ed278`). See **Done** below. The wedge's
  critical path (R1 → R3 → R4 → R5) is complete; what remains is the **video** (R5-video) + P1
  (monetize/distribute) and P2 (compliance/polish).
- ✅ ⭐ **R5-video · Actual Budget before/after governance video** — **done**: recorded and published
  on the site demo page → **[kriyanative.com/#demo](https://kriyanative.com/#demo)**. An on-device
  agent driving Actual Budget through kriya — routine actions run + are signed, money-moving ones
  blocked pending in-app approval, every receipt verifiable offline. The one-glance proof a cloud
  sidecar / Copilot / Scout can't replicate (a no-cloud-API app governed **in-process**). P0 is now
  fully complete (R1 → R3 → R4 → R5 + the video).

## P0.5 — Cross-shell parity — ✅ COMPLETE (R14 / R15 / R16 shipped, `93c5a67`)

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

- 🟡 **R6 · Governance dashboard (the paid surface)** — in progress in 🔒 private `kriya-console`:
  cross-app/agent audit viewer, org policy editor, multi-approval routing, **live budget controls**,
  and **per-user/agent + RBAC dashboards** are built; the remaining surface is hosted-tier (SSO/OIDC
  sign-in). (The D-011 split keeps the build invisible to this public repo.) Open-core monetization;
  leans on the audit/budget/approval/policy primitives this repo ships.
- ✅ **R2 · Publish packages** — **done 2026-06-15** (planner ran it). All four npm packages are
  live (`kriya-core` 0.0.1, `kriya-sidecar` 0.0.1, `kriya-inspector` 0.3.0, `create-kriya-app`
  0.2.0) and the `kriya` crate is on crates.io (0.1.0); the scaffolder template already path-swapped
  to the published crate (`kriya = "0.1"`). **Republish pending for the P0.5 API:** today's R14–R16
  added `recentMemory()`/`Episode` to `kriya-core` + `kriya-sidecar` (npm immutable, so a 0.0.2 bump
  is staged) and a `memory_recent` handler to the `kriya` crate (0.1.1 staged) — versions are bumped
  in-repo; the `npm publish`/`cargo publish` commands are the planner's ([D-004](DECISIONS.md),
  [PUBLISHING.md](PUBLISHING.md) → "Republishing for P0.5"). Optional — only external installers need it.

## P2 — Compliance & polish

> **Repo split (D-011):** R7 → 🔒 `kriya-console`; **R8 splits** (signed-receipt `actor` field 🌐 public, identity-mgmt 🔒 private); R13 / R9 / R10 / R11 / R20 / R21 → 🌐 public.

- ✅ **R7 · Compliance-evidence export** — shipped in 🔒 `kriya-console` (`a7e9d68`): audit log →
  SOC 2 / ISO 42001 / EU AI Act evidence bundle (integrity + R8 attribution + R13 on-device +
  control mapping), Markdown/JSON export. The willingness-to-pay hook (EU AI Act high-risk
  obligations now Dec 2027 per the Digital Omnibus; SOC 2 / record-keeping demand is already here).
- 🟡 **R8 · Agent + user identity per action.** Public **signed-receipt `actor` field** shipped
  (`ccdb444` runtime + `57784fb` console) — agent + operator stamped *inside* the signed bytes
  (tamper-evident), threaded through the in-process host, MCP governor (`--actor`), the offline
  verifier, and the console audit table. The 🔒 identity-**management** half is now mostly built in
  the private console (**RBAC** roles keyed on the operator + **per-user/agent dashboards**); only
  **SSO/OIDC sign-in** remains (hosted-tier — it needs a backend the client-only console doesn't have).
- ✅ **R13 · On-device guarantee** — shipped (`64b340f`). A sealed policy (`on_device: true`) makes
  the in-process host refuse an egressing inference backend and sign a `kriya.attestation.on_device`
  receipt (verifiable offline). Backends declare an honest `NetworkProfile` (scripted=none,
  Ollama=localhost-only/remote, claude-cli + Anthropic=remote). The regulated-ICP "nothing leaves
  the device" posture, attested.
- ✅ **R9 · Resume-ability UI + persist approval queue** — shipped (`4873812` + `fede962` +
  `1a37038`). See **Done** below. Pending approvals are now persisted durably; a run interrupted
  mid-approval re-issues the held action on resume instead of dropping it, and note-app has a
  "Resume last task" button.
- 🟡 **R10 · Reliability (retry/backoff + clean escalation) — shipped; OpenAI cloud backend deferred.**
  A roadmap-validation split this item in two and built only the **on-thesis reliability half**.
  ✅ **Shipped (`feat/r10-reliability`):** bounded retry/backoff around `backend.next_step()` in the
  host loop (configurable `retry:` policy — `max_retries`/`initial_backoff_ms`/`max_backoff_ms`,
  default 3 retries / 250ms→5s) so a *transient* backend error (network blip, rate-limit, parse
  hiccup) is retried instead of failing the whole run; and a **"too-hard → escalate/abort cleanly"
  fallback** — past the retry budget the host ends the run **gracefully** (a descriptive `AgentDone`
  + error log), never hanging or panicking, the degrade-cleanly behavior a regulated workstation host
  needs. Deterministic/scripted backends never error → zero behavior change. New `inference::retry`
  unit (5 tests) + a `retry:` policy-parse test + two end-to-end host tests (retry-then-recover,
  always-fail → clean done). 88 crate tests, clippy clean.
  ⬜ **Deliberately NOT built — the OpenAI cloud inference backend.** It is **off the on-device wedge**
  (D-009): a cloud backend cuts against the thesis-critical on-device inference path (Ollama,
  claude-cli). **Explicitly demand-pulled/demoted** — build only if a concrete design partner needs
  it. The reliability half above is backend-agnostic and helps the on-device backends today.
- ✅ **R11 · Audit-receipt tamper tests + finish the budget (api-calls/hr cap)** — shipped
  (`44637f5` + `e2ae449`). See **Done** below.
- ✅ **R20 · Durable host signing identity + tamper-evident log chaining** (🌐 public) — shipped
  (`26f750c` + `2163b10`). See **Done** below. Closed both [SECURITY.md](SECURITY.md) limitations:
  a persisted host key (`--signing-key`, stable across runs) **and** hash-chaining each receipt to its
  predecessor (deletion/truncation/reorder now detectable). The roadmap-validation (2026-06-21) named
  this the highest-leverage on-theme build — the AGT-differentiating half a free cloud audit sidecar
  can't hand you (an in-process *complete-log* guarantee).
- ✅ **R21 · Deterministic canonical receipt serialisation** (🌐 public) — shipped (`b51370f`). See
  **Done** below. Explicit recursive key-sort of `params` before signing (in `audit.rs::record()`)
  and the identical sort in `tools/verify-receipts`, so the canonical bytes are independent of any
  consumer's `serde_json` `preserve_order` flag. Byte-identical today; hardens the audit trail + the
  patent's canonicalisation claim.

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

- 🟡 **R18 · C#/.NET SDK binding** ⭐ (🌐 public, MIT) — **binding shipped & verified** (`fa61b0a` +
  `9380299`); the marquee third-party bolt-on is the remaining flagship piece. Built **without** a
  design partner (planner's call, 2026-06-22) as the #1 second binding after Python
  ([D-012](DECISIONS.md)). `bindings/dotnet/` is a faithful port of the verified Python binding —
  `Registry` (RegisterAction/WrapAction, validation, composition, MCP/JSON-Schema), `Protocol`,
  `Host` (spawn kriya-host over stdio, RunTask, RecentMemoryAsync); targets **net8.0** (LTS — the
  regulated Windows-desktop ICP) + net10.0. **25 tests** incl. an integration test driving the real
  `kriya-host` binary, plus a runnable console example (the macOS parallel of node-sidecar-host).
  NuGet metadata staged (publish is the planner's, [D-004](DECISIONS.md)). **Still ⬜ — the
  third-party bolt-on demo:** the 2026-06-22 target-research workflow's top pick (Mnemo, Avalonia)
  was disqualified by its own adversarial-verify phase — it shipped an enabled-by-default in-process
  **MCP server** + adopted an external agent permission framework *that day*, so it's no longer
  Kriya-Prime. No clean high-stars + cross-platform + non-MCP target surfaced; the finance runner-up
  **MyMoney.Net** (WPF, the .NET Actual-Budget parallel) was chosen as the strongest narrative target.
  The bolt-on is **staged at [`examples/mymoney-dotnet-bolt-on/`](../examples/mymoney-dotnet-bolt-on/)**
  (`f904de8`) — drop-in `WrapAction` code (categorize/list free; delete/transfer/close → approval) +
  `agent-policy.yaml`, source-only. WPF is Windows-only, so **build + record it on Windows** and
  confirm the `⚠️ API` signatures against the checkout; the binding it builds on is verified
  cross-platform.
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

- 🟡 **R12 · Launch.** **YC application submitted** (the launch's first milestone — the
  demo at [kriyanative.com/#demo](https://kriyanative.com/#demo) is live). Remaining in
  **[LAUNCH.md](LAUNCH.md)**: the public Show HN / Twitter push (pre-launch checklist — GIF in README,
  fresh-machine smoke, repo public, CI green — then title + opening comment + thread), on the
  planner's timing.

## Explicitly deprioritized / not doing (per research)

- ❌ Web framework bindings (Vue/Svelte for web) — don't fight WebMCP.
- ❌ Mobile (Flutter/SwiftUI/Compose) — premature.
- ❌ Scaffolder polish beyond demo quality — it's the demo, not the product.
- ✅ **Renamed off "agent-native" → `kriya`** — Builder.io owns the term "agent-native"; the
  public name is now **kriya** and the packages/crate/binaries were renamed accordingly.

---

## Done (newest first)

- 🟡 **R18 (binding) · C#/.NET SDK binding** — `fa61b0a` (binding + 25 tests) + `9380299` (runnable
  example). The .NET binding of kriya: `bindings/dotnet/` — `Registry` (RegisterAction/WrapAction,
  validation, composition, MCP/JSON-Schema tool schemas), `Protocol` (camelCase NDJSON wire), `Host`
  (spawn `kriya-host` over stdio, `RunTask`, `RecentMemoryAsync`), typed `P.*` parameter schemas.
  A faithful C# port of the verified Python binding — a *second binding, not a new host*. net8.0
  (LTS) + net10.0. Verified end-to-end: 25 tests (registry/validation/json-schema/host-framing) +
  an integration test driving the **real** `kriya-host` (action → approval → memory recall), + a
  runnable console example (the macOS parallel of `node-sidecar-host`) — proven live on macOS.
  Built without a design partner (planner's call). Remaining flagship: the third-party bolt-on
  (research disqualified Mnemo — went MCP-native; MyMoney.Net is the Windows finance fallback).
  NuGet publish staged (planner, [D-004](DECISIONS.md)).
- 🟡 **R10 (reliability half) · Retry/backoff + clean escalation** — `feat/r10-reliability`. The
  on-thesis half of R10 (a roadmap validation split the item): a new `inference::retry` unit wraps
  the host loop's `backend.next_step()` (previously a bare `?`) in **bounded retry with exponential
  backoff** — a *transient* failure (network blip, rate-limit, parse hiccup) is ridden out instead of
  killing the run — and past the budget the host **escalates by ending the run gracefully** (a
  descriptive `AgentDone` + error log; never a hang or panic — the degrade-cleanly behavior a
  regulated workstation needs). Retry count/backoff are configurable via an optional `retry:` policy
  section (default 3 retries / 250ms→5s). Backend-agnostic and helps the on-device backends (Ollama,
  claude-cli) today. **Deterministic/scripted backends never error → zero behavior change** (all prior
  host tests untouched). 8 new tests (5 retry-unit + 1 policy-parse + 2 end-to-end host:
  retry-then-recover, always-fail → clean done); 88 crate tests, clippy clean. **Explicitly NOT
  built: the OpenAI cloud inference backend** — off the on-device wedge (D-009), demand-pulled/demoted
  (a cloud backend cuts against the thesis-critical on-device path). R10 stays 🟡, not ✅.
- ✅ **R20 · Durable host signing identity + tamper-evident log chaining** — `26f750c` (durable
  identity) + `2163b10` (hash-chaining). **(1)** `Signer::with_identity(key_path, log_path)` persists
  the Ed25519 seed (`0600` hex, loaded-if-present, error-not-overwrite), exposed as `kriya-host
  --signing-key` — a trust anchor stable across runs (proven: two host processes, one key →
  byte-identical `public_key`). **(2)** each receipt carries `prev_hash` = SHA-256 of the previous
  *line* (signed, `skip_if_none` so genesis/pre-R20 receipts are byte-identical); the `Signer` holds
  the chain head, seeds it from the log's last line so a new process continues the chain, and holds
  the write lock so on-disk order == chain order; `verify-receipts` re-checks the chain and flags
  deletion/reorder/head-truncation (exit 1). Proven end-to-end: host signed a 3-receipt chain →
  verifier passed → deleting the middle receipt → `CHAIN-BREAK`. 82 crate + 9 verifier tests; clippy
  clean. The on-device *complete-log* guarantee a cloud audit sidecar (e.g. MS Agent Governance
  Toolkit) structurally can't produce.
- ✅ **R21 · Deterministic canonical receipt serialisation** — `b51370f`. Explicit recursive key-sort
  of `params` before signing (`audit.rs::canonical_value`) + the identical sort in `tools/verify-receipts`,
  so the signed bytes don't depend on any build's serde_json `preserve_order` feature. Byte-identical
  today (cross-checked: Python binding drove the host → a sorted-params receipt the standalone
  verifier validated). Hardens the audit trail + the patent's canonicalisation claim. 15 audit tests.
- ✅ **Positioning essay in-repo** — `0c3cf24`. `docs/THREE-FRONTIERS.md`: the original "three
  frontiers of agent-meets-software" essay. **Superseded 2026-06-27** — its premise (desktop has no
  agent tooling standard) is outdated (desktop has MCP-stdio too); the public positioning is now the
  governance / control-plane thesis in the README. The essay is kept **local-only** (gitignored) for
  history; the README no longer links it.
- ✅ **R11 · Audit-receipt tamper tests + finish the budget (api-calls/hr cap)** — `44637f5`
  (budget) + `e2ae449` (tamper tests). **Budget battery complete:** a second, independent
  trailing-hour cap on inference/API calls (`budget.max_api_calls_per_hour`) bounds model *cost*,
  next to the existing per-minute action cap that bounds bursts — a loop can't run up unbounded
  backend spend even when it dispatches few/no actions. `budget.rs` refactored onto a reusable
  sliding window (per-minute API + tests unchanged); the host meters each `backend.next_step()` and
  stops on exceed. **Tamper-evidence hardened:** 8 new audit tests prove that rewriting any signed
  field (action_id / success / step_id / ts_ms), fabricating an actor after signing, a forged
  signature, a substituted public key, or malformed hex all fail to verify — the cryptographic
  spine of [SECURITY.md](SECURITY.md). 69 crate tests + clippy clean.
- ✅ **R9 · Resume-ability UI + persist approval queue** — `4873812` (durable `pending_approvals`
  store in episodic memory) + `fede962` (host re-issues unresolved approvals on resume) + `1a37038`
  (note-app "Resume last task" button). The host records a guarded action when it holds it for a
  human and resolves it once the human decides; a run that dies mid-approval leaves the row
  unresolved, so on resume — after reseeding completed history — the host drains the prior run's
  unresolved approvals, re-checks the current policy, and re-requests + re-dispatches each held
  action instead of silently dropping it (a second resume won't re-issue). The dispatch→sign→record
  path was extracted into a shared `dispatch_and_record` so re-issued actions are signed + audited
  identically; `run_task` split into a thin wrapper + a testable `run_task_with_memory`. 64 crate
  tests (incl. a seeded-crash resume test) + clippy clean. The reference-app button surfaces the
  already-plumbed `resume: true` (task-manager can mirror it trivially).
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
