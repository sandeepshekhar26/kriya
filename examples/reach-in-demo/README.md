# Reach-in demo — govern a real app that has NO MCP server and NO API

This points kriya-gateway's **Front 2 (reach-in)** at a *running* macOS app and synthesizes governed
MCP tools from the app's **accessibility tree** — zero changes to the app. It's both a "try it on a
real app" demo and the **coverage-measurement** harness for the R25 reach-in front.

> macOS only. Requires the Accessibility permission granted to your terminal:
> **System Settings → Privacy & Security → Accessibility → enable your terminal app.**

## 1. See what tools an app exposes (read-only)

Open the app first, then:

```bash
./probe.sh "Calculator"
./probe.sh "Actual Budget"
./probe.sh "Spent"
```

Output reports the total synthesized tools, how many are directly **`press_*`** (the operable
controls), and a breakdown by action verb. Example (Calculator):

```
total synthesized tools : 383
directly-pressable       : 134   (press_* — the operable controls)
sample: press_button_7, press_button_add, press_button_equals, …
```

## 2. Drive a governed action

Use a restrictive policy so governance is real, and pick a tool name from step 1. Example — let an
agent press the "7" key but **block** anything destructive, with a signed audit trail:

```bash
GW=../../crates/kriya/target/debug/kriya-gateway
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"press_button_7","arguments":{}}}' \
  | "$GW" reach-in --app "Calculator" \
      --approval gui \                 # destructive calls raise the macOS approval modal
      --policy ./governed-policy.yaml \ # your real policy (allow some, gate/deny the rest)
      --signing-key ~/.kriya/key.bin --audit-log ~/.kriya/audit.jsonl \
      --actor my-agent
```

With `--signing-key`, the log opens with a signed **on-device attestation**, then a signed receipt
per performed action. Verify any receipt offline:

```bash
cargo run -p verify-receipts -- ~/.kriya/audit.jsonl
```

## Which of your apps suits which front

| Your app | Best front | Why |
|---|---|---|
| **Native AppKit/SwiftUI** (Calculator, Notes, a native POS/CRM) | **reach-in** (this demo) | rich AX tree → many clean `press_*` tools |
| **Actual Budget, VS Code, Slack** (Electron) | **proxy** if it has an MCP server; else expect **sparse reach-in** | Electron renders web content → AX exposes few actions. Running `./probe.sh "Actual Budget"` *measures* exactly that. |
| **App that already speaks MCP / has the `wrapAction` bolt-on** (Spent, the Actual Budget bolt-on) | **proxy** (`kriya-gateway proxy -- <its mcp server>`) — see `examples/gateway-proxy-demo/` | the app already exposes typed tools; the proxy adds governance with zero changes |

**Honest scope:** reach-in is macOS-only today, needs the Accessibility grant, and **degrades on
Electron/Qt/web-rendered UIs**. The probe is how you find out, per app, whether reach-in or the
proxy is the right front. The "any app" claim is not true — coverage is per-app.
