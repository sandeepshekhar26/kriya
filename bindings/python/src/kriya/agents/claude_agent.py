"""kriya.agents.claude_agent — govern a Claude Agent SDK (Python) custom tool with kriya.

The Claude Agent SDK (Python) defines an in-process tool with the `@tool(name, description,
input_schema)` decorator over an async `handler(args) -> {"content": [...], "isError"?}` (verified
2026-07-22 against `claude-agent-sdk` 0.2.125). Governing the tool is governing that handler: this
adapter wraps the built `SdkMcpTool.handler` so every call is policy -> approval -> budget gated and
(when it runs) signs a receipt, with name/description/input_schema untouched. No dependency on
`claude_agent_sdk` -- you build the tool, then hand it to `govern_sdk_tool`.

```python
from claude_agent_sdk import tool, create_sdk_mcp_server
from kriya.agents import GovernClient
from kriya.agents.claude_agent import govern_sdk_tool

client = GovernClient(actor="claude-agent", audit_log="~/.kriya/audit/claude.jsonl")

@tool("search", "Search the web", {"query": str})
async def search(args):
    return {"content": [{"type": "text", "text": f"Results for: {args['query']}"}]}

govern_sdk_tool(client, search)   # same tool object, now governed
server = create_sdk_mcp_server(name="my-tools", tools=[search])
```

A denied / approval-refused / over-budget call returns an `isError` result (idiomatic MCP tool shape
-- the model sees the tool errored and adapts), not a raised exception. Honest ceiling: in-process
governance is cooperative (a hostile process can skip it -- that is what launch-under containment /
B14 is for); it governs the action tier + signs, and does not see the tool's own egress.
"""

from __future__ import annotations

from typing import Any, Dict, Optional

from . import GovernClient

__all__ = ["govern_sdk_tool"]


def _denied_result(action_id: str, decision: str, reason: Optional[str]) -> Dict[str, Any]:
    detail = f" — {reason}" if reason else ""
    return {
        "content": [{"type": "text", "text": f'kriya denied "{action_id}": {decision}{detail}'}],
        "isError": True,
    }


def govern_sdk_tool(client: GovernClient, tool: Any, action_id: Optional[str] = None) -> Any:
    """Govern a built Claude Agent SDK `SdkMcpTool` in place (its `handler` is wrapped). Returns the
    same `tool`. `action_id` defaults to `tool.name` -- the id your policy rules match on.
    """
    resolved_id = action_id or getattr(tool, "name", None)
    if not resolved_id:
        raise ValueError("govern_sdk_tool: tool has no name and no action_id was given")
    original = tool.handler

    async def governed(args: Any) -> Dict[str, Any]:
        params = dict(args) if isinstance(args, dict) else {"input": args}
        gate = client.check(resolved_id, params)
        if gate.decision != "allow":
            # Action-tier deny signs no receipt (parity with the governor); surface it as isError.
            return _denied_result(resolved_id, gate.decision, gate.reason)
        success = False
        try:
            result = await original(args)
            success = not (isinstance(result, dict) and result.get("isError"))
            return result
        finally:
            client.record(resolved_id, params, success)

    tool.handler = governed
    return tool
