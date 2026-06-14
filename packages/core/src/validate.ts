/**
 * Runtime validation of action parameters against their declared schemas.
 *
 * This is what stops an agent (or a buggy caller) from invoking a handler with the wrong
 * shape. It validates types, enums, required-ness, and array element types. Unknown
 * parameters are ignored (forward-compatible).
 */

import type { ParameterSchema, ParameterType } from "./types.js";

export interface ValidationIssue {
  /** Dotted path to the offending value, e.g. `tags[2]`. */
  path: string;
  message: string;
}

function typeOf(value: unknown): ParameterType | "null" | "undefined" {
  if (value === null) return "null";
  if (value === undefined) return "undefined";
  if (Array.isArray(value)) return "array";
  const t = typeof value;
  if (t === "string" || t === "number" || t === "boolean" || t === "object") return t;
  return "object";
}

function checkValue(value: unknown, schema: ParameterSchema, path: string): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  const actual = typeOf(value);

  if (actual !== schema.type) {
    issues.push({ path, message: `expected ${schema.type}, got ${actual}` });
    return issues; // type is wrong; deeper checks would be noise
  }

  if (schema.enum && !schema.enum.includes(value as string | number)) {
    issues.push({
      path,
      message: `value ${JSON.stringify(value)} is not one of ${JSON.stringify(schema.enum)}`,
    });
  }

  if (schema.type === "array" && schema.items) {
    const itemSchema: ParameterSchema =
      typeof schema.items === "string" ? { type: schema.items } : schema.items;
    (value as unknown[]).forEach((el, i) => {
      issues.push(...checkValue(el, itemSchema, `${path}[${i}]`));
    });
  }

  if (schema.type === "object" && schema.properties) {
    for (const [key, propSchema] of Object.entries(schema.properties)) {
      const v = (value as Record<string, unknown>)[key];
      if (v === undefined) {
        if (propSchema.required) issues.push({ path: `${path}.${key}`, message: "required" });
        continue;
      }
      issues.push(...checkValue(v, propSchema, `${path}.${key}`));
    }
  }

  return issues;
}

/** Validate a params object against a map of named parameter schemas. */
export function validateParams(
  params: Record<string, unknown>,
  schemas: Record<string, ParameterSchema>
): ValidationIssue[] {
  const issues: ValidationIssue[] = [];
  for (const [name, schema] of Object.entries(schemas)) {
    const value = params[name];
    if (value === undefined) {
      if (schema.required) issues.push({ path: name, message: "required" });
      continue;
    }
    issues.push(...checkValue(value, schema, name));
  }
  return issues;
}

/** Format issues into a single human/agent-readable string. */
export function formatIssues(issues: ValidationIssue[]): string {
  return issues.map((i) => `${i.path}: ${i.message}`).join("; ");
}
