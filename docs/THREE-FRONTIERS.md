# The three frontiers of "agent meets software" — and the one nobody's building for

> **The canonical positioning essay for kriya — the theme of this whole project.** This is the
> in-repo copy of the published article; the canonical version (and the one to share) lives on
> Medium:
> [medium.com/@sandeepshekhar26/the-three-frontiers-of-agent-meets-software…](https://medium.com/@sandeepshekhar26/the-three-frontiers-of-agent-meets-software-and-the-one-nobodys-building-for-a7cafda13715).
> By Sandeep Kumar, June 2026. Kept here so the project's *why* is readable from a fresh clone and
> stays anchored to the wedge ([D-009](DECISIONS.md), [strategy/](strategy/)). If the two ever
> diverge, the published article is canonical for prose; [D-009](DECISIONS.md) is canonical for the bet.

> *Web apps are getting WebMCP. Cloud APIs have MCP. Desktop apps with no API? They're still stuck
> with screenshots. That gap matters more than you think.*

By mid-2026, AI agents' interaction with software is splitting into three distinct tiers — each with
a different standard emerging, or none at all. This is a map of where the lines are drawn, and why
the third tier is the one that matters for the apps that handle the most sensitive data.

## The three surfaces

An agent can reach software through exactly three surfaces.

**1. The browser (web apps) → WebMCP.** Google and Microsoft are advancing **WebMCP**, a proposed
W3C standard that entered a Chrome origin trial in June 2026. Instead of an agent scraping the DOM
and guessing what buttons do, the web developer declares structured tools — JavaScript functions
and HTML forms with machine-readable descriptions the agent reads directly.

**2. Cloud APIs (SaaS) → MCP.** The **Model Context Protocol**, now stewarded by the Linux
Foundation, wraps backend APIs as agent-callable tools. Adoption is real and large: ~97M monthly SDK
downloads, 86,000+ GitHub stars, 9,600+ servers in the registry, and 41% of surveyed organizations
reporting production use (Stacklok).

**3. Desktop & local apps → ❌ nothing.** Apps with **in-process handlers** — no REST endpoint,
local data, private or regulated information: personal finance, POS, CRM clients, regulated
workstations. Today an agent can only reach these by **screenshotting the screen and guessing where
to click.**

## Why the gap matters: the cost of "seeing"

Typed actions vs. vision is not a gentle tradeoff. A **Reflex benchmark (May 2026)** ran both
approaches on identical tasks with Claude Sonnet: the vision agent burned **~551,000 input tokens
across 53 steps (~17 minutes)**; the typed-action agent used **~12,000 tokens in 8 calls (~20
seconds)**. That's roughly **45× more tokens and 50× slower** for the same result.

It's a single vendor-published benchmark with high variance — but the *direction* (vision costs
dramatically more in tokens, time, and accuracy than typed actions) is uncontested across the
literature; conservative personal estimates still land at 10–20×. Typed actions are cheaper
*because* the model reasons over meaning, not pixels.

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

And the clock is real: the **EU AI Act's high-risk obligations take effect August 2, 2026**, with
penalties up to **7% of worldwide annual turnover**. An agent touching regulated data legally must
be permissioned and audited.

## The missing piece

What a local app actually needs is a way to **expose its real typed actions to agents with
governance built in** — permission, human approval, budget, and a signed audit trail — all enforced
**on-device, by the host process, with no network hop**. Because for local apps with private data,
governance *cannot* live in the cloud: it has to live where the data and the user are.

## kriya

That's the layer I'm building: **kriya** — an open-source (MIT) framework (Rust agent host +
TypeScript SDK + React inspector). Each action runs through **permission → human approval → budget →
a signed audit log**, on-device, while speaking **MCP** outward so any agent can drive it.

The proof: kriya was bolted onto **Actual Budget** — a local finance app with *no HTTP API* — in
**~37 lines**. The agent auto-categorizes transactions (each signed), but **cannot delete or move
money without in-app approval**; unlisted actions are refused; a per-minute budget is enforced.

## Current state of play (honest)

- **WebMCP** — origin trial, ~96 open issues; adoption beyond Google properties unproven.
- **MCP** — massive, but governance-immature; core enterprise features not shipped.
- **Desktop** — one GA product (Microsoft Copilot Studio), still vision-based; the typed-action,
  governed approach is early.
- **kriya** — alpha, no production users; the pattern works, APIs may still change.

The structural truth doesn't change: **local apps with private data cannot be governed from the
outside.** Governance has to live on-device, where the data and the users are. Right now, almost
nobody is building that layer. That's the frontier kriya is for.

---

*Reproduced in-repo from the published essay for offline/agent reference. Figures (Reflex 45×/50×,
MCP adoption, NSA/CoSAI, Gartner 40%/40%, EU AI Act Aug 2 2026 / 7%) are as cited in the original;
some could not be independently re-verified and are reproduced as the article's own claims — see
[research notes in strategy/](strategy/) and the README's regulatory section for sourcing caveats.*
