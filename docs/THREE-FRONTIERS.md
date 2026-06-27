# The three frontiers of "agent meets software" — and the one nobody's building for

> **The canonical positioning essay for kriya — the theme of this whole project.** This is the
> in-repo copy of the published article; the canonical version (and the one to share) lives on
> Medium:
> [medium.com/@sandeepshekhar26/the-three-frontiers-of-agent-meets-software…](https://medium.com/@sandeepshekhar26/the-three-frontiers-of-agent-meets-software-and-the-one-nobodys-building-for-a7cafda13715).
> By Sandeep Kumar, June 2026. Kept here so the project's *why* is readable from a fresh clone and
> stays anchored to the wedge ([D-009](DECISIONS.md), [strategy/](strategy/)). If the two ever
> diverge, the published article is canonical for prose; [D-009](DECISIONS.md) is canonical for the bet.
>
> **Correction (2026-06-24, [D-015](DECISIONS.md)):** this in-repo copy has been corrected ahead of
> the Medium version. The original framed tier 3 as "desktop has no tooling standard / stuck with
> screenshots." That undersells **MCP-stdio**, the original local-first transport — desktop apps
> *can* expose typed tools today. The honest, defensible gap on tier 3 is **governance, not
> tooling**. The published article still carries the old framing and should be updated to match.

> *Web apps got WebMCP. Cloud APIs got MCP over HTTP. Desktop apps got MCP over stdio — the
> original, local-first transport. So all three can already expose typed tools to an agent. What a
> desktop app with private data still can't do is **govern** the agent that drives it. That gap
> matters more than you think.*

By mid-2026, AI agents' interaction with software is splitting into three distinct tiers. For
*tooling* — how an agent calls an app's functions — a standard has emerged on all three (and on
desktop it's older than most people remember). For *governance* — permissioning, approving,
budgeting, and auditing what the agent then does — only two tiers have a natural owner, and the
third, which handles the most sensitive data, has none on-device. This is a map of where those
lines are drawn, and why the governance gap on the third tier is the one that matters.

## The three surfaces

An agent can reach software through exactly three surfaces.

**1. The browser (web apps) → WebMCP.** Google and Microsoft are advancing **WebMCP**, a proposed
W3C standard that entered a Chrome origin trial in June 2026. Instead of an agent scraping the DOM
and guessing what buttons do, the web developer declares structured tools — JavaScript functions
and HTML forms with machine-readable descriptions the agent reads directly.

**2. Cloud APIs (SaaS) → MCP over HTTP/SSE.** The **Model Context Protocol**, now stewarded by the
Linux Foundation, wraps backend APIs as agent-callable tools. Adoption is real and large: ~97M
monthly SDK downloads, 86,000+ GitHub stars, 9,600+ servers in the registry, and 41% of surveyed
organizations reporting production use (Stacklok). But "MCP = cloud" is a common slip: MCP defines
**two transports**, and the remote HTTP/SSE one is the *newer* of the pair.

**3. Desktop & local apps → MCP over stdio (the transport people forget).** MCP's *original*
transport is **stdio**: the agent's client spawns the tool server as a local subprocess and talks to
it over standard in/out. It was designed for exactly this case — and it is how Claude Desktop and
Cursor already launch local MCP servers today. So an app with **in-process handlers** — no REST
endpoint, local/private or regulated data: personal finance, POS, CRM clients, regulated
workstations — **can** ship an stdio MCP server and expose real typed tools to an agent, no
screenshots involved. **Screenshotting the screen and guessing where to click is the fallback for an
app nobody has instrumented — not the ceiling for desktop apps.** So the honest gap on this tier
isn't *tooling*. It's *governance* — and that's the rest of this piece.

## Why typed actions beat screenshots — on every tier

Typed actions vs. vision is not a gentle tradeoff. A **Reflex benchmark (May 2026)** ran both
approaches on identical tasks with Claude Sonnet: the vision agent burned **~551,000 input tokens
across 53 steps (~17 minutes)**; the typed-action agent used **~12,000 tokens in 8 calls (~20
seconds)**. That's roughly **45× more tokens and 50× slower** for the same result.

It's a single vendor-published benchmark with high variance — but the *direction* (vision costs
dramatically more in tokens, time, and accuracy than typed actions) is uncontested across the
literature; conservative personal estimates still land at 10–20×. Typed actions are cheaper
*because* the model reasons over meaning, not pixels. This is the whole reason a desktop app should
expose stdio MCP tools rather than leave the agent squinting at the screen — and the reason
screenshots are the floor, not the frontier.

## The governance problem none of the three tiers has solved

Even MCP — the most mature tier — has not solved governance, and that's the most concerning part:

