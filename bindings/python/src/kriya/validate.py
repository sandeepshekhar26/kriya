"""Runtime validation of action parameters against their declared schemas.

This is what stops an agent (or a buggy caller) from invoking a handler with the wrong shape.
It validates types, enums, required-ness, and array/object element types. Unknown parameters are
ignored (forward-compatible). Faithful port of kriya-core's ``validate.ts`` -- same rules, same
messages, so a Python app rejects exactly what a TS app would.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, List, Union

from .types import ParameterSchema


@dataclass
class ValidationIssue:
    # Dotted path to the offending value, e.g. "tags[2]".
    path: str
    message: str


def _type_of(value: Any) -> str:
    """JS-``typeof``-equivalent classification used by the schema checks.

    Order matters: ``bool`` is a subclass of ``int`` in Python, so it must be tested *before*
    the numeric check or ``True`` would read as a number.
    """
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, (list, tuple)):
        return "array"
    if isinstance(value, dict):
        return "object"
    return "object"


def _check_value(value: Any, schema: ParameterSchema, path: str) -> List[ValidationIssue]:
    issues: List[ValidationIssue] = []
    actual = _type_of(value)

    if actual != schema.type:
        issues.append(ValidationIssue(path, f"expected {schema.type}, got {actual}"))
        return issues  # type is wrong; deeper checks would be noise

    if schema.enum is not None and value not in schema.enum:
        import json

        issues.append(
            ValidationIssue(
                path,
                f"value {json.dumps(value)} is not one of {json.dumps(list(schema.enum))}",
            )
        )

    if schema.type == "array" and schema.items is not None:
        item_schema: ParameterSchema = (
            ParameterSchema(schema.items)
            if isinstance(schema.items, str)
            else schema.items
        )
        for i, el in enumerate(value):
            issues.extend(_check_value(el, item_schema, f"{path}[{i}]"))

    if schema.type == "object" and schema.properties is not None:
        for key, prop_schema in schema.properties.items():
            if key not in value:
                if prop_schema.required:
                    issues.append(ValidationIssue(f"{path}.{key}", "required"))
                continue
            issues.extend(_check_value(value[key], prop_schema, f"{path}.{key}"))

    return issues


def validate_params(
    params: Dict[str, Any], schemas: Dict[str, ParameterSchema]
) -> List[ValidationIssue]:
    """Validate a params dict against a map of named parameter schemas."""
    issues: List[ValidationIssue] = []
    for name, schema in schemas.items():
        if name not in params:
            if schema.required:
                issues.append(ValidationIssue(name, "required"))
            continue
        issues.extend(_check_value(params[name], schema, name))
    return issues


def format_issues(issues: List[ValidationIssue]) -> str:
    """Format issues into a single human/agent-readable string."""
    return "; ".join(f"{i.path}: {i.message}" for i in issues)


__all__ = ["ValidationIssue", "validate_params", "format_issues"]
