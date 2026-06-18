#!/usr/bin/env python3
"""Screen-recordable demo of governed agent access to a Replicad CAD model via kriya.

It drives the real `kriya-mcp` governor exactly as an MCP client (Claude Desktop / Cursor) would,
so what you see is the genuine governance path. Geometry is real Replicad/OpenCascade unless
CAD_FAKE=1 is set (analytic fallback, for a faster dry-run / CI).

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
    say(f"{D}{'─' * 66}{X}", 0.2)


class Server:
    """A live kriya-mcp governor over stdio, driven like an MCP client."""

    def __init__(self, approval):
        self.p = subprocess.Popen(
            [KRIYA_MCP, "--persistent", "--exec", f"node {HANDLER}",
             "--policy", POLICY, "--tools", TOOLS, "--approval", approval],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, bufsize=1, env=dict(os.environ),
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
            self.p.wait(timeout=8)
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

    kernel = "analytic (CAD_FAKE)" if os.environ.get("CAD_FAKE") else "Replicad / OpenCascade"
    print("\033[2J\033[H", end="")  # clear screen
    say(f"{B}{M}kriya{X} {B}· governed agent access to a CAD model{X}  {D}(geometry: {kernel}){X}", 1.2)
    rule()
    say(f"{D}Desktop CAD has no cloud API. We gave an AI agent the ability to operate a parametric{X}")
    say(f"{D}model — through typed actions, every one gated on-device. Watch what it can and can't do.{X}", 1.4)
    say()

    def measured(text):
        try:
            d = json.loads(text)
            bb = d["boundingBoxMm"]
            return f"bbox {bb['x']}×{bb['y']}×{bb['z']} mm · volume {d['volumeMm3']} mm³ · {d['holes']} holes · kernel={d['kernel']}"
        except Exception:
            return text[:70]

    # ── Act 1: the agent does the safe, everyday CAD work, unattended ──────────
    say(f"{B}1 · The agent revises the part{X}", 0.8)
    srv = Server("deny")
    action(srv, "measure", {}, show=measured)
    action(srv, "set_parameter", {"name": "width", "value": 100})
    action(srv, "add_hole", {"x": 0, "y": 0, "diameter": 12})
    action(srv, "measure", {}, show=measured)

    # ── Act 2: the agent reaches for something destructive ─────────────────────
    say(f"{B}2 · The agent tries to destroy geometry{X}", 0.8)
    action(srv, "delete_body", {})
    action(srv, "reset_model", {})
    action(srv, "run_macro", {"script": "rm -rf *"})  # not even a registered tool
    srv.close()
    say(f"  {D}↑ deny-by-default: destructive ops need a human; unlisted tools are refused outright.{X}", 1.4)
    say()

    # ── Act 3: with your approval, it proceeds — and that's audited too ────────
    say(f"{B}3 · When YOU approve, it proceeds{X} {D}(approval granted){X}", 0.8)
    srv2 = Server("auto")  # stands in for a human clicking "Approve"
    action(srv2, "delete_feature", {"id": "h1"})
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
                say(f"  {D}receipt{X} {r['action_id']:<16} {mark}  {M}sig {sig}…{X}", 0.35)
    except FileNotFoundError:
        say(f"  {D}(no audit log at {AUDIT_LOG}){X}")
    pause()

    if VERIFY and os.path.exists(VERIFY):
        say()
        say(f"  {D}$ verify-receipts   # offline, on-device — no network{X}", 0.6)
        res = subprocess.run([VERIFY, AUDIT_LOG], capture_output=True, text=True)
        for l in (res.stdout + res.stderr).strip().splitlines()[-3:]:
            say(f"  {l}", 0.2)
        verdict = (f"{G}✔ all signatures verify — tamper-evident proof of what the agent did{X}"
                   if res.returncode == 0 else f"{R}✗ verification failed{X}")
        say(f"  {verdict}", 1.2)
    say()

    rule()
    say(f"{B}~45 lines of glue. Zero changes to the CAD kernel.{X} {D}The agent got capability; you kept control.{X}", 1.0)
    say(f"{M}kriya{X} {D}· github.com/sandeepshekhar26/kriya{X}", 0.5)


if __name__ == "__main__":
    main()
