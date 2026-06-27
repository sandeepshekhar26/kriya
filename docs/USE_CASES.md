# Use Cases — Why local, API-less software needs a governed agent layer

> **Purpose:** the sales/positioning doc. One place that answers "who needs kriya, for what,
> and why the obvious alternatives don't work." Grounded in the governed local-first wedge thesis.
> Use it for the website, pitch, cold outreach, and the landing-page copy.
>
> **The one-line frame:** *Agents are becoming users of software. For apps whose capabilities
> live only inside a running local process — no cloud API, private data — the only way an agent
> can use them safely is a governed action layer inside the app. That's kriya.*

---

## The problem, in one paragraph

Users are starting to delegate to agents: "agent, do X in my app." If the app has a good cloud
API and non-sensitive data, the agent just calls the API — you don't need us. But a huge class of
software has **no API to wrap**: the capability *is* the running app — an in-memory handler over
local files, local devices, private/regulated data. For those, the agent's only options today are
**(a) screen-scraping / accessibility-tree control** (computer-use: slow, flaky, ungoverned,
unauditable) or **(b) a full rewrite to expose a cloud API** (impossible for local/private data).
kriya is the third option, in two shapes: **download the Console app** to wrap any existing MCP
agent (Claude Desktop, Cursor, …) with policy, approval, and signed audit in zero integration code;
or for greenfield apps, use the **SDK** to expose the app's **real typed actions** to any agent over
MCP. Either way you get **permission, human approval, budget, and signed audit built in, on-device.**

## Why the alternatives fail (put this on the landing page)

| Approach | Why it breaks for local/API-less apps |
|---|---|
| **Wrap your REST API with MCP** | There is no API. The capability is an in-process handler. Nothing to wrap. |
| **Screen-scrape / computer-use** | Slow, breaks on every UI change, can't be permissioned or audited, can't prove what it did. Unacceptable for money/health/legal data. |
| **Send the data to a cloud agent** | The data is local, private, or regulated — legally or contractually it *can't* leave the device. |
| **Hand-build governance per app** | Every team re-implements policy + approval + budget + audit + memory. Most ship without it → an unsafe agent surface. kriya is buy-not-build. |

---

## Who needs this (ICP segments + concrete use cases)

### 1. Personal finance & accounting (local-first money apps) — the flagship
**Examples:** Actual Budget, GnuCash, Tiller-style local ledgers, indie bookkeeping apps.
**Why local/API-less:** local SQLite, often client-side encrypted, no public HTTP API
(`@actual-app/api` is an *in-process* package).
**Agent use cases:**
- Auto-categorize and reconcile transactions overnight.
- "Find every subscription I'm paying for and flag duplicates."
- Draft next month's budget from the last 6 months of spend.
**Why governance is non-negotiable:** the data is *money*. The agent may categorize/reconcile but
**cannot move money or delete a transaction without on-device human approval** — every action
signed and auditable. This is the visceral demo (R5).

### 2. Point-of-sale & retail ops
**Examples:** offline-first POS, inventory, register systems (the planner is building one).
**Why local/API-less:** all transactions/inventory live in-app, on local hardware, often offline;
no public API by design.
**Agent use cases:**
- "Reorder anything below threshold from the usual supplier" (drafts the order, human approves).
- End-of-day reconciliation and variance reports.
- Natural-language inventory edits ("mark the damaged case of #4412 as shrinkage").
**Why governance:** price changes, refunds, voids, and inventory write-offs are theft/fraud
surfaces. Each needs a permission tier + audit trail; high-value ones need approval.

### 3. CRM & vertical desktop tools with private records
**Examples:** local/desktop CRMs, case-management tools, field-service apps.
**Why local/API-less:** customer PII lives locally or in a private store; no public API, or one too
limited to drive the app.
**Agent use cases:**
- "Summarize every interaction with this account and draft a follow-up."
- Bulk-update records from a meeting transcript.
- Flag stale deals and prep outreach.
**Why governance:** PII access must be permissioned and logged (GDPR/CCPA). Bulk edits and external
sends need approval to prevent an agent from mass-mailing the wrong list.

