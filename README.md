# kriya — the on-device control plane for AI agents on your Mac

> **Download one app.** It installs the governance gateway, walks you through the macOS permissions,
> wires your MCP client, and shows live, signed proof of everything an agent does. **Nothing leaves
> your machine.** Receipts land in a standard on-device location (`~/.kriya/audit/`) and the app
> tails them automatically — open it and you're watching live governance, no import, no log-hunting.
>
> Works with any MCP agent (Claude Desktop, Cursor, …). Every call is checked against deny-by-default
> policy, paused for human approval when it matters, and written to a signed, tamper-evident receipt —
> zero integration code.

**Get it:**
[⬇ Kriya Console for macOS](https://github.com/sandeepshekhar26/kriya/releases/tag/console-v0.1.0)
(free, signed + notarized, universal) ·
[⬇ `kriya-audit` verifier](https://github.com/sandeepshekhar26/kriya/releases/tag/audit-v0.1.0)
(free, macOS + Linux) ·
[Govern Claude Code in 5 minutes](https://kriyanative.com/docs/claude-code/)

**Don't trust us — check (60 seconds):**

```sh
curl -fsSLO https://github.com/sandeepshekhar26/kriya/releases/download/audit-v0.1.0/kriya-audit-0.1.0-macos-universal.zip
unzip -o kriya-audit-0.1.0-macos-universal.zip
curl -fsSLO https://github.com/sandeepshekhar26/kriya/releases/download/audit-v0.1.0/sample-receipts.jsonl
./kriya-audit sample-receipts.jsonl        # 20 signature(s) verified — OK
sed '1s/list_transactions/X/' sample-receipts.jsonl > tampered.jsonl
./kriya-audit tampered.jsonl               # FAIL — signature does not match (exit 1)
```

## Calling is solved. Governing isn't.

MCP already standardized how an agent *calls* software — stdio for local apps (how Claude Desktop and
Cursor spawn local servers), HTTP for cloud, WebMCP for the browser. That part is commodity and free.
What it deliberately leaves out is **enforcement**: human approval is a client-side *should*, and
stdio gets no auth at all.

So the missing piece isn't tooling — it's a **control plane**: a checkpoint between the agent and your
data that enforces **deny-by-default policy, human approval on the calls that matter, budgets, and a
signed, tamper-evident audit** — one the agent can't bypass and the client can't disable. It matters
most for the apps holding your most sensitive data: a local POS terminal, a finance tool, a healthcare
workstation. Those can't be governed from the *outside* — there's no cloud API to wrap, no external
door to enforce at. The control plane has to live where the data and the human are: **on the device.**

kriya is that control plane. Point any MCP agent at it and every call is checked, paused for approval
when it matters, and signed — on your machine, with nothing leaving it.

<p align="center">
  <img src="examples/actual-budget-bolt-on/demo.gif" alt="an AI agent operating Actual Budget through kriya — routine actions run and are signed, money-moving ones are blocked pending approval, every receipt verifiable" width="760">
  <br><em>An agent operating <a href="https://actualbudget.org">Actual Budget</a> through kriya: routine actions run and are signed; money-moving ones are blocked pending approval; every action verifies offline.</em>
</p>

## kriya-gateway — govern any MCP server, zero changes

**The fastest way in is the Kriya app** (macOS) — download it; it installs the gateway, requests the
macOS permissions it needs, and wires your MCP client (Claude Desktop, Cursor, Claude Code) for you,
then opens a live cockpit where you watch each governed call get approved and signed. No config files,
no manual paths — and because receipts land in `~/.kriya/audit/`, the app auto-discovers and tails
them the moment they appear.

If an app **already exposes an MCP server**, you don't have to bolt anything *into* it to govern it.
Every `tools/call` now passes deny-by-default policy → **human approval** for destructive calls →
budget → an Ed25519 **signed, hash-chained audit** — *before* it reaches the real server. **Zero
lines changed in that server.**

MCP over stdio is already easy and free — that's not the value. The value is the **approval modal
firing on a destructive call**, and the **signed, tamper-evident receipt** afterward, on a server
**you didn't write**.

**Power-user / manual path** — drive the same gateway from the command line instead:

```bash
# wrap any existing MCP server — your real server command goes after the `--`
kriya-gateway proxy -- node your-mcp-server.js
```

…or drop it straight into a client's MCP config (Claude Desktop, Cursor, Claude Code):

```jsonc
{
  "mcpServers": {
    "notes-governed": {
      "command": "kriya-gateway",
      "args": ["proxy", "--approval", "gui", "--", "node", "your-mcp-server.js"]
    }
  }
}
```

A runnable, no-human, end-to-end proof (a read goes through and is signed; a destructive call is
blocked at the gate and is not) lives in
[`examples/gateway-proxy-demo/`](examples/gateway-proxy-demo/) — `./run.sh`. The design is one
governance core + a **4-tier reach model**, described next.

### Reaches every app — governed (the 4-tier model)

One governance core, the reach picked by how the app exposes itself, richest first:
**bolt-on/serve** (a kriya-instrumented app's real named handlers) → **proxy** (any MCP server,
zero changes) → **reach-in** (typed tools synthesized from the macOS accessibility tree, for apps with
no MCP/API — shipped, macOS) → **computer-use** (system-wide pixels: click/type/scroll/screenshot —
shipped, macOS; the universal floor, so no app is unsupported). Governance *richness* is tiered by
instrumentation: semantic deny/approve of a **named** action at the top, coarse click/keystroke
gating at the floor.

> **Honest scope.** A named-action policy needs an instrumented app; reach-in is coverage-bounded
> (degrades on Electron/Qt/web UIs); computer-use is universal but its governance is **coarse**
> (gates/audits clicks & keystrokes, not named actions) and needs Accessibility + Screen Recording.
> The audit trail is tamper-**evident** (hash-chained, offline-verifiable), not tamper-proof.
> **Vs an ungoverned computer-use agent (e.g. Cowork):** the differentiator is that every action is
> policy-gated + signed + on-device + **vendor-neutral** (governs *any* MCP agent, under the app
> owner's rules) — not the ability to drive apps.

### Govern the whole Claude Code lane — `kriya-hook`

`kriya-hook` governs **everything Claude Code does through tools**: its native tools (`Bash`,
`Edit`, `Write`, …) *and* every MCP server attached to it (`mcp__<server>__<tool>` calls). Servers
added straight to Claude Code never pass a gateway — the hooks seam is the one place that sees them
all, with zero per-server config (the snippet sets no `matcher`, so it fires for every tool). The
gateway remains the seam for other MCP clients (Claude Desktop, Cursor, …):

```bash
cargo install kriya --bin kriya-hook --no-default-features
```

…then one paste into `~/.claude/settings.json`:

```jsonc
{
  "hooks": {
    "PreToolUse":  [{ "hooks": [{ "type": "command", "command": "kriya-hook pre" }] }],
    "PostToolUse": [{ "hooks": [{ "type": "command", "command": "kriya-hook post" }] }]
  }
}
```

`pre` is the **gate** (policy → optional human approval; a blocked call exits 2 with the reason, and
the blocked *attempt* is itself a signed receipt), `post` is the **record** (an Ed25519, hash-chained
receipt of what actually ran). MCP calls keep their full name — `mcp__github__create_issue` becomes
the governed action `claude-code__mcp__github__create_issue` — so **per-server policy is one prefix
glob** (`- { action: "claude-code__mcp__github__*", allow: true, require_approval: true }`).
Receipts land in `~/.kriya/audit/claude-code.jsonl` under a persistent
signing identity, the chain spans hook invocations, and the same offline verifiers — `kriya-audit`,
[`tools/verify-receipts`](tools/verify-receipts/), the 5-language bindings — re-prove them without
trusting this repo. The default policy is **record-only** (nothing blocks until you author rules);
budgets need the long-running gateway, and hooks are a cooperative seam — whoever controls
`settings.json` controls them (use managed settings org-wide). This works the same when Claude Code
runs against **your own AWS Bedrock** (`CLAUDE_CODE_USE_BEDROCK=1`): the model stays in your cloud
boundary, the action evidence stays on your machine.

## Why "kriya"?

**kriya** (Sanskrit, क्रिया) means *action* — and, in grammar, *verb*. That is the whole idea: an
agent shouldn't squint at your pixels, it should **act through your app's verbs**. The unit it
works through is the *kriya* — a single typed, governed action. We bind agents to **actions**, not
to screenshots. Same word, same bet: software you operate by *doing*, whether you're a human or a
machine.

## One app, two doors

```
Human  ──clicks a button──┐
                          ├──▶  the same typed action  ──▶  your handler ──▶ state ──▶ UI
Agent  ──calls an action──┘        (governed: permission · approval · budget · audit)
```

You declare each capability once — `registerAction(...)` for a new app, or `wrapAction(...)` to
adopt one you already have. The agent never simulates a human: it calls the typed action directly,
and it **can't** bypass the gates, because the *host* (not the agent) owns the policy and the
signing key.

## How it fits together

Every path in — a human click, your local agent, or an outside agent over MCP — lands on the same
registry and runs the same gauntlet before it ever touches your data:

```mermaid
flowchart TB
    subgraph HOST[kriya host - on-device, the agent cannot bypass]
      direction TB
      REG[Action registry: registerAction or wrapAction] --> PERM{Permission?}
      PERM -->|deny| NO[Blocked]
      PERM -->|needs approval| APPR[Human approval: Approve or Deny]
      APPR -->|denied| NO
      PERM -->|allow| BUD[Budget: per-minute cap]
      APPR -->|approved| BUD
      BUD --> RUN[Run the registered handler]
      RUN --> SIGN[Sign Ed25519 receipt]
    end

    H([Human clicks a button]) --> REG
    A([Local agent calls an action]) --> REG
    EXT([External agent - Claude Desktop, Cursor]) -->|MCP| MCP[kriya-mcp governed server]
    MCP --> REG

    RUN --> STATE[(App state)]
    STATE --> UI[UI re-renders]
    SIGN --> AUDIT[(Append-only audit log)]
    RUN --> MEM[(Memory: SQLite, across runs)]

    STATE -. structured state .-> A
    MEM -. recall .-> A
    INF[Inference: deterministic / claude-cli / ollama / anthropic] -. picks next action .-> A
```

The agent's *entire* view of your app is the right-hand side: **structured state** plus a typed
**menu of actions**. It never sees a pixel. The left-hand side — permission, approval, budget,
signing — is enforced in the host process, so a misbehaving or jailbroken agent still can't get
past it.

## What you get

- **Typed actions, not pixels.** Declare a capability once; humans click it, agents call it, both
  run the same handler. The agent reasons over structured state and a typed tool schema — fast and
  reliable, and it doesn't break when you restyle a button.
- **Governance, built in.** Every action an agent proposes runs this gauntlet on-device, before it
  executes:
  1. **Permission** — a deny-by-default policy: allow / require-approval / deny.
  2. **Human approval** — guarded actions pause for an Approve/Deny decision in *your* app's UI.
  3. **Budget** — a per-minute action cap *and* a per-hour inference-call cap stop a runaway or
     looping agent (the second bounds model cost, not just action bursts).
  4. **Signed audit** — an Ed25519 receipt per action → append-only log, verifiable offline.

  Plus persistent **memory** across runs, policy **linting**, and **step-through** debugging.
- **Speaks MCP.** Your actions become MCP tools; the governed `kriya-mcp` server lets any external
  agent drive your app — with every call routed *through* the gates, not around them.
- **Cross-shell, cross-language.** Runs in a Tauri backend, or as a standalone `kriya-host` sidecar
  that Electron, plain Node, **and Python** apps drive over stdio — governance in a process the
  renderer can't tamper with. One Rust host, a binding per language (TS + Python today; .NET/JVM
  next).

## Go deeper: instrument your own app

The download-the-app path above governs any MCP agent with zero integration code. When you own the
app and want the **deepest, enterprise tier** — semantic deny/approve of your app's *named* actions,
not just generic MCP calls — you instrument it directly. Two ways in:

**Build a new local-first agent app:**
```bash
npm create kriya-app@latest my-app    # Tauri 2 + React + Rust host, safety layer pre-wired
```

Then declaring a capability is one small block — the *same* handler your button already calls:

```ts
import { registerAction, str } from "kriya-core";

registerAction({
  id: "create_note",
  description: "Create a new note with a title and content.",
  parameters: { title: str, content: str },
  permissions: ["write:notes"],            // policy decides: allow / require approval / deny
  handler: ({ title, content }) =>          // ← your ordinary business logic, nothing special
    db.notes.insert({ title, content }),
});
```

That's the whole contract. A human clicks **New note**; an agent calls `create_note` — both run
the exact same `handler`, and the agent's call still passes permission → approval → budget → audit
on the way in. The agent discovers it automatically (kriya turns it into a typed tool schema), so
you write app logic, not agent plumbing. Adding the next capability is one more `registerAction`.

**Bolt onto an app you already have** — wrap a function it *already exposes*, no rewrite:
```ts
import { wrapAction } from "kriya-core";

wrapAction(actual.updateTransaction, {
  id: "categorize_transaction",
  description: "Assign a category to a transaction.",
  parameters: { id: str, category: str },
  mapParams: (p) => [p.id, { category: p.category }],
});

wrapAction(actual.deleteTransaction, {
  id: "delete_transaction",
  description: "Permanently delete a transaction.",
  parameters: { id: str },
  mapParams: (p) => [p.id],          // policy: require_approval — pauses for a human
});
```

That snippet is the demo above: [`examples/actual-budget-bolt-on/`](examples/actual-budget-bolt-on/)
gives a frontier agent governed access to [Actual Budget](https://actualbudget.org) — a real,
local-first finance app with **no HTTP API** — in ~37 lines, without changing Actual's code.
(`kriya wrap <file>` scaffolds the wrappers from your exported functions.)

> **Fewer lines — and far fewer tokens.** A published benchmark (Reflex, May 2026) ran vision-based
> and typed-action agents on the same task with Claude Sonnet: vision needed **551,000 input tokens
> across 53 steps** (~17 min); typed actions needed **12,000 tokens in 8 calls** (~20 sec) —
> **~45× more tokens and ~50× slower**. In kriya's own Actual Budget demo the ratio is similar:
> categorizing a transaction costs ~700 tokens via a typed action vs. ~8,000–15,000 tokens when
> screenshot-and-clicking the same edit. Typed actions are cheaper *because* the model reasons over
> meaning, not pixels.
> <br><sub>(Benchmark: Reflex, May 2026; kriya estimate via Anthropic's documented image-token formula.)</sub>

## What's in the box

| Package / crate | What |
|---|---|
| [`kriya-core`](packages/core/) | TypeScript SDK — `registerAction`, `wrapAction`, validation, MCP/JSON-Schema export, the `kriya` CLI (`dump`, `wrap`) |
| [`kriya-sidecar`](packages/sidecar/) | Node/TS binding — host the runtime from Electron or plain Node over stdio |
| [`kriya-inspector`](packages/inspector/) | Drop-in React dev inspector — step log, approval modal, memory replay |
| [`create-kriya-app`](packages/create-kriya-app/) | Scaffolder for a new local-first agent app |
| [`kriya`](crates/kriya/) | Rust agent host — step loop, swappable inference, permissions, budget, signed audit, memory, **governed MCP-server mode** |
| [`kriya` (Python)](bindings/python/) | **Python binding** — `register_action` / `wrap_action` + the `Host` stdio driver, for PyQt/PySide/Tk apps, FreeCAD/Blender plugins, data & quant tools (`pip install kriya`) |

**Binaries:** `kriya-mcp` (governed MCP server — external agents drive your app through the gates) ·
`kriya-host` (the stdio sidecar) · [`tools/verify-receipts`](tools/verify-receipts/) (offline audit-log verifier).

**Reference apps:** [`apps/note-app`](apps/note-app/) and [`apps/task-manager`](apps/task-manager/)
— two domains on the one shared host crate.

## Quick start

Try the governed bolt-on with zero setup (in-memory budget, no real data):

```bash
cd examples/actual-budget-bolt-on && ./demo.sh   # builds everything on first run, then plays it
```

Or run the reference desktop app:

```bash
npm install
npm run build --workspace kriya-core
npm run tauri dev --workspace note-app   # first run compiles the Rust backend (a few min)
```

Pick the inference backend with `AGENT_BACKEND` (`deterministic` default, or `claude-cli` /
`ollama` / `anthropic`).

## Docs

- [architecture.md](architecture.md) — how the pattern works, end to end
- [docs/SECURITY.md](docs/SECURITY.md) — how the signed audit trail is tamper-evident (and an honest threat model)
- [docs/ROADMAP.md](docs/ROADMAP.md) — what's built and what's next
- [docs/PRODUCT_GAPS.md](docs/PRODUCT_GAPS.md) — honest feature-completion tracker
- [docs/CONTINUE.md](docs/CONTINUE.md) — copy-paste prompt to advance the roadmap by one item

## Why this matters now

- **EU AI Act** high-risk (Annex III) obligations were **deferred to December 2, 2027** by the
  Digital Omnibus (agreed May 2026; Annex I embedded systems → Aug 2028) — but the requirement is
  unchanged: agents touching regulated data need auditable, governed interfaces, with non-compliance
  fines up to **€15M / 3%** of worldwide turnover (the 7% / €35M tier is for *prohibited* practices).
- **Gartner** projects 40% of enterprise apps will embed AI agents by end of 2026 — and 40% of
  enterprises will demote or decommission autonomous agents by 2027 due to governance gaps.
- The **NSA AI Security Center** (May 2026) warned that MCP's rapid adoption has outpaced its
  security model; a **CoSAI** whitepaper identified nearly 40 MCP-specific threats across 12
  categories.
- For desktop/local apps, the only GA product is Microsoft Copilot Studio — still vision-based,
  still ungoverned. That's the gap kriya fills.

## One app, two tiers

It's a single download with two tiers — no SaaS, no accounts, no cloud; **everything runs
on-device.**

- **Free** — the live governance monitor, offline receipt verification, and guided setup. Fully
  usable on its own: download free and you're watching each governed call get approved and signed,
  and you can verify any receipt without trusting the vendor.
- **Compliance tier** *(license-gated)* — unlocks **auditor-ready evidence export** and **cross-app
  correlation**: one trustworthy view of what every agent did across all your governed apps,
  exported as evidence for an auditor. Tampered or forged entries are flagged. The license is an
  offline license — no SaaS, no accounts, no cloud.

The paid tier never changes how governance works — it consumes the same Ed25519-signed receipts and
the same policy the free monitor already verifies and enforces. Tamper-evident audit plus
correlated, exportable evidence is what **EU AI Act** (enforcement opens Aug 2026) and **SOC 2** ask
for when an agent touches real data — on-device, where cloud MCP gateways structurally can't reach.

> Cross-app correlation across the apps on one machine has shipped. A cloud / cross-*machine* fleet
> console is on the roadmap, not a current deliverable.

Enterprise & regulated deployments → [kriyanative.com](https://kriyanative.com) ·
Sandeepshekhar26@gmail.com.

## Status

Alpha. The pattern, the cross-shell runtime, and the full safety layer work end-to-end — typed
actions, governed MCP-server mode, the Electron/Node sidecar, the `wrapAction` bolt-on, and the
Actual Budget flagship are all shipped. APIs may still change before a stable release. MIT licensed.
