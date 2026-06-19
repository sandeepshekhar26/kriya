"""kriya -- the governed in-process action layer for Python apps.

Declare each of your app's capabilities once as a typed **action**: a human triggers it by clicking,
an agent triggers it by calling it -- the *same* handler underneath. Then hand the registry to
:func:`run_task`, which spawns the governed ``kriya-host`` and drives an agent run with permission,
human approval, budget, and a signed audit trail enforced on-device, in a process your UI can't
tamper with.

This is the Python binding (roadmap R17). It speaks the same stdio/NDJSON protocol to the same
``kriya-host`` binary the Node ``kriya-sidecar`` does -- a second binding, not a new host.

Quick start::

    import kriya
    from kriya import string, required

    kriya.register_action(
        id="categorize_transaction",
        description="Assign a category to a transaction.",
        parameters={"id": required(string), "category": required(string)},
        permissions=["write:ledger"],
        handler=lambda p, ctx: kriya.ok({"id": p["id"]}),
    )

    host = kriya.Host.spawn("/path/to/kriya-host", ["--script", "demo.json"])
    done = kriya.run_task(host, goal="tidy up", state={}, registry=kriya.default_registry())
    host.close()
"""

from __future__ import annotations

__version__ = "0.0.1"

from .types import (
    ParameterType,
    ParameterSchema,
    string,
    number,
    boolean,
    required,
    array,
    obj,
    Permission,
    ActionResult,
    ok,
    err,
    coerce_result,
    Handler,
    ActionContext,
    ActionDefinition,
)
from .validate import ValidationIssue, validate_params, format_issues
from .jsonschema import to_json_schema, params_to_json_schema
from .registry import (
    ActionValidationError,
    MAX_COMPOSE_DEPTH,
    RegisteredAction,
    Registry,
    default_registry,
    register_action,
    wrap_action,
    dispatch_action,
    tool_schemas,
    mcp_tool_schemas,
    list_action_ids,
    clear_registry,
)
from .protocol import (
    ActionRequest,
    ApprovalRequest,
    AwaitStep,
    Done,
    LogEntry,
    Episode,
    StartRequest,
    ActionResultMsg,
    ApprovalResponse,
    StepAdvance,
)
from .host import Host, run_task, StateLike

__all__ = [
    "__version__",
    # schema + helpers
    "ParameterType",
    "ParameterSchema",
    "string",
    "number",
    "boolean",
    "required",
    "array",
    "obj",
    "Permission",
    "ActionResult",
    "ok",
    "err",
    "coerce_result",
    "Handler",
    "ActionContext",
    "ActionDefinition",
    # validation + schema export
    "ValidationIssue",
    "validate_params",
    "format_issues",
    "to_json_schema",
    "params_to_json_schema",
    # registry
    "ActionValidationError",
    "MAX_COMPOSE_DEPTH",
    "RegisteredAction",
    "Registry",
    "default_registry",
    "register_action",
    "wrap_action",
    "dispatch_action",
    "tool_schemas",
    "mcp_tool_schemas",
    "list_action_ids",
    "clear_registry",
    # protocol
    "ActionRequest",
    "ApprovalRequest",
    "AwaitStep",
    "Done",
    "LogEntry",
    "Episode",
    "StartRequest",
    "ActionResultMsg",
    "ApprovalResponse",
    "StepAdvance",
    # host driver
    "Host",
    "run_task",
    "StateLike",
]