### 4. Regulated workstations (the willingness-to-pay segment)
**Examples:** healthcare (EHR/imaging clients), legal (doc review), finance (trading/risk
terminals), gov/defense (air-gapped).
**Why local/API-less:** data legally cannot leave the device/network; the software is a thick local
client; agents must operate *in place*.
**Agent use cases:**
- Legal: "Pull every clause about indemnification across these 200 local documents."
- Health: pre-populate a chart from notes — but never submit an order without a clinician's approval.
- Finance: an agent prepares a trade ticket; a human must confirm before it's placed.
**Why governance:** mandatory. An agent touching regulated data **must** be permissioned + audited,
and that can only happen in-app, on-device. EU AI Act enforcement opens Aug 2026 — signed audit +
compliance-evidence export (R7) is the buy reason.

### 5. Creative & knowledge tools (local-first notes, PKM, media)
**Examples:** Obsidian/Logseq/Joplin-style apps, local DAWs, photo/video editors.
**Why local/API-less:** the vault/library is local files; the value is in-app operations on them.
**Agent use cases:**
- "Reorganize my vault by topic and fix broken links" (proposes a diff, human approves).
- Batch-edit metadata across a local media library.
**Why governance:** lower stakes than money/health, but bulk file mutation still wants a preview +
approval gate so an agent can't silently rewrite a year of notes. (Recognizable but lower-stakes —
a good adoption funnel, not the enterprise sale.)

### 6. Near-future agentic SaaS / new local-first AI apps (the developer wedge)
**Examples:** greenfield AI desktop apps being built right now.
**Why:** these teams *will* add "let an agent operate the app" soon. kriya is the SDK that's ready
when they do — `create-kriya-app` for greenfield, the same `registerAction` handler for human
clicks and agent calls.
**Agent use cases:** whatever the app does — the point is the app ships agent-operable *and*
governed from day one instead of bolting on an unsafe surface later.
**Why governance:** building it in is one line per action (`requires: approval`, budget, audit);
retrofitting safety after a breach is a rewrite.

---

## The recurring shape (why every one of these is the same sale)

Across all six segments the pattern is identical:
1. **The capability has no cloud API** — it's an in-process handler. → in-process layer is the only mechanism.
2. **The data is local / private / high-stakes.** → access must be governed where the data is.
3. **The same handler serves a human click and an agent call** — one implementation, two callers.
4. **Read-ish actions flow; mutating/high-value actions gate** on permission → approval → budget,
   and *everything* is signed and auditable.

That's the whole product: **MCP moves the call; kriya is the safety the wire leaves out.**

**Two ways to get it, both on-device.** Download the **Console app** and you're governing any MCP
agent on your Mac in minutes — it installs the gateway, walks you through the macOS permissions, and
wires your MCP client, with zero integration code. Receipts land in a standard on-device location
(`~/.kriya/audit/`) and the app tails them automatically — open it and you're watching live
governance, no import, no log-hunting. It's **free to monitor and verify**; a license unlocks the
compliance tier (auditor-ready evidence export + cross-app correlation). For greenfield local-first
apps, the **SDK** builds the same governance in from day one. No SaaS, no accounts, no cloud.

## What to say when someone asks "why not just use [X]?"

- *"Why not MCP?"* — kriya isn't an alternative to MCP; it's the governed runtime you put *behind*
  it. MCP moves calls and has a client-mediated confirm prompt; it does **not** ship the policy
  engine, budget, signed audit, persistent memory, or app-mediated on-device approval. kriya does.
- *"Why not computer-use / a screen agent?"* — it can't be permissioned, can't be audited, breaks
  on UI changes, and you can never prove what it did. For money/health/legal that's a non-starter.
- *"Why not just expose an API?"* — for local/private data you legally can't, and the capability is
  in-process anyway. There's nothing to host.

## Honest limit (keep this — credibility)

An app with a **good cloud API and non-private data does not need kriya** — the agent uses the API.
The target is the **intersection**: local + in-process + private/regulated + no good API path.
Don't sell outside the seam; that's where governance-only competitors and funded MCP gateways win.
