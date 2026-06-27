# Licensing & IP posture

> The plan for how kriya is licensed and what's defended. Decision recorded 2026-06-17.
> Strategic *why* lives in [strategy/governed-local-first-wedge.md](strategy/governed-local-first-wedge.md).

## The model: open-core

The idea is public and uncopyrightable; the defense is **execution depth + brand + distribution +
being the default**, not secrecy. License accordingly:

| Layer | Where | License | Why |
|---|---|---|---|
| **SDK / runtime** — `kriya-core`, `kriya-sidecar`, `kriya-inspector`, `create-kriya-app`, the `kriya` crate, examples, tools | **this (public) repo** | **MIT** (already in [`/LICENSE`](../LICENSE)) | Max adoption, zero friction. Devs must be able to drop it in without a license conversation. |
| **On-device control-plane app (paid Console)** — R6 and beyond | **`kriya-console`** — a **separate, private repo** | **Proprietary / All Rights Reserved** (decided 2026-06-17, while purely internal); revisit → **Elastic License v2** at first external source-sharing | Stops a cloud giant from reselling our paid surface as a service, without hurting SDK adoption. |
| **Business playbook** — pricing, customer pipeline, GTM sequencing, `docs/strategy/`, `DECISIONS.md` | **local-only / gitignored** | n/a (not published) | The recipe stays private even though the runtime is open. |

**Yes — the dashboard goes in its own repo (`kriya-console`) with its own (different) license.** You
do not mix two licenses in one repo, and you do not relicense the open SDK. Keep the seam clean: open
SDK ⟂ private console ⟂ private playbook. **Dependency direction is one-way:** `kriya-console`
consumes the public packages (`kriya-core`, `kriya-sidecar`, the `kriya` crate) as published deps —
the public repo never references `kriya-console`.

### Distribution & tiers (D-018)

The product ships as **one download** for the Mac. Installing it sets up the on-device governance
gateway, walks through the macOS permissions, wires the MCP client, and starts tailing receipts —
no separate server to stand up. Tiering is **freemium**: the free tier (live governance monitor,
offline receipt verification, guided setup) is fully usable on its own; a license unlocks the
compliance tier (auditor-ready evidence export + cross-app correlation). The license is **offline /
on-device** — no SaaS, no accounts, no cloud. Note the purchase/issuer path is **not yet live** (a
deferred stub); the free tier stands alone until it is. Receipts land in the standard on-device
location **`~/.kriya/audit/`**, which the app auto-discovers and tails.

## P1 / P2 — public vs private split (decided 2026-06-17, [D-011](DECISIONS.md))

**Dividing line:** anything that runs inside **one app on one machine** to make it governed and
agent-drivable is **public (MIT)**; anything that sits **across many apps / agents / users / an org**
(cross-app correlation, management, compliance reporting, identity, cross-machine fleet policy —
the last is roadmap) is **private (`kriya-console`)**. *The engine is open; the cockpit is paid.*

| Item | Destination | Why |
|---|---|---|
| **R6 · On-device control-plane app** | 🔒 `kriya-console` | The paid surface: cross-app audit viewer, org policy editor, multi-approval routing, budget controls. |
| **R7 · Compliance-evidence export** (SOC 2 / ISO 42001 / EU AI Act) | 🔒 `kriya-console` | Willingness-to-pay hook (EU AI Act enforcement Aug 2026); on-device report generation over the local audit log. |
| **R8 · Identity per action** | ⚠️ split | **Public:** an `actor` field on the signed `Receipt` + richer `caller` (an audit log must record *who*). **Private:** identity management — SSO/OIDC, RBAC, per-user dashboards. |
| **R13 · On-device / no-egress guarantee** | 🌐 public | Runtime mechanism in the `kriya` crate; the *export* of its attestation is the R7 private value-add. |
| **R9 · Resume UI + persist approval queue** | 🌐 public | Single-app runtime hardening + reference-app button (multi-app routing is R6). |
| **R10 · OpenAI backend + retry/backoff** | 🌐 public | Another `Inference`-trait impl in the crate. |
| **R11 · Tamper tests + budget cap** | 🌐 public | Hardens the open audit + verifier + budget module. |

Net: `kriya-console` starts with **R6**, then **R7** and the enterprise half of **R8**; everything
else in P2 stays public and strengthens the free runtime (adoption → funnel).

## What's actually defended (in priority order)

1. **Brand + namespace** — `kriya` on npm / crates.io / GitHub (held), plus the domain and social
   handles. Cheapest real protection. Trademark "kriya" for software once there's revenue.
2. **Execution depth** — the reusable governance batteries (policy engine, budget, signed audit,
   persistent memory, step-through, policy-lint) + the on-device/regulated focus. A fork still has
   to rebuild and maintain all of it.
3. **Distribution / default status** — first, opinionated, cross-ecosystem default way to govern
   on-device agent actions (the Vercel-on-HTTP play).
4. **License speed-bump** — source-available enterprise tier, per the table above.

## Open questions to settle at dashboard build time (R6)

- **Settled 2026-06-17 ([D-011](DECISIONS.md)):** `kriya-console` ships **Proprietary / All Rights
  Reserved** while purely internal (max control, simplest), flipping to **Elastic License v2** at the
  moment source is first shared externally (customer review/escrow, or a public source-available
  tier). BSL 1.1 (time-bombed to Apache, Sentry/MariaDB style) only if a customer needs the
  eventual-open guarantee.
- Whether any SDK piece should move to Apache-2.0 for its explicit patent grant (MIT has none). Only
  matters if patents enter the picture — defer.
