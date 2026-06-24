# kriya-gateway — govern an MCP server we didn't write, with zero changes

**What this proves, in one sentence:** you can put policy + human approval + a signed,
tamper-evident audit trail *in front of* an MCP server you don't own — changing **zero lines** of
that server — by launching it through the `kriya-gateway` proxy instead of directly.

This is the demo beat: a destructive `tools/call` (`delete_note`) is **stopped at the approval
gate before the downstream server ever sees it**, while a read-only call goes through and comes
back with a **signed receipt** — all on a server whose code we never touched.

> **This is a governance demo, not an "easy MCP" demo.** MCP over stdio is already easy and free —
> the mock server here is ~150 lines of plain Node and speaks MCP fine on its own. What it
> *cannot* do on its own is refuse a destructive call, pause for a human, cap a runaway agent, or
> prove afterward what ran. The gateway adds exactly that, from the outside.

## The architecture (three bullets)

- **client → `kriya-gateway` → downstream server.** The agent's MCP client (Claude Desktop,
  Cursor, Claude Code) launches `kriya-gateway` *as if it were the server*. The gateway spawns the
  real server as a child process and relays JSON-RPC both ways.
- **Governance lives in the middle.** Every `tools/call` runs the unchanged kriya core —
  deny-by-default **policy** → **human approval** for destructive calls → per-minute **budget** →
  Ed25519 **signed, hash-chained audit** — *before* the call is forwarded. Blocked calls never
  reach the downstream server and are never signed.
- **Nothing downstream changed.** [`mock-mcp-server.js`](mock-mcp-server.js) has no governance code
  at all. It is the stand-in for "an app that already ships an MCP server." The gateway adds the
  guardrails entirely from outside it.

```
 MCP client (Claude Desktop)      kriya-gateway  (Rust governor)        downstream server (Node)
 ───────────────────────────      ──────────────────────────────       ────────────────────────
   tools/call list_notes ──MCP──▶ policy: allow → budget ────────────▶ mock-mcp-server.js
                                  → SIGN hash-chained receipt      ◀──  real result
   tools/call delete_note ──MCP─▶ policy: require approval
                                  ⏸  approval gate (deny / asked)      (never reaches downstream)
                                  → NOT signed (you don't sign what didn't run)
```

## Run it (non-interactive — no human, no real agent needed)

```bash
cd examples/gateway-proxy-demo
./run.sh
```

`run.sh` builds the gateway (`cargo build -p kriya --features mcp-client --bin kriya-gateway`,
first run takes a few minutes), then drives a scripted JSON-RPC conversation through it with
`--approval deny` — so the destructive call is auto-blocked at the gate and you see governance
fire **without anyone clicking a button**.

> Need just `node`? The downstream server is **zero-dependency** — no `npm install`. You only need
> the Rust toolchain (pinned 1.90.0) to build the gateway once.

### Expected output (annotated)

```jsonc
// initialize — transparent passthrough; the gateway relays the downstream's capabilities
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{...},"serverInfo":{"name":"mock-notes",...}}}

// tools/list — the downstream's real tools (policy may hide denied ones)
{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"list_notes",...},{"name":"get_note",...},{"name":"create_note",...},{"name":"delete_note",...}]}}

// tools/call list_notes — ALLOWED → forwarded → real result, and it gets SIGNED
{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"note-1: Groceries\nnote-2: Standup\nnote-3: Idea"}],"isError":false}}

// tools/call delete_note — require_approval + --approval deny → BLOCKED, never reaches downstream
{"jsonrpc":"2.0","id":4,"result":{"content":[{"type":"text","text":"Blocked: delete_note requires human approval (denied)."}],"isError":true}}
```

Then `run.sh` `cat`s the audit log. You should see **one** signed receipt — for `list_notes`.
There is **no** receipt for `delete_note`, because a call that was blocked never ran, so there is
nothing to sign:

```jsonc
{"action":"list_notes","outcome":"executed","actor":...,"prev_hash":...,"hash":...,"signature":"<ed25519 hex>","public_key":"<hex>"}
```

