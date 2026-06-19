"""Standards-compliant JSON Schema exporter (draft 2020-12 clean).

kriya's internal :class:`ParameterSchema` uses a compact custom shape where ``required`` is a
per-property boolean and ``items`` may be a bare type string. MCP clients and strict validators
(e.g. the Anthropic tool API) expect standard JSON Schema where ``required`` is an array of names
on the *parent* object and ``items`` is always a schema object. This converts between the two,
byte-for-byte matching kriya-core's ``jsonschema.ts`` so a Python app emits identical tool schemas.
"""

from __future__ import annotations

from typing import Any, Dict, List

from .types import ParameterSchema


def to_json_schema(schema: ParameterSchema) -> Dict[str, Any]:
    """Convert one :class:`ParameterSchema` node to a standards-compliant JSON Schema object.

    The per-property ``required`` boolean is **never** emitted; the parent object collects
    required names into an array instead. ``items`` is always normalised to a schema object.
    """
    base: Dict[str, Any] = {"type": schema.type}

    if schema.description is not None:
        base["description"] = schema.description
    if schema.enum is not None:
        base["enum"] = list(schema.enum)

    if schema.type == "array":
        raw = schema.items
        item_schema = ParameterSchema(raw) if isinstance(raw, str) else raw
        # ``items`` is guaranteed non-None for arrays by the registry validator.
        base["items"] = to_json_schema(item_schema) if item_schema is not None else {}
    elif schema.type == "object":
        if schema.properties is not None:
            properties: Dict[str, Any] = {}
            req: List[str] = []
            for key, prop in schema.properties.items():
                properties[key] = to_json_schema(prop)
                if prop.required:
                    req.append(key)
            base["properties"] = properties
            if req:
                base["required"] = req

    return base


def params_to_json_schema(params: Dict[str, ParameterSchema]) -> Dict[str, Any]:
    """Build a top-level object schema from a flat map of named :class:`ParameterSchema`."""
    properties: Dict[str, Any] = {}
    req: List[str] = []
    for name, schema in params.items():
        properties[name] = to_json_schema(schema)
        if schema.required:
            req.append(name)

    result: Dict[str, Any] = {"type": "object", "properties": properties}
    if req:
        result["required"] = req
    return result


__all__ = ["to_json_schema", "params_to_json_schema"]
