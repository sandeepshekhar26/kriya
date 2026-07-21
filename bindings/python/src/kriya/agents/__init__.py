"""kriya.agents — govern in-process agent-framework tool calls with kriya (Python).

The Python sibling of the TypeScript ``kriya-agents`` package. Wrap an agent framework's tool
callback so every tool call is policy -> approval -> budget gated and, when it runs, signs an
Ed25519, hash-chained receipt -- **without an MCP hop and without inverting the framework's control
flow**. Your agent (LangGraph, CrewAI, the OpenAI/Claude Agent SDKs) keeps driving its own loop;
kriya governs and signs each call.

The one-Signer law (design law 3): this module contains **no cryptography, no policy engine, and no
chain writer**. It spawns the runtime's ``kriya-govern`` binary and speaks its two-op stdio protocol
(``check`` then ``record``); every trust decision and every signature is made by ``kriya-govern``,
which reuses the exact Policy / BudgetTracker / ApprovalGate / Signer primitives the in-process host
and ``kriya-mcp`` use. Zero runtime dependencies -- stdlib only.

Honest ceiling: in-process governance is **cooperative** -- a hostile agent *process* can simply not
call this (that is what launch-under containment, ``kriya-gateway run --``, B14, is for). And because
the tool runs in your process, this lane governs the **action tier** (policy / approval / budget) and
signs the receipt; it does **not** see the tool's own outbound network calls.

Quickstart::

    from kriya.agents import GovernClient, govern

    client = GovernClient(policy_path="agent-policy.yaml", actor="my-agent",
                          audit_log="~/.kriya/audit/my-agent.jsonl")

    @govern(client, "web_search")
    def web_search(params):
        return real_search(params["q"])

    web_search({"q": "kriya"})   # gated + receipted; a denied call raises GovernDenied
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from typing import Any, Callable, Dict, Optional

__all__ = ["GovernClient", "govern", "GovernDenied", "CheckResult"]


@dataclass
class CheckResult:
    """The decision ``check`` returns. Only ``allow`` should proceed to execution."""

    decision: str  # "allow" | "denied" | "not_approved" | "budget_exceeded"
    reason: Optional[str] = None


class GovernDenied(Exception):
    """Raised when ``check`` returns a non-``allow`` decision -- surface it as the tool's error."""

    def __init__(self, action_id: str, decision: str, reason: Optional[str] = None):
        self.action_id = action_id
        self.decision = decision
        self.reason = reason
        detail = f" -- {reason}" if reason else ""
        super().__init__(f'kriya denied "{action_id}": {decision}{detail}')


class GovernClient:
    """A live connection to a ``kriya-govern`` subprocess.

    Requests are answered strictly in order (the binary reads one line, writes one line), so a plain
    synchronous read-after-write correlates each response to its request. One client = one govern
    session, so the budget cap spans the whole run.
    """

    def __init__(
        self,
        binary_path: str = "kriya-govern",
        policy_path: Optional[str] = None,
        actor: Optional[str] = None,
        user: Optional[str] = None,
        audit_log: Optional[str] = None,
        approval: str = "deny",
    ):
        args = [binary_path]
        if policy_path:
            args += ["--policy", policy_path]
        if approval:
            args += ["--approval", approval]
        if actor:
            args += ["--actor", actor]
        if user:
            args += ["--user", user]
        if audit_log:
            args += ["--audit-log", audit_log]
        # stderr inherited so the banner + governance log show up, exactly like the TS client.
        self._proc = subprocess.Popen(
            args,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self._closed = False

    def _send(self, request: Dict[str, Any]) -> Dict[str, Any]:
        if self._closed or self._proc.stdin is None or self._proc.stdout is None:
            raise RuntimeError("GovernClient is closed")
        self._proc.stdin.write(json.dumps(request) + "\n")
        self._proc.stdin.flush()
        line = self._proc.stdout.readline()
        if not line:
            raise RuntimeError("kriya-govern closed the stream before replying")
        resp = json.loads(line)
        if resp.get("op") == "error":
            raise RuntimeError(f"kriya-govern: {resp.get('error', 'unknown error')}")
        return resp

    def check(self, action_id: str, params: Dict[str, Any]) -> CheckResult:
        """Ask whether ``action_id`` may run (policy -> approval -> budget). Signs nothing."""
        resp = self._send({"op": "check", "action_id": action_id, "params": params})
        return CheckResult(decision=resp["decision"], reason=resp.get("reason"))

    def record(self, action_id: str, params: Dict[str, Any], success: bool) -> Dict[str, Any]:
        """Sign the receipt for a call the framework executed (success or failure)."""
        resp = self._send(
            {"op": "record", "action_id": action_id, "params": params, "success": success}
        )
        return resp["receipt"]

    def close(self) -> None:
        """Close stdin (signals shutdown), then reap the subprocess and close its streams. Idempotent."""
        if self._closed:
            return
        self._closed = True
        for stream in (self._proc.stdin, self._proc.stdout):
            try:
                if stream:
                    stream.close()
            except Exception:
                pass
        self._proc.terminate()
        try:
            self._proc.wait(timeout=2)
        except Exception:
            self._proc.kill()

    def __enter__(self) -> "GovernClient":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.close()


def govern(
    client: GovernClient,
    action_id: str,
    fn: Optional[Callable[[Dict[str, Any]], Any]] = None,
) -> Callable:
    """Wrap a tool function into a governed one.

    ``check`` first, run the real tool only on ``allow``, then ``record`` the outcome (success OR
    failure -- a raised tool error still signs a ``success=False`` receipt, exactly like the
    in-process governor). A non-``allow`` decision raises :class:`GovernDenied` so the framework
    surfaces it as that tool's native error and the model can adapt.

    Usable directly (``governed = govern(client, "id", fn)``) or as a decorator
    (``@govern(client, "id")``).
    """

    def wrap(f: Callable[[Dict[str, Any]], Any]) -> Callable[[Dict[str, Any]], Any]:
        def wrapped(params: Dict[str, Any]) -> Any:
            gate = client.check(action_id, params)
            if gate.decision != "allow":
                raise GovernDenied(action_id, gate.decision, gate.reason)
            success = False
            try:
                result = f(params)
                success = True
                return result
            finally:
                # Record the attempt whether it succeeded or raised -- the receipt is the record of
                # what was attempted, not only of what worked (parity with the in-process governor).
                client.record(action_id, params, success)

        wrapped.__name__ = getattr(f, "__name__", action_id)
        wrapped.__doc__ = getattr(f, "__doc__", None)
        return wrapped

    return wrap(fn) if fn is not None else wrap
