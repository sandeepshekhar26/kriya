# Recording the kriya demo (mock fund)

Two ways to record the flagship demo. Both use the **mock fund** (`ACTUAL_FAKE=1`) — no real
money, no Actual install, no API key. Pick one:

- **A. Scripted terminal demo** (`./demo.sh`) — fully reproducible, ~70 s, ideal for a clean
  recording and a README GIF. **Recommended for the first video.**
- **B. Live Claude Desktop** — a real frontier agent driving it, most convincing, but less
  scripted. Use once A is in the can.

---

## A. Scripted terminal demo

```bash
cd examples/actual-budget-bolt-on
DEMO_SPEED=0 ./demo.sh     # dry-run first (no pauses) to pre-build + sanity check
./demo.sh                  # the real take (DEMO_SPEED defaults to 1.4s pacing)
```

First run builds the SDK + `kriya-mcp` + `verify-receipts` (a minute or two); later runs are
instant. Tune `DEMO_SPEED` (e.g. `1.6` calmer, `1.0` snappier).

### What it shows (the four beats)
1. **The agent reconciles your books** — lists transactions, categorizes, sets a budget; each
   ✅ allowed and signed.
2. **The agent tries to touch your money** — `delete_transaction`, `close_account`, and a
   never-registered `wire_money` are all ⛔ blocked (approval required / deny-by-default).
3. **When you approve, it proceeds** — the same delete runs once approved.
4. **Cryptographic proof** — the signed receipts, then `verify-receipts` confirms every
   signature offline.

### Recording tips
- Terminal: ~100×32, large font (18–22pt), high-contrast dark theme. The script uses color.
- For a GIF/cast: [`asciinema rec demo.cast`](https://asciinema.org) then `agg demo.cast demo.gif`,
  or just QuickTime screen-record the terminal window.
- Run the dry-run once so all builds are cached — the real take then has zero build noise.

### Voiceover / captions (~70 s)
> "Actual Budget is a real, local-first finance app — and it has no API. So how do you safely
> let an AI agent operate it? **(beat 1)** With kriya, the agent reconciles your books on its
> own — categorizing, budgeting — every action signed. **(beat 2)** But when it reaches for
> something dangerous — deleting a transaction, moving money — kriya stops it. It physically
> cannot, without you. Anything not on the policy is refused outright. **(beat 3)** Approve it,
> and it proceeds — you're the checkpoint, not a bystander. **(beat 4)** And every action that
> ran is cryptographically signed and verifiable offline. **(close)** That's ~37 lines of glue,
> zero changes to Actual. The agent gets capability; you keep control. That's kriya."

---

## B. Live Claude Desktop (real agent, mock fund)

Point Claude Desktop's MCP config at `kriya-mcp` with `ACTUAL_FAKE=1` so it drives the in-memory
budget. Use absolute paths and a release `kriya-mcp` (`cargo build -p kriya --release --bin kriya-mcp`).

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
      "env": { "ACTUAL_FAKE": "1" }
    }
  }
}
```

Then on camera, type to Claude:
1. *"What transactions do I have this month, and categorize the groceries one."* → runs, signed.
2. *"Delete the Shell transaction."* → kriya pauses for approval (`--approval tty` prompts in the
   terminal running `kriya-mcp`; approve on camera to show the in-the-loop moment, or `deny` to
   show the block).
3. Show the audit log / `verify-receipts` to close.

> Note: with `--approval tty`, keep the terminal running `kriya-mcp` visible — that's where the
> approve/deny prompt appears. Swap to `--approval deny` if you want to show a hard block with no
> prompt, or `auto` only for an unattended dry-run.
