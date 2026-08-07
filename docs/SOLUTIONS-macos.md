# Kriya Gateway on macOS — solutions guide (tombstone)

> **This guide was removed on 2026-08-07.** It documented the macOS **desktop-reach lanes** —
> `kriya-gateway reach-in` (typed tools synthesized from an app's accessibility tree) and
> `kriya-gateway computer-use` (a governed system-wide pixel tool set), plus the `router`
> composition and the `doctor` Accessibility preflight. Those lanes were removed from the library
> to keep it focused on **govern/audit** — kriya governs what agents do and proves it with signed,
> offline-verifiable receipts; it no longer drives the desktop itself.

## Where the removed content lives

- The full guide, the lane implementations (`mcp::reachin`, `mcp::computeruse`), the gateway
  subcommands, and the `examples/reach-in-demo/` walkthrough are all in **git history** (last
  commit before 2026-08-07).
- The last **released** build carrying the lanes is crates.io **`kriya 0.1.4`** (they were behind
  the non-default `reach-in` / `computer-use` / `router` features).

## What still works on macOS

- **`kriya-hook`** — governs everything Claude Code does through tools (and `kriya-hermes-hook`
  for Hermes): policy → approval → budget → Ed25519-signed receipts, all on-device.
- **`kriya-gateway proxy`** — the zero-change stdio governance proxy in front of any MCP server;
  **`kriya-gateway broker`** — one endpoint multiplexing N MCP upstreams under one governor;
  **`kriya-gateway run`** — the macOS Seatbelt egress-containment lane.
- **`kriya-llm-proxy`** — the governed reverse proxy in front of a local inference server.
- The **signed `.app` bundle** packaging (`scripts/macos/build-gateway-app.sh`). The engineering
  finding that motivated it stands as historical record: macOS TCC keys a durable grant to a
  signed bundle's stable `CFBundleIdentifier`, never to a loose binary. See
  [`scripts/macos/README.md`](../scripts/macos/README.md).

Every governed action still produces a hash-chained, tamper-**evident** (not tamper-proof),
offline-verifiable receipt — that guarantee is unchanged.
