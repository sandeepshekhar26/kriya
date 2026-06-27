# Launch Plan — Show HN

> Roadmap item **R12**. Gated on the planner's go, and on R1–R5 + publishing being done.
> "HN" = Hacker News (news.ycombinator.com); "Show HN" is its category for things you built.
> A strong Show HN is read by YC partners — directly load-bearing for the YC goal.

## Decide first: launch now, or after the MCP-bridge (R1)?

**Resolved — both gates cleared.** The downloadable **Kriya Console** (drop-in governance gateway
for any MCP agent) and the Actual Budget proof are shipped, so the strong-hook version of the
pitch is available now: "download one app, point it at an MCP agent you already run, and watch
every call get checked, approved, and signed — on your Mac, nothing leaves your machine." Launch
on the Console framing (Title #1, opening comment below). The old "launch now vs. wait" tradeoff
is moot; the only remaining gates are the pre-launch checklist.

## Pre-launch checklist (hard requirements — do not skip)

- [x] All packages published ([PUBLISHING.md](PUBLISHING.md)) — `kriya-core` 0.0.1, `kriya-sidecar`
      0.0.1, `kriya-inspector` 0.3.0, `create-kriya-app` 0.2.0 (npm), `kriya` crate 0.1.0
      (crates.io). *(A P0.5 republish of core/sidecar at 0.0.2 + crate 0.1.1 is staged but optional —
      see PUBLISHING.md; not a launch blocker.)*
- [x] Scaffolder template swapped to the published crate (PUBLISHING.md step 5) — done & verified:
      a freshly scaffolded app `cargo check`s clean against crates.io `kriya 0.1.0` and `npm install`
      resolves `kriya-core` from the public registry; `create-kriya-app` 0.2.0 (crate-based template)
      is live, so `npm create kriya-app@latest` serves it.
- [ ] Repo public: `gh repo edit sandeepshekhar26/kriya --visibility public --accept-visibility-change-consequences` *(planner — needs gh auth)*
- [x] **End-to-end smoke (build level):** fresh `create-kriya-app` → `cargo check` + `npm install`
      both green against published artifacts, zero manual fixups (the brotli/alloc-no-stdlib gotcha
      did **not** bite a fresh resolve). **Planner action remaining:** one visual `npm run tauri dev`
      launch on a clean account (the 0.2.0 template is already live).
- [ ] README has a **15–30s GIF** of the inspector driving an agent (use Kap / gifski). Static
      shots underperform. *(planner — record the Actual Budget before/after; still unrecorded)*
- [ ] CI green on `main` (badge in README). A red badge on day one is fatal. *(planner — verify via gh)*
- [x] LICENSE (MIT ✓) + CONTRIBUTING.md (✓) + issue templates (✓) present.
- [x] Rename decided — public name is **kriya** ("agent-native" is Builder.io's).

## Soft but high-leverage

- [ ] 90-second Loom/YouTube walkthrough linked in README.
- [ ] The flagship before/after demo (R5) as the centerpiece GIF — stock local app → agent
      driving it through governed actions.

## Title (pick one)

1. `Show HN: Kriya Console – drop-in governance for any MCP agent, runs on your Mac` (lead)
2. `Show HN: Kriya – every MCP agent call checked, approved, and signed — on your Mac` (sharp diff)
3. `Show HN: Kriya Console – watch live, signed proof of everything an agent does, on-device`

Rules: no caps, no emoji, < 80 chars. **Pick #1** — it leads with the downloadable product
(Console) and the drop-in, vendor-neutral, on-device promise, which is the load-bearing pitch.

## Opening comment (post within 30s of submitting, as the first comment)

> Hey HN — I'm Sandeep. Kriya is the on-device control plane for AI agents on your Mac.
>
> You download one app — **Kriya Console**. It installs the governance gateway, walks you through
> the macOS permissions, wires your MCP client (Claude Desktop, Cursor, …), and then shows you
> **live, signed proof of everything an agent does**. Nothing leaves your machine.
>
> The point: native MCP commoditized the *transport*, not the *enforcement*. Approval in the spec
> is a SHOULD-not-MUST and lives client-side; auth is optional and skips stdio entirely. So the
> Console drops a zero-change governance gateway in front of any MCP server you already run. Every
> call is:
>
> - checked against **deny-by-default policy** — unlisted actions are refused;
> - **paused for human approval** when it matters (destructive calls hold on an approval modal;
>   timeout = deny);
> - **rate-limited** by a per-minute budget — a runaway agent hits the cap and stops;
> - written to a **signed, tamper-evident receipt** you can verify offline, without trusting me.
>
> Zero integration code. Receipts land in a standard on-device location (`~/.kriya/audit/`) and
> the Console **auto-tails** them — you open the app and you're already watching live governance,
> no import, no log-hunting.
>
> **Free / paid, no SaaS:** the free tier is the full live governance monitor, offline receipt
> verification, and the guided setup — everything above. A license unlocks the compliance tier:
> auditor-ready evidence export and cross-app correlation. No accounts, no cloud, no upload —
> everything runs on-device, and the license is offline too.
>
> The flagship demo: I pointed the Console at **Actual Budget** — a real, local-first, no-HTTP-API
> finance app — with no rewrite. An agent categorizes transactions (allowed, signed), but
> `delete_transaction` and `close_account` hold for human approval, unlisted actions are refused,
> and there's a per-minute budget. [GIF]
>
> If you want to go deeper than the drop-in gateway, there's a build-time path too — apps can
> declare typed actions (`registerAction` / `wrapAction`) so the *same* handler serves a human
> click and an agent call, no screenshots or DOM scraping. macOS-first, alpha. AMA on the design —
> especially whether the governance layer is drawn at the right boundary.

*(Swap bullets to match what's actually shipped at launch. The "Why not just MCP?" answer below
is now your lead, not a defense — the Console adds the enforcement native MCP leaves out.)*

## Anticipated objections — pre-built answers

**"Why not just MCP?"** — MCP is great, and the Console works with any MCP agent. But native MCP
governs the *transport*, not the *enforcement*: approval is a SHOULD-not-MUST and lives
client-side, auth is optional and skips stdio. The Console drops in front of any MCP server you
already run and adds the missing layer — deny-by-default policy, human approval, budget, and a
signed, verifiable receipt — with zero integration code.

**"Is it a cloud service?"** — No. The Console runs entirely on your Mac — no accounts, no
sign-up, no upload. Receipts are written to a standard on-device location the app tails locally,
and the offline verifier confirms them without contacting any server. Even the license is
offline. Nothing about an agent's activity leaves your machine.

**"How is this different from browser-use / computer-use?"** — Those drive the app *as a human*
(pixels, clicks, DOM). kriya doesn't — the app declares typed actions the agent calls directly.
Faster (no screenshot RTT), deterministic (no flaky selectors), debuggable (params + reasoning
+ signed receipt per call), safer (policy gates before execution).

**"Copyable in a weekend?"** — The code, sure. The *combination* — typed-action protocol +
permission/approval/budget/audit/memory + inspector + scaffolder + working reference apps +
MCP-bridge — is months of integration and taste, and we keep moving. Moat = depth + breadth +
first.

**"Only works if the developer cooperates."** — Yes, that's the point. kriya is for apps built
*with* agents as first-class users, not retrofitting agents onto arbitrary apps. For the
retrofit case, use the computer-use/accessibility-tree stack.

**"Tests? Production users?"** — 34 SDK + 13 host-crate tests, green on CI, integrated in two
reference apps. No production users yet — alpha, MIT, please break it and tell me how.

## Timing

- **Best:** Tue/Wed, 7:30–9:30am Pacific. Be online the **first 2 hours** to reply to every
  comment (early engagement drives ranking). If you can't be online, don't post.
- **Avoid:** Fri afternoon, weekends, US holidays, and any day with a big Apple/Google/Anthropic
  launch. Glance at the HN front page first; if 3 AI dev tools are already trending, wait.

## Distribution after HN (in order)

1. **Twitter/X thread** (draft below) — 1 GIF/screenshot per tweet; tag @AnthropicAI in the
   *last* tweet only. Pin it.
2. **Lobste.rs** ~4h after HN if it's going well — tags: `programming`, `practices`, `security`.
3. **Reddit** with per-sub titles: r/macapps (on-device governance Console), r/LocalLLaMA
   (local MCP governance), r/ClaudeAI (drop-in governance for Claude Desktop).
4. **Direct outreach** day 1–2: Tauri core devs, Anthropic devrel (MCP-compatibility), 2–3
   dev-tool podcast hosts — mention the HN post.

## Twitter/X thread (draft)

1/ I built Kriya Console — the on-device control plane for AI agents on your Mac. Download one
app: it installs a governance gateway, walks you through the macOS permissions, wires your MCP
client, and shows live, signed proof of everything an agent does. Nothing leaves your machine. [link]

2/ Works with any MCP agent — Claude Desktop, Cursor, whatever you already run. Drop the gateway
in front, zero integration code. Native MCP governs the transport, not the enforcement; this adds
the enforcement. [GIF: wiring an agent]

3/ Every call is checked against deny-by-default policy, paused for human approval when it
matters, rate-limited by a per-minute budget, and written to a signed, tamper-evident receipt.
[screenshot: approval modal]

4/ Receipts land in a standard on-device location and the Console auto-tails them — open the app
and you're already watching live governance. No import, no log-hunting. [GIF: live monitor]

5/ Verify any receipt offline, without trusting me — the proof stands on its own. No accounts,
no cloud, no upload. [snippet: offline verify]

6/ Free to monitor and verify: the live governance monitor, offline verification, and guided
setup are the free tier. A license unlocks the compliance tier — auditor-ready evidence export
and cross-app correlation. No SaaS; the license is offline too.

7/ The flagship: I pointed the Console at Actual Budget (a real local-first finance app) with no
rewrite — an agent categorizes transactions (signed), but deletes and account-closes hold for
human approval, and there's a per-minute budget. macOS-first, alpha. /cc @AnthropicAI

8/ Want to go deeper than the drop-in gateway? There's a build-time path: apps declare typed
actions so the same handler serves a human click and an agent call — no screenshots, no DOM
scraping. Feedback welcome, especially on where the governance boundary should sit. [link]

## Day-of / week-after

- First 2h: reply to every comment. Be brief; don't argue — link the source.
- First 24h: any reproducible bug → patch and ship within 24h. Visible momentum compounds.
- Week-1 metrics that matter: GitHub stars (curve, not vanity), npm downloads, **any external
  repo importing `kriya-core`** (the real leading indicator), issue quality.
- Metrics that don't: Twitter likes, HN position after front page, hot takes from non-readers.
