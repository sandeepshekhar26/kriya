"""kriya.agents.openai_agents — govern an OpenAI Agents SDK (Python) function tool with kriya.

The OpenAI Agents SDK (Python) turns a function into a tool with `@function_tool`, producing a
`FunctionTool` whose `on_invoke_tool(ctx, input: str)` is the SDK's invocation seam — it parses the
model's JSON arguments and calls your function (verified 2026-07-22 against `openai-agents` 0.18.3).
Governing the tool is governing that seam: this adapter wraps `on_invoke_tool` so every call is
policy -> approval -> budget gated and (when it runs) signs a receipt, with the tool's name and JSON
schema untouched (the model sees an identical tool). No dependency on `agents` -- you build the tool,
then hand it to `govern_function_tool`.

```python
from agents import function_tool
from kriya.agents import GovernClient
from kriya.agents.openai_agents import govern_function_tool

client = GovernClient(actor="openai-agents", audit_log="~/.kriya/audit/oai.jsonl")

@function_tool
def add(a: int, b: int) -> str:
    "Add two numbers."
    return str(a + b)

govern_function_tool(client, add)   # same tool object, now governed
```

A denied / approval-refused / over-budget call raises :class:`kriya.agents.GovernDenied`; the SDK
surfaces it to the model as the tool's error, so it can adapt. Honest ceiling: in-process governance
is cooperative (a hostile process can skip it -- that is what launch-under containment / B14 is for);
it governs the action tier + signs, and does not see the tool's own egress.
"""

from __future__ import annotations

import json
from typing import Any, Optional

from . import GovernClient, GovernDenied

__all__ = ["govern_function_tool"]


def govern_function_tool(client: GovernClient, tool: Any, action_id: Optional[str] = None) -> Any:
    """Govern a built OpenAI Agents SDK `FunctionTool` in place (its `on_invoke_tool` is wrapped).

    Returns the same `tool` object so it can be used inline. `action_id` defaults to `tool.name` --
    the id your policy rules match on. The wrapped seam takes the SDK's raw JSON argument string; it
    is parsed for the receipt's params (the SDK re-parses it for the underlying function, unchanged).
    """
    resolved_id = action_id or getattr(tool, "name", None)
    if not resolved_id:
        raise ValueError("govern_function_tool: tool has no name and no action_id was given")
    original = tool.on_invoke_tool

    async def governed(ctx: Any, input_str: str) -> Any:
        params: Any
        try:
            params = json.loads(input_str) if input_str and input_str.strip() else {}
        except Exception:
            params = {}
        if not isinstance(params, dict):
            params = {"input": params}

        # NOTE: GovernClient is synchronous (fast local IPC to kriya-govern); the brief block inside
        # this coroutine is acceptable for the check/record round-trips.
        gate = client.check(resolved_id, params)
        if gate.decision != "allow":
            raise GovernDenied(resolved_id, gate.decision, gate.reason)
        success = False
        try:
            result = await original(ctx, input_str)
            success = True
            return result
        finally:
            client.record(resolved_id, params, success)

    tool.on_invoke_tool = governed
    return tool
