"""kriya.agents.langgraph — govern a LangGraph / LangChain (Python) tool with kriya.

LangChain's ``@tool`` / ``StructuredTool.from_function`` wraps a plain **function** into an
agent-callable tool; LangGraph's ``ToolNode`` then invokes it. So governing LangGraph is governing
that function -- this adapter is a thin shim over the framework-agnostic :func:`kriya.agents.govern`,
with no dependency on ``langchain`` (so it imports without the framework installed).

Recommended usage -- wrap the function BEFORE you build the tool (version-proof)::

    from langchain_core.tools import tool
    from kriya.agents import GovernClient
    from kriya.agents.langgraph import govern_tool

    client = GovernClient(actor="langgraph", audit_log="~/.kriya/audit/langgraph.jsonl")

    @tool
    def web_search(q: str) -> str:
        '''Search the web.'''
        ...

    # governed version, same name/schema/description the model sees:
    web_search.func = govern_tool(client, "web_search", web_search.func)

Honest ceiling (state it in your app too): in-process governance is cooperative -- a hostile agent
process can skip it (that is what launch-under containment / B14 is for) -- and this lane governs the
action tier + signs the receipt; it does not see the tool's own outbound network calls.
"""

from __future__ import annotations

from typing import Any, Callable, Dict, Optional

from . import GovernClient, govern

__all__ = ["govern_tool"]


def govern_tool(
    client: GovernClient,
    action_id: str,
    fn: Optional[Callable[..., Any]] = None,
) -> Callable:
    """Wrap the function you hand to LangChain's ``@tool`` / ``StructuredTool.from_function`` so
    every invocation is policy / approval / budget gated and (when it runs) signs a receipt.

    ``action_id`` is the stable id policy rules match on -- use the tool's ``name``. LangChain calls
    tool functions with **keyword** arguments (``fn(**args)``); this shim accepts that shape and
    presents the args to :func:`kriya.agents.govern` as the receipt's params. Usable directly or as a
    decorator, exactly like :func:`kriya.agents.govern`.
    """

    def wrap(f: Callable[..., Any]) -> Callable[..., Any]:
        # LangChain invokes tools as f(**kwargs) (or f(single_positional)); govern() works on a
        # params dict, so adapt at the boundary and pass the original call through untouched.
        def call_with_params(params: Dict[str, Any]) -> Any:
            return f(**params)

        governed = govern(client, action_id, call_with_params)

        def wrapped(*args: Any, **kwargs: Any) -> Any:
            if kwargs:
                params: Dict[str, Any] = dict(kwargs)
            elif len(args) == 1 and isinstance(args[0], dict):
                params = args[0]
            else:
                params = {"args": list(args)}
            return governed(params)

        wrapped.__name__ = getattr(f, "__name__", action_id)
        wrapped.__doc__ = getattr(f, "__doc__", None)
        return wrapped

    return wrap(fn) if fn is not None else wrap