> The exact response strings for a blocked call and the exact receipt field names come from the
> gateway binary (built by the kriya core). Treat the shapes above as illustrative; the contract is
> what `kriya-gateway --help` and `docs/SERVICE-ARCHITECTURE.md` §7 confirm.

## Verify a receipt offline (tamper-**evident**, no network)

The audit log is Ed25519-signed and hash-chained, so any later edit or deletion is detectable
offline — no server, no internet:

```bash
cargo run -p verify-receipts -- /path/to/kriya-audit.jsonl   # the path run.sh prints
```

See [`tools/verify-receipts/`](../../tools/verify-receipts/). (It's tamper-**evident**, not
tamper-**proof**: the chain detects tampering; pinning the signing key in a TPM/Secure Enclave is
the hardening on the roadmap — see [`docs/SECURITY.md`](../../docs/SECURITY.md).)

## The live experience (real approval modal, via Claude Desktop)

[`.mcp.json`](.mcp.json) is the drop-in config that points an MCP client at the gateway wrapping
the mock server, with `--approval gui`. With it installed, asking Claude Desktop to *"delete the
Standup note"* pops a real **Approve / Deny** modal before anything happens; *"list my notes"*
just runs and is signed.

```jsonc
{
  "mcpServers": {
    "notes-governed": {
      "command": "/abs/path/to/kriya-gateway",
      "args": ["proxy", "--approval", "gui",
               "--policy", "/abs/path/to/agent-policy.yaml",
               "--audit-log", "/abs/path/to/kriya-audit.jsonl",
               "--", "node", "/abs/path/to/mock-mcp-server.js"]
    }
  }
}
```

> In a real install, `kriya-gateway` is on your `PATH`, so `command` is just `"kriya-gateway"`.
> The absolute paths in the committed `.mcp.json` are for running it straight from this repo.

## The governance policy

[`agent-policy.yaml`](agent-policy.yaml) makes the posture explicit:

| Tool | Policy | Why |
|---|---|---|
| `list_notes`, `get_note` | allow | read-only, safe unattended |
| `create_note` | allow | routine write — everyday agent work |
| `delete_note` | **require approval** | destroys a record — pauses for a human |
| anything else | **deny** | deny-by-default; the agent can't invoke what isn't listed |

Plus a 30-calls/minute budget so a looping agent can't hammer the downstream server. Omit
`--policy` and the gateway falls back to a built-in deny-by-default default (reads allow;
`delete_*`/`remove_*`/`destroy_*` require approval; else deny).

## Honest scope (read this)

- **The proxy wraps apps that already speak MCP.** This demo is Front 1: the target app *already
  exposes an MCP server*; the gateway sits in front of it. That's the headline and it works today.
- **Apps with no MCP and no API are not reached by this proxy.** Those are the job of the
  *reach-in* front (Front 2), which synthesizes a governed MCP server from the OS accessibility
  tree — that is **roadmap (R25), not shipped**, and it's coverage-bounded (it degrades on
  custom-drawn / Electron / Qt UIs). Computer-use over pixels (Front 3) is a deferred fallback.
  Do not assume either works today.
- **Tamper-evident, not tamper-proof** (see verify section above).

## Files

| File | What |
|---|---|
| [`mock-mcp-server.js`](mock-mcp-server.js) | the "app we didn't write" — a zero-dependency stdio MCP server, no governance of its own |
| [`agent-policy.yaml`](agent-policy.yaml) | the governance policy the gateway enforces |
| [`.mcp.json`](.mcp.json) | drop-in config: launch the gateway wrapping the mock server (live `--approval gui`) |
| [`run.sh`](run.sh) | non-interactive end-to-end proof (builds the gateway, drives JSON-RPC, shows the blocked call + signed receipt) |

## Learn more

- [`docs/SERVICE-ARCHITECTURE.md`](../../docs/SERVICE-ARCHITECTURE.md) — the gateway design: one
  governance core, three reach fronts, and the product spec (§7).
- [`examples/actual-budget-bolt-on/`](../actual-budget-bolt-on/) — the in-process `wrapAction`
  bolt-on (the enterprise-depth, non-bypassable topology) on a real finance app.
