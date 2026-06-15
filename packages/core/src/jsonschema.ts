/**
 * Standards-compliant JSON Schema exporter for @kriya/core.
 *
 * Our internal `ParameterSchema` uses a compact custom shape where:
 *   - `required` is a per-property boolean, and
 *   - `items` may be a bare type string like `"string"`.
 *
 * MCP clients and external agents expect standard JSON Schema Draft 7 / 2019-09 where:
 *   - `required` is an array of names on the *parent* object schema, and
 *   - `items` is always a schema object.
 *
 * This module converts between the two without touching the original registry output.
 */

import type { ParameterSchema } from "./types.js";
import { getToolSchemas } from "./registry.js";

// ---------------------------------------------------------------------------
// Core converter
// ---------------------------------------------------------------------------

/**
 * Convert a single `ParameterSchema` node to a standards-compliant JSON Schema object.
 *
 * Rules applied:
 * - The per-property `required` boolean is **never** emitted; the parent object schema
 *   collects required names into an array instead.
 * - `items` is always normalised to a schema object (bare type strings are expanded).
 * - `description` and `enum` are forwarded as-is.
 */
export function toJSONSchema(schema: ParameterSchema): object {
  const base: Record<string, unknown> = { type: schema.type };

  if (schema.description !== undefined) {
    base["description"] = schema.description;
  }

  if (schema.enum !== undefined) {
    base["enum"] = schema.enum;
  }

  switch (schema.type) {
    case "array": {
      // `items` is guaranteed non-undefined for arrays by the registry validator.
      const raw = schema.items;
      const itemSchema: ParameterSchema =
        typeof raw === "string" ? { type: raw } : (raw as ParameterSchema);
      base["items"] = toJSONSchema(itemSchema);
      break;
    }

    case "object": {
      if (schema.properties !== undefined) {
        const properties: Record<string, object> = {};
        const required: string[] = [];

        for (const [key, prop] of Object.entries(schema.properties)) {
          properties[key] = toJSONSchema(prop);
          if (prop.required === true) {
            required.push(key);
          }
        }

        base["properties"] = properties;
        if (required.length > 0) {
          base["required"] = required;
        }
      }
      break;
    }

    // string / number / boolean: no extra fields beyond type/description/enum.
    default:
      break;
  }

  return base;
}

// ---------------------------------------------------------------------------
// Top-level params map → object schema
// ---------------------------------------------------------------------------

/**
 * Build a top-level JSON Schema object from a flat map of named `ParameterSchema`s —
 * the shape stored in `ActionDefinition.parameters`.
 *
 * Produces:
 * ```json
 * {
 *   "type": "object",
 *   "properties": { "<name>": <JSON Schema node>, … },
 *   "required": ["<name>", …]   // omitted when no params are required
 * }
 * ```
 */
export function paramsToJSONSchema(params: Record<string, ParameterSchema>): object {
  const properties: Record<string, object> = {};
  const required: string[] = [];

  for (const [name, schema] of Object.entries(params)) {
    properties[name] = toJSONSchema(schema);
    if (schema.required === true) {
      required.push(name);
    }
  }

  const result: Record<string, unknown> = { type: "object", properties };
  if (required.length > 0) {
    result["required"] = required;
  }
  return result;
}

// ---------------------------------------------------------------------------
// MCP tool schema — standard JSON Schema edition
// ---------------------------------------------------------------------------

/**
 * An MCP tool descriptor whose `inputSchema` is standards-compliant JSON Schema,
 * ready for consumption by MCP clients and external agents.
 */
export interface McpTool {
  name: string;
  version: number;
  description: string;
  inputSchema: object;
}

/**
 * Like `getToolSchemas()`, but with `inputSchema` converted to standard JSON Schema:
 * - `required` is an array of names, not a per-property boolean.
 * - `items` is always a schema object, never a bare type string.
 *
 * The registry itself is not duplicated — this function delegates to `getToolSchemas()`
 * and post-processes the result.
 */
export function getMcpToolSchemas(): McpTool[] {
  return getToolSchemas().map((tool) => ({
    name: tool.name,
    version: tool.version,
    description: tool.description,
    inputSchema: paramsToJSONSchema(tool.inputSchema.properties),
  }));
}
