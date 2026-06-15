# Real-app visual demo — Actual's UI on screen, driven by an agent

This is the heavier take: a **real Actual Budget** with a real budget, the app **visible in a
browser**, and a frontier agent (Claude Desktop) operating it through `kriya-mcp` while you watch
the UI update — and watch kriya **block** the money-moving actions until you approve.

Unlike the mock demo (`./demo.sh`), this needs a running Actual server and the real
`@actual-app/api`. Budget once, record many times.

## How the pieces fit

```
 Browser: Actual web app  ◀──sync──▶  actual-server (localhost:5006)  ◀──sync──▶  kriya handler
        (you watch this)                                                          (@actual-app/api)
                                                                                       ▲
 Claude Desktop ──MCP──▶ kriya-mcp (governance: policy/approval/budget/audit) ──exec──┘
```

The handler mutates the budget and `sync()`s to the server after each write; the open web app
pulls those changes on its next sync, so you see the agent's edits appear live.

## 1. Run Actual + make a budget

```bash
# a local Actual server (serves the web app + sync at http://localhost:5006)
npx @actual-app/sync-server
# (or Docker: docker run --rm -p 5006:5006 actualbudget/actual-server:latest)
```

In the browser at **http://localhost:5006**:
1. Set a **server password** (you'll pass it as `ACTUAL_PASSWORD`).
2. Create a budget named **Demo**; add an account (e.g. "Checking") and a handful of
   transactions, leaving a few **uncategorized** (those are what the agent will categorize).
3. Copy the **Sync ID**: Settings → **Show advanced settings** → *Sync ID* (a UUID).

## 2. Install the real Actual API in this example

```bash
cd examples/actual-budget-bolt-on
npm install @actual-app/api      # real, in-process API (native deps) — not needed for the mock
npm run build
```

## 3. Point the handler at the real budget

Set these (no `ACTUAL_FAKE`!). `ACTUAL_DATA_DIR` is a scratch cache dir that must exist:

```bash
mkdir -p /tmp/kriya-actual-data
export ACTUAL_SERVER_URL="http://localhost:5006"
export ACTUAL_PASSWORD="<your server password>"
export ACTUAL_DATA_DIR="/tmp/kriya-actual-data"
export ACTUAL_SYNC_ID="<budget Sync ID from step 1>"
# export ACTUAL_FILE_PASSWORD="..."   # only if the budget is end-to-end encrypted
```

Smoke-test the real path before involving Claude (drive it like an MCP client):

```bash
cargo build -p kriya --release --bin kriya-mcp     # the governor
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_transactions","arguments":{"accountId":"<acct-id>","startDate":"2026-06-01","endDate":"2026-06-30"}}}' \
| /path/to/kriya-mcp --persistent --exec "node $(pwd)/dist/handler.js" \
    --policy agent-policy.yaml --tools tools.json --approval deny
```
You should get your real transactions back. (`list_accounts` first if you need the account id.)

## 4. Wire Claude Desktop (the real agent)

`claude_desktop_config.json` — absolute paths, real env, `--approval tty` so you can approve on
camera:

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
        "ACTUAL_SERVER_URL": "http://localhost:5006",
        "ACTUAL_PASSWORD": "<your server password>",
        "ACTUAL_DATA_DIR": "/tmp/kriya-actual-data",
        "ACTUAL_SYNC_ID": "<budget Sync ID>"
      }
    }
  }
}
```
With `--approval tty`, the approve/deny prompt appears in the **terminal Claude Desktop spawned
the server from** — keep a terminal visible, or run `kriya-mcp` yourself and point Claude at it.

## 5. The shot (≈90 s)

Frame the **browser (Actual)**, **Claude Desktop**, and the **approval terminal** on screen.

1. Show the budget — a few uncategorized transactions.
2. Claude: *"Categorize my uncategorized transactions."* → agent calls `categorize_transaction`
   (✅, signed). **Switch to the browser** — the categories appear (Actual pulls the synced change;
   refresh/focus the tab if it hasn't auto-synced yet).
3. Claude: *"Delete the Shell transaction."* → kriya **pauses for approval** in the terminal.
   Pause on camera — *this is the moment*. Approve → it deletes, the row disappears in the app.
   (Run it again and **deny** to show the hard block, if you want both.)
4. Cut to the audit trail: `verify-receipts` → every action the agent took, signed and verified.

### Gotchas
- **Live update lag:** the web app syncs on focus / a short interval. If a change doesn't show,
  click into the app or refresh — the handler already `sync()`d it to the server.
- Account/category **IDs**: the agent works in IDs. Either let it `list_accounts` /
  `list_transactions` first (it'll discover them), or pre-seed the prompt.
- Keep `--approval tty` for the live human-in-the-loop beat; `deny` for a hard block; never `auto`
  for a real-money demo.

For the lightweight, fully reproducible version, see [DEMO.md](DEMO.md) (mock fund, `./demo.sh`).
