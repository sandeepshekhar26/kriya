# Flagship demo — governed agent access for Actual Budget, in <50 lines

This is the bolt-on that *is* the pitch: take [**Actual Budget**](https://actualbudget.org) — a
real, shipped, open-source, **local-first** personal-finance app — and give a frontier agent
(Claude Desktop, Cursor, …) the ability to drive it, **without touching Actual's code**, with
permission, human approval, budget, and a signed audit trail enforced on-device.

Why Actual is the perfect target: it has **no HTTP API**. It ships an in-process npm package
(`@actual-app/api`) backed by a local SQLite budget. There is no endpoint for a cloud agent to
hit — the only way to drive it is an **in-process action layer**, and because the data is your
**money**, that access has to be **governed where the data and the human are**. That's the
whole thesis in one app.

<p align="center">
  <img src="demo.gif" alt="kriya governing an agent driving Actual Budget" width="760">
  <br><em>The governed flow (mock fund). Reproduce it with <code>./demo.sh</code> — see <a href="DEMO.md">DEMO.md</a>.</em>
</p>

## The entire integration

[`src/actions.ts`](src/actions.ts) — **~37 lines** — wraps six of Actual's existing functions as
governed, agent-callable actions. No rewrite, no new API:

```ts
wrapAction(actual.updateTransaction, {
  id: "categorize_transaction",
  description: "Assign a category to a transaction (the everyday reconciliation task).",
  parameters: { id: str, category: str },
  permissions: ["write:transactions"],
  mapParams: (p) => [p.id, { category: p.category }],
});
```

(`kriya wrap src/actual-api.ts` would scaffold most of this for you — see the codemod.)

## How it runs

```
 Claude Desktop / Cursor          kriya-mcp  (Rust governor)        this bolt-on (Node)
 ───────────────────────          ─────────────────────────       ───────────────────
   tools/call categorize ──MCP──▶ policy → approval → budget ──▶  actual.updateTransaction(...)
                                  → SIGN audit receipt        ◀──  result
   tools/call delete_txn  ──MCP──▶ policy says: needs a human
                                  ⏸  approval gate (denied/asked)  (never reaches Actual)
```

`kriya-mcp` speaks MCP to the agent and owns governance; the handler holds the (expensive)
Actual connection open and runs only cleared actions. `--persistent` keeps that one connection
alive across calls.

## The governance story (the whole point)

The policy in [`agent-policy.yaml`](agent-policy.yaml):

| Action | Policy | Why |
|---|---|---|
| `list_accounts`, `list_transactions` | allow | read-only, safe unattended |
| `categorize_transaction`, `set_budget` | allow | routine reconciliation — the everyday agent work |
| `delete_transaction` | **require approval** | destroys a record |
| `close_account` | **require approval** | moves money between accounts |
| anything else (`wire_money`, …) | **deny** | deny-by-default; the agent can't invoke what isn't listed |

Plus a 30-actions/minute budget so a looping agent can't hammer your finances, and an
Ed25519-signed receipt appended for every action that actually runs.

## Try it now (no real budget needed)

`ACTUAL_FAKE=1` swaps in an in-memory budget so you can see the full governed flow without
installing Actual or setting up data.

> **Want the cinematic, screen-recordable version?** Run `./demo.sh` (builds everything on first
> run, then plays a paced four-beat walkthrough — reconcile → blocked → approved → signed proof).
> Recording guide, shot list, and voiceover: [DEMO.md](DEMO.md).

```bash
# from the repo root
npm run build --workspace kriya-core      # the SDK this imports
cd examples/actual-budget-bolt-on && npm install && npm run build
cargo build -p kriya --bin kriya-mcp --release   # the governor

# drive it like an MCP client would:
ACTUAL_FAKE=1 printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"categorize_transaction","arguments":{"id":"txn-1","category":"cat-groceries"}}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"delete_transaction","arguments":{"id":"txn-2"}}}' \
| ACTUAL_FAKE=1 path/to/kriya-mcp \
    --persistent --exec "node $(pwd)/dist/handler.js" \
    --policy agent-policy.yaml --tools tools.json --approval deny
```

You'll see `categorize_transaction` run and get a signed receipt, while `delete_transaction`
is blocked pending approval — governance, with zero changes to Actual.

## Wire it into Claude Desktop (the real demo)

> For the **full real-app visual demo** — a real budget in Actual's UI, updating live as the agent
> works, with the approval prompt on camera — follow [REAL-DEMO.md](REAL-DEMO.md) (sets up
> `actual-server`, the real `@actual-app/api`, sync, and the shot list). Quick version below.

Install Actual's API and point the MCP server at a real budget:

```bash
npm install @actual-app/api      # the real, in-process Actual API
```

`claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "actual-budget": {
      "command": "/abs/path/to/kriya-mcp",
      "args": [
        "--persistent",
        "--exec", "node /abs/path/to/examples/actual-budget-bolt-on/dist/handler.js",
        "--policy", "/abs/path/to/examples/actual-budget-bolt-on/agent-policy.yaml",
        "--tools", "/abs/path/to/examples/actual-budget-bolt-on/tools.json",
        "--approval", "tty"
      ],
      "env": {
        "ACTUAL_DATA_DIR": "/abs/path/to/actual-data",
        "ACTUAL_SERVER_URL": "http://localhost:5006",
        "ACTUAL_PASSWORD": "your-sync-password"
      }
    }
  }
}
```

Now in Claude Desktop: *"categorize my uncategorized June transactions"* runs through the
governed server; *"delete that duplicate transaction"* pauses for your approval first
(`--approval tty` prompts on the terminal; `deny` refuses outright; `auto` is for trusted/test
only). Every action that runs is in the signed audit log.

## Files

| File | What |
|---|---|
| [`src/actions.ts`](src/actions.ts) | **the bolt-on** — `wrapAction` over Actual's functions |
| [`src/actual-api.ts`](src/actual-api.ts) | typed slice of `@actual-app/api` + runtime loader |
| [`src/handler.ts`](src/handler.ts) | holds the connection; the persistent `--exec` handler |
| [`src/fake-actual.ts`](src/fake-actual.ts) | in-memory budget for `ACTUAL_FAKE=1` demo mode |
| [`agent-policy.yaml`](agent-policy.yaml) | the governance policy |
| [`tools.json`](tools.json) | generated MCP tool schemas (`npm run dump`) |
