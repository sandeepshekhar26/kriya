"""Core type definitions for kriya actions.

An "action" is the unit an agent can call and a human can trigger. It is a typed,
permission-scoped, self-describing affordance -- the desktop analogue of an MCP tool.

This is the Python mirror of ``kriya-core``'s ``types.ts`` / param helpers, kept faithful so a
Python app exposes the same governed action surface a Tauri or Electron/Node app does.
"""

from __future__ import annotations

from dataclasses import dataclass, field, replace
from typing import Any, Callable, Dict, List, Optional, Sequence, Union

# The subset of JSON Schema we use to describe a parameter. Deliberately small -- mirrors
# kriya-core's ``ParameterType``.
ParameterType = str  # one of: "string" | "number" | "boolean" | "array" | "object"

_VALID_TYPES = ("string", "number", "boolean", "array", "object")


@dataclass(frozen=True)
class ParameterSchema:
    """The schema for a single action parameter.

    Frozen so the shared ``string`` / ``number`` / ``boolean`` singletons below can't be mutated
    by accident; use :func:`required`, :func:`array`, :func:`obj` (or :func:`dataclasses.replace`)
    to derive a richer schema.
    """

    type: ParameterType
    description: Optional[str] = None
    required: bool = False
    # For ``type == "array"``: the element type (a bare type string or a nested schema).
    items: Optional[Union[str, "ParameterSchema"]] = None
    # For ``type == "object"``: the nested property schemas.
    properties: Optional[Dict[str, "ParameterSchema"]] = None
    # Restrict to an enumerated set of values.
    enum: Optional[Sequence[Union[str, int]]] = None


# Ready-to-use singletons for the common scalar cases, so the developer writes
# ``parameters={"title": string}`` exactly like kriya-core's ``{ title: str }``.
string = ParameterSchema("string")
number = ParameterSchema("number")
boolean = ParameterSchema("boolean")


def required(schema: ParameterSchema) -> ParameterSchema:
    """Return a copy of ``schema`` marked required, e.g. ``required(string)``."""
    return replace(schema, required=True)


def array(
    items: Union[str, ParameterSchema],
    *,
    description: Optional[str] = None,
    required: bool = False,  # noqa: A002 - mirrors the schema field name on purpose
) -> ParameterSchema:
    """Build an ``array`` parameter schema with the given element type."""
    return ParameterSchema(
        "array", description=description, required=required, items=items
    )


def obj(
    properties: Dict[str, ParameterSchema],
    *,
    description: Optional[str] = None,
    required: bool = False,  # noqa: A002
) -> ParameterSchema:
    """Build an ``object`` parameter schema with the given property schemas."""
    return ParameterSchema(
        "object", description=description, required=required, properties=properties
    )


# A permission scope string, e.g. "write:notes" or "delete:notes".
Permission = str


@dataclass
class ActionResult:
    """What an action handler returns. ``data`` is action-specific payload; ``error`` is set
    only when ``success`` is False."""

    success: bool
    data: Any = None
    error: Optional[str] = None

    def to_dict(self) -> Dict[str, Any]:
        out: Dict[str, Any] = {"success": self.success}
        if self.data is not None:
            out["data"] = self.data
        if self.error is not None:
            out["error"] = self.error
        return out


def ok(data: Any = None) -> ActionResult:
    """A successful :class:`ActionResult`."""
    return ActionResult(success=True, data=data)


def err(message: str) -> ActionResult:
    """A failed :class:`ActionResult`."""
    return ActionResult(success=False, error=message)


def coerce_result(value: Any) -> ActionResult:
    """Normalize a handler's return value into an :class:`ActionResult`.

    Accepts an :class:`ActionResult` (used as-is) or a dict that looks like one
    (``{"success": ..., "data"?: ..., "error"?: ...}``). Anything else is treated as the
    ``data`` of a successful result, so a handler can simply ``return some_value``.
    """
    if isinstance(value, ActionResult):
        return value
    if isinstance(value, dict) and "success" in value:
        return ActionResult(
            success=bool(value["success"]),
            data=value.get("data"),
            error=value.get("error"),
        )
    return ActionResult(success=True, data=value)


# A handler is ``(params, ctx) -> ActionResult | dict | Any``. Sync only (the sidecar driver runs
# handlers on the host reader thread). ``ctx`` is an :class:`ActionContext`.
Handler = Callable[[Dict[str, Any], "ActionContext"], Any]


@dataclass
class ActionContext:
    """Context handed to an action handler at execution time."""

    # Who triggered this action: "human" or "agent".
    caller: str = "human"
    # Opaque id of the agent step this call belongs to, when caller is "agent".
    step_id: Optional[str] = None
    # Composition: invoke a *child* action from inside a parent handler. The child runs through
    # the same validation + audit path. Cycles / excessive depth surface as a failed
    # ActionResult rather than raising. Bound by the registry at dispatch time.
    call: Optional[Callable[[str, Dict[str, Any]], ActionResult]] = None
    # Stack of action ids the current call descends from, parent-first (read-only).
    chain: Sequence[str] = field(default_factory=tuple)


@dataclass
class ActionDefinition:
    """The definition a developer passes to :func:`kriya.register_action`."""

    id: str
    description: str
    handler: Handler
    parameters: Dict[str, ParameterSchema] = field(default_factory=dict)
    permissions: List[Permission] = field(default_factory=list)
    # Schema version of this action. Bump when parameters change incompatibly. Defaults to 1.
    version: int = 1


__all__ = [
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
]
