"""The action registry: a map of registered actions.

Developers call :func:`register_action` once per affordance (or :func:`wrap_action` to adopt a
function the app already has). The registry can then hand the Rust agent host an MCP-compatible
schema of every action (no handlers) and dispatch an incoming agent tool-call to the right handler
-- through the same validation + composition path as kriya-core.

A :class:`Registry` instance owns its own map; module-level :func:`register_action` etc. operate on
a default process-global registry, mirroring kriya-core's global-registry ergonomics. Tests should
use a fresh :class:`Registry` (or call :func:`clear_registry`) to avoid cross-test bleed.
"""

from __future__ import annotations

import re
from typing import Any, Callable, Dict, List, Optional, Sequence

from .jsonschema import params_to_json_schema
from .types import (
    ActionContext,
    ActionDefinition,
    ActionResult,
    Handler,
    ParameterSchema,
    Permission,
    coerce_result,
)
from .validate import format_issues, validate_params


class ActionValidationError(Exception):
    """Raised when an action *definition* is invalid (bad id, dup, missing handler, …)."""


# Maximum nested-composition depth. A handler that calls a child that calls another, etc., is
# capped here so a buggy graph can't blow the stack. Matches kriya-core's MAX_COMPOSE_DEPTH.
MAX_COMPOSE_DEPTH = 8

_ID_PATTERN = re.compile(r"^[a-z][a-z0-9_]*$")
_VALID_TYPES = ("string", "number", "boolean", "array", "object")


class RegisteredAction:
    """A callable handle returned by :func:`Registry.register_action` (handy for tests)."""

    def __init__(self, registry: "Registry", action_id: str):
        self._registry = registry
        self.id = action_id

    def call(
        self, params: Dict[str, Any], ctx: Optional[ActionContext] = None
    ) -> ActionResult:
        base = ctx or ActionContext(caller="human")
        return self._registry.dispatch_action(self.id, params, base)