- MCP's own roadmap lists **Enterprise Readiness** as "the least defined" of its priorities.
- The **NSA's AI Security Center (May 2026)** warned that "MCP's rapid adoption has outpaced its
  security model."
- **CoSAI** identified nearly **40 MCP-specific threats across 12 categories**, 7 of them
  "particularly insidious." Traditional API security falls short because the LLM acts as a
  "non-deterministic, manipulable router."
- **Gartner** projects 40% of enterprise apps will embed task-specific agents by end-2026 — and that
  **40% of enterprises will decommission autonomous agents by 2027** over governance gaps found in
  production.

## Desktop apps: a structural problem, not just a missing feature

For a local app handling regulated data, the vision-based approach is a compliance dead-end. You
**cannot**:

- **permission** a screenshot-and-click agent — it can do anything a user could;
- **audit** what it did — there are no signed receipts, only "the model said it clicked here";
- **gate** a high-risk action for human approval; or
- **prove** compliance — no audit trail, no policy enforcement.

And the clock is real (if a bit further out than this essay first said): the EU AI Act's **high-risk
(Annex III) obligations were deferred to December 2, 2027** by the Digital Omnibus (agreed May 2026;
Annex I embedded systems → Aug 2028), with non-compliance fines up to **€15M / 3% of worldwide
turnover** (the 7% / €35M tier is reserved for *prohibited* practices). The requirement is unchanged:
an agent touching regulated data legally must be permissioned and audited.

## The missing piece

What a local app actually needs is a way to **expose its real typed actions to agents with
governance built in** — permission, human approval, budget, and a signed audit trail — all enforced
**on-device, by the host process, with no network hop**. Because for local apps with private data,
governance *cannot* live in the cloud: it has to live where the data and the user are.

## kriya

That's the layer I'm building: **kriya** — the on-device control plane for AI agents on your Mac.
You download one app: it installs the governance gateway, walks you through the macOS permissions,
wires your MCP client, and shows live, signed proof of everything an agent does. It works with any
MCP agent (Claude Desktop, Cursor, …), and nothing leaves your machine. Under the hood is an
open-source (MIT) runtime that speaks **MCP** outward over the same **stdio** transport an agent
already knows — so the typed-tool half is nothing exotic. The part a raw stdio server leaves out is
the governance: with kriya each action runs through **permission → human approval → budget → a
signed audit log**, on-device, before it touches your data. Receipts land in a standard on-device
location and the app tails them automatically — open it and you're watching live governance, no
import, no log-hunting.

The proof: kriya was bolted onto **Actual Budget** — a local finance app with *no HTTP API* — in
**~37 lines**. The agent auto-categorizes transactions (each signed), but **cannot delete or move
money without in-app approval**; unlisted actions are refused; a per-minute budget is enforced.

## Current state of play (honest)

- **WebMCP** — origin trial, ~96 open issues; adoption beyond Google properties unproven.
- **MCP** — massive, but governance-immature; core enterprise features not shipped.
- **Desktop** — one GA product (Microsoft Copilot Studio), still vision-based; the typed-action,
  governed approach is early.
- **The on-device-governance contest is real and growing.** Don't oversell "nobody's here":
  endpoint and platform players are converging on the laptop — Maxim AI's **Bifrost Edge**,
  **CrowdStrike** ("the endpoint as the epicenter for AI security"), **Microsoft Defender**'s local
  AI-agent coverage, and Microsoft's open-source **Agent Governance Toolkit**. But each governs from
  *outside* the app: at the OS/endpoint layer or as a cloud sidecar. None reaches *inside* a no-API
  app's own process to gate the app's real, typed handlers with an in-app approval the user sees in
  *that* app. That seam — the sole gatekeeper between the agent and a resource with no other door —
  is the part still unoccupied.
- **kriya** — alpha, no production users; the pattern works, APIs may still change.

The structural truth doesn't change: **a local app whose data has no other door cannot be governed
from the outside.** When the app is the only path to the resource, the host inside it is the only
place an "the agent cannot bypass this" guarantee can actually hold — so governance has to live
on-device, where the data and the users are. That's the frontier kriya is for.

**Download the app (macOS).** It's free to monitor and verify: live governance monitor, offline
receipt verification, and guided setup — no SaaS, no accounts, no cloud. Everything runs on-device.

---

*Reproduced in-repo from the published essay for offline/agent reference. Figures (Reflex 45×/50×,
MCP adoption, NSA/CoSAI, Gartner 40%/40%, EU AI Act Aug 2 2026 / 7%) are as cited in the original;
some could not be independently re-verified and are reproduced as the article's own claims — see
[research notes in strategy/](strategy/) and the README's regulatory section for sourcing caveats.*
