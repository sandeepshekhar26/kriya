#!/usr/bin/env python3
"""Cinematic, screen-recordable demo of governed agent access to Actual Budget via kriya.

Runs entirely on the MOCK fund (ACTUAL_FAKE=1) — no real money, no setup. It drives the
real `kriya-mcp` governor exactly as an MCP client (Claude Desktop / Cursor) would, so what
you see on screen is the genuine governance path, not a mock of it.

Usage:  demo.py <path-to-kriya-mcp> [<path-to-verify-receipts>]
Pacing: DEMO_SPEED env (seconds, default 1.4; set 0 for a fast dry-run).
"""

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
KRIYA_MCP = sys.argv[1] if len(sys.argv) > 1 else "kriya-mcp"
VERIFY = sys.argv[2] if len(sys.argv) > 2 else ""
HANDLER = str(HERE / "dist" / "handler.js")
POLICY = str(HERE / "agent-policy.yaml")
TOOLS = str(HERE / "tools.json")
SPEED = float(os.environ.get("DEMO_SPEED", "1.4"))

B = "\033[1m"; D = "\033[2m"; G = "\033[32m"; R = "\033[31m"
Y = "\033[33m"; C = "\033[36m"; M = "\033[35m"; X = "\033[0m"

AUDIT_LOG = os.path.join(os.environ.get("TMPDIR") or tempfile.gettempdir(), "kriya-audit.jsonl")


def pause(mult=1.0):
    if SPEED:
        time.sleep(SPEED * mult)


def say(s="", mult=1.0):
    print(s)
    sys.stdout.flush()
    pause(mult)


def rule():
    say(f"{D}{'─' * 64}{X}", 0.2)


class Server:
    """A live kriya-mcp governor over stdio, driven like an MCP client."""

    def __init__(self, approval):
        env = dict(os.environ, ACTUAL_FAKE="1")
        self.p = subprocess.Popen(
            [KRIYA_MCP, "--persistent", "--exec", f"node {HANDLER}",
             "--policy", POLICY, "--tools", TOOLS, "--approval", approval],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, bufsize=1, env=env,
        )
        self._id = 0

    def call(self, name, args):
        self._id += 1
        req = {"jsonrpc": "2.0", "id": self._id, "method": "tools/call",
               "params": {"name": name, "arguments": args}}
        self.p.stdin.write(json.dumps(req) + "\n")
        self.p.stdin.flush()
        for raw in self.p.stdout:
            if raw.strip() and json.loads(raw).get("id") == self._id:
                return json.loads(raw)["result"]

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


def action(srv, name, args, show=None):
    say(f"  {C}🤖 agent{X} → {B}{name}{X} {D}{json.dumps(args)}{X}", 0.55)
    result = srv.call(name, args)
    ok = not result.get("isError", False)
    text = result["content"][0]["text"]
    if ok:
        say(f"     {G}✅ allowed{X} · ran · {D}receipt signed → audit log{X}", 0.4)
        if show:
            say(f"     {D}{show(text)}{X}", 0.5)
    else:
        say(f"     {R}⛔ BLOCKED by kriya{X} — {Y}{text}{X}", 0.8)
    say()


def main():
    try:
        os.remove(AUDIT_LOG)  # fresh trail for this run
    except FileNotFoundError:
        pass

    print("\033[2J\033[H", end="")  # clear screen (no TERM dependency)
    say(f"{B}{M}kriya{X} {B}· governed agent access to Actual Budget{X}  {D}(mock fund — no real money){X}", 1.2)
    rule()
    say(f"{D}Actual Budget has no API. We gave an AI agent the ability to operate it —{X}")
    say(f"{D}through typed actions, every one gated on-device. Watch what it can and can't do.{X}", 1.4)
    say()

    # ── Act 1: the agent does the safe, everyday work, unattended ──────────────
    say(f"{B}1 · The agent reconciles your books{X}", 0.8)
    srv = Server("deny")
    n_txns = [0]

    def summarize(text):
        try:
            data = json.loads(text)
            n_txns[0] = len(data)
            return f"sees {len(data)} transactions: " + ", ".join(
                f"{t['payee_name']} ({t['amount'] / 100:.2f})" for t in data)
        except Exception:
            return text[:60]

    action(srv, "list_transactions",
           {"accountId": "acct-checking", "startDate": "2026-06-01", "endDate": "2026-06-30"},
           show=summarize)
    action(srv, "categorize_transaction", {"id": "txn-1", "category": "cat-groceries"})
    action(srv, "set_budget", {"month": "2026-06", "categoryId": "cat-groceries", "amount": 40000})

    # ── Act 2: the agent reaches for something dangerous ───────────────────────
    say(f"{B}2 · The agent tries to touch your money{X}", 0.8)
    action(srv, "delete_transaction", {"id": "txn-2"})
    action(srv, "close_account", {"id": "acct-checking", "transferAccountId": "acct-savings"})
    action(srv, "wire_money", {"to": "stranger", "amount": 999900})  # not even a registered tool
    srv.close()
    say(f"  {D}↑ deny-by-default: destructive moves need a human; unlisted tools are refused outright.{X}", 1.4)
    say()

    # ── Act 3: with your approval, it proceeds — and that's audited too ────────
    say(f"{B}3 · When YOU approve, it proceeds{X} {D}(approval granted){X}", 0.8)
    srv2 = Server("auto")  # stands in for a human clicking "Approve"
    action(srv2, "delete_transaction", {"id": "txn-2"})
    srv2.close()
    say(f"  {D}↑ you stay in control — the gate isn't a wall, it's a checkpoint.{X}", 1.4)
    say()

    # ── Act 4: cryptographic proof of everything that ran ──────────────────────
    say(f"{B}4 · Every action that ran is cryptographically signed{X}", 0.8)
    try:
        with open(AUDIT_LOG) as f:
            for raw in f:
                if not raw.strip():
                    continue
                r = json.loads(raw)
                sig = r.get("signature", "")[:16]
                mark = f"{G}ok{X}" if r.get("success") else f"{R}fail{X}"
                say(f"  {D}receipt{X} {r['action_id']:<22} {mark}  {M}sig {sig}…{X}", 0.35)
    except FileNotFoundError:
        say(f"  {D}(no audit log at {AUDIT_LOG}){X}")
    pause()

    if VERIFY and os.path.exists(VERIFY):
        say()
        say(f"  {D}$ verify-receipts   {X}{D}# offline, on-device — no network{X}", 0.6)
        res = subprocess.run([VERIFY, AUDIT_LOG], capture_output=True, text=True)
        tail = (res.stdout + res.stderr).strip().splitlines()[-3:]
        for l in tail:
            say(f"  {l}", 0.2)
        verdict = (f"{G}✔ all signatures verify — tamper-evident proof of what the agent did{X}"
                   if res.returncode == 0 else f"{R}✗ verification failed{X}")
        say(f"  {verdict}", 1.2)
    say()

    rule()
    say(f"{B}~37 lines of glue. Zero changes to Actual.{X} {D}The agent got capability; you kept control.{X}", 1.0)
    say(f"{M}kriya{X} {D}· github.com/sandeepshekhar26/kriya{X}", 0.5)


if __name__ == "__main__":
    main()