class Registry:
    """An isolated action registry."""

    def __init__(self) -> None:
        self._actions: Dict[str, ActionDefinition] = {}

    # -- registration -------------------------------------------------------

    def register_action(
        self,
        *,
        id: str,  # noqa: A002 - matches the public field name on purpose
        description: str,
        handler: Handler,
        parameters: Optional[Dict[str, ParameterSchema]] = None,
        permissions: Optional[Sequence[Permission]] = None,
        version: int = 1,
    ) -> RegisteredAction:
        """Register an action so agents can discover and call it.

        Returns a callable handle the app can also invoke directly.
        """
        defn = ActionDefinition(
            id=id,
            description=description,
            handler=handler,
            parameters=dict(parameters or {}),
            permissions=list(permissions or []),
            version=version,
        )
        self._validate_definition(defn)
        self._actions[defn.id] = defn
        return RegisteredAction(self, defn.id)

    def wrap_action(
        self,
        fn: Callable[..., Any],
        *,
        id: str,  # noqa: A002
        description: str,
        parameters: Optional[Dict[str, ParameterSchema]] = None,
        permissions: Optional[Sequence[Permission]] = None,
        version: int = 1,
        map_params: Optional[Callable[[Dict[str, Any]], Sequence[Any]]] = None,
        map_result: Optional[Callable[[Any], Any]] = None,
    ) -> RegisteredAction:
        """Bolt the kriya action layer onto a function the app *already has*. Augment, not migrate.

        ``fn`` may take positional args, return a plain value, and raise on failure.
        ``map_params`` maps the agent's params dict onto the function's positional arguments
        (default: pass the whole params dict as one argument). A returned value becomes
        ``ActionResult(success=True, data=...)``; a raised exception becomes
        ``ActionResult(success=False, error=...)``. ``map_result`` optionally transforms the
        return value before it reaches ``data``.
        """

        def handler(params: Dict[str, Any], _ctx: ActionContext) -> ActionResult:
            try:
                args = map_params(params) if map_params else [params]
                returned = fn(*args)
            except Exception as exc:  # the app's own throw is an expected failure, not a crash
                return ActionResult(success=False, error=str(exc))
            data = map_result(returned) if map_result else returned
            return ActionResult(success=True, data=data)

        return self.register_action(
            id=id,
            description=description,
            handler=handler,
            parameters=parameters,
            permissions=permissions,
            version=version,
        )

    def _validate_definition(self, defn: ActionDefinition) -> None:
        if not defn.id or not _ID_PATTERN.match(defn.id):
            raise ActionValidationError(
                f'Action id "{defn.id}" is invalid. Use snake_case starting with a letter.'
            )
        if defn.id in self._actions:
            raise ActionValidationError(f'Action "{defn.id}" is already registered.')
        if not (defn.description and defn.description.strip()):
            raise ActionValidationError(
                f'Action "{defn.id}" needs a non-empty description.'
            )
        if not callable(defn.handler):
            raise ActionValidationError(f'Action "{defn.id}" needs a handler function.')
        for name, schema in defn.parameters.items():
            self._validate_parameter(defn.id, name, schema)

    @staticmethod
    def _validate_parameter(action_id: str, name: str, schema: ParameterSchema) -> None:
        if schema.type not in _VALID_TYPES:
            raise ActionValidationError(
                f'Action "{action_id}" parameter "{name}" has invalid type "{schema.type}".'
            )
        if schema.type == "array" and schema.items is None:
            raise ActionValidationError(
                f'Action "{action_id}" parameter "{name}" is an array but has no "items".'
            )

    # -- dispatch -----------------------------------------------------------

    def dispatch_action(
        self, action_id: str, params: Dict[str, Any], ctx: ActionContext
    ) -> ActionResult:
        """Look up and run a registered action by id (used when an agent calls a tool)."""
        return self._dispatch_with_chain(
            action_id, params, ctx, tuple(ctx.chain or ())
        )

    def _dispatch_with_chain(
        self,
        action_id: str,
        params: Dict[str, Any],
        ctx: ActionContext,
        parent_chain: Sequence[str],
    ) -> ActionResult:
        # Cycle and depth guards -- surfaced as a failed ActionResult, not a raise.
        if action_id in parent_chain:
            trail = " -> ".join([*parent_chain, action_id])
            return ActionResult(
                success=False, error=f"Composition cycle detected: {trail}."
            )
        if len(parent_chain) >= MAX_COMPOSE_DEPTH:
            return ActionResult(
                success=False,
                error=(
                    f'Composition depth {MAX_COMPOSE_DEPTH} exceeded at "{action_id}" '
                    f'(chain: {" -> ".join(parent_chain)}).'
                ),
            )

        defn = self._actions.get(action_id)
        if defn is None:
            return ActionResult(success=False, error=f'Unknown action "{action_id}".')

        issues = validate_params(params, defn.parameters)
        if issues:
            return ActionResult(
                success=False, error=f"Invalid parameters: {format_issues(issues)}."
            )

        new_chain = (*parent_chain, action_id)

        def child_call(child_id: str, child_params: Dict[str, Any]) -> ActionResult:
            child_ctx = ActionContext(
                caller=ctx.caller, step_id=ctx.step_id, chain=new_chain
            )
            return self._dispatch_with_chain(
                child_id, child_params, child_ctx, new_chain
            )

        child_ctx = ActionContext(
            caller=ctx.caller, step_id=ctx.step_id, chain=new_chain, call=child_call
        )

        try:
            return coerce_result(defn.handler(params, child_ctx))
        except Exception as exc:
            return ActionResult(success=False, error=str(exc))

    # -- schema export ------------------------------------------------------

    def tool_schemas(self) -> List[Dict[str, Any]]:
        """Emit MCP-compatible tool schemas for every registered action (no handlers).

        ``inputSchema`` is standards-compliant JSON Schema. The wire keys are camelCase
        (``inputSchema``) to match the host's ``ToolSchema`` deserialization exactly.
        """
        return [
            {
                "name": defn.id,
                "version": defn.version,
                "description": defn.description,
                "permissions": list(defn.permissions),
                "inputSchema": params_to_json_schema(defn.parameters),
            }
            for defn in self._actions.values()
        ]

    def mcp_tool_schemas(self) -> List[Dict[str, Any]]:
        """Like :func:`tool_schemas` but the bare MCP shape (no kriya ``permissions``)."""
        return [
            {k: t[k] for k in ("name", "version", "description", "inputSchema")}
            for t in self.tool_schemas()
        ]

    # -- introspection ------------------------------------------------------

    def list_action_ids(self) -> List[str]:
        return list(self._actions.keys())

    def get(self, action_id: str) -> Optional[ActionDefinition]:
        return self._actions.get(action_id)

    def clear(self) -> None:
        self._actions.clear()


# -- default process-global registry + module-level shims (kriya-core parity) ----------------

_default = Registry()


def default_registry() -> Registry:
    """The process-global registry the module-level functions operate on."""
    return _default


def register_action(**kwargs: Any) -> RegisteredAction:
    return _default.register_action(**kwargs)


def wrap_action(fn: Callable[..., Any], **kwargs: Any) -> RegisteredAction:
    return _default.wrap_action(fn, **kwargs)


def dispatch_action(
    action_id: str, params: Dict[str, Any], ctx: Optional[ActionContext] = None
) -> ActionResult:
    return _default.dispatch_action(action_id, params, ctx or ActionContext(caller="human"))


def tool_schemas() -> List[Dict[str, Any]]:
    return _default.tool_schemas()


def mcp_tool_schemas() -> List[Dict[str, Any]]:
    return _default.mcp_tool_schemas()


def list_action_ids() -> List[str]:
    return _default.list_action_ids()


def clear_registry() -> None:
    _default.clear()


__all__ = [
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
]
