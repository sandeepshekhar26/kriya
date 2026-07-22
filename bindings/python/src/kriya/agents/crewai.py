"""kriya.agents.crewai — govern a CrewAI tool with kriya.

CrewAI's `@tool("name")` decorator (from `crewai.tools`) wraps a function into a `Tool` whose `.func`
is the underlying callable; the agent runs it via `tool.run(**kwargs)` -> `._run` -> `.func(**kwargs)`
(verified 2026-07-22 against `crewai` 1.15.5). Governing the tool is governing that `.func`: this
adapter replaces it with a governed callable so every run is policy -> approval -> budget gated and
(when it runs) signs a receipt, with the tool's name and args-schema untouched. No dependency on
`crewai` -- you build the tool, then hand it to `govern_crew_tool`.

```python
from crewai.tools import tool
from kriya.agents import GovernClient
from kriya.agents.crewai import govern_crew_tool

client = GovernClient(actor="crewai", audit_log="~/.kriya/audit/crewai.jsonl")

@tool("add")
def add(a: int, b: int) -> str:
    "Add two numbers."
    return str(a + b)

govern_crew_tool(client, add)   # same tool object, now governed
```

A denied / approval-refused / over-budget run raises :class:`kriya.agents.GovernDenied`; CrewAI
surfaces it as the tool's error so the agent can adapt. Honest ceiling: in-process governance is
cooperative (a hostile process can skip it -- that is what launch-under containment / B14 is for); it
governs the action tier + signs, and does not see the tool's own egress.
"""

from __future__ import annotations

from typing import Any, Optional

from . import GovernClient, GovernDenied

__all__ = ["govern_crew_tool"]


def govern_crew_tool(client: GovernClient, tool: Any, action_id: Optional[str] = None) -> Any:
    """Govern a built CrewAI `Tool` in place (its `.func` is wrapped). Returns the same `tool`.

    `action_id` defaults to `tool.name` -- the id your policy rules match on. CrewAI calls tool
    functions with keyword arguments; those become the receipt's params.
    """
    resolved_id = action_id or getattr(tool, "name", None)
    if not resolved_id:
        raise ValueError("govern_crew_tool: tool has no name and no action_id was given")
    original = tool.func

    def governed(*args: Any, **kwargs: Any) -> Any:
        params = dict(kwargs)
        if args:
            params["_args"] = list(args)
        gate = client.check(resolved_id, params)
        if gate.decision != "allow":
            raise GovernDenied(resolved_id, gate.decision, gate.reason)
        success = False
        try:
            result = original(*args, **kwargs)
            success = True
            return result
        finally:
            client.record(resolved_id, params, success)

    tool.func = governed
    return tool
