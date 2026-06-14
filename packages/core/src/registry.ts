/**
 * The action registry: a process-global map of registered actions.
 *
 * Developers call `registerAction` once per affordance. The registry can then:
 *   - hand the Rust agent host an MCP-compatible schema of every action (no handlers), and
 *   - dispatch an incoming agent tool-call to the right handler.
 */

import type {
  ActionContext,
  ActionDefinition,
  ActionResult,
  ParameterSchema,
  ToolSchema,
} from "./types.js";
import { formatIssues, validateParams } from "./validate.js";

export class ActionValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ActionValidationError";
  }
}

const registry = new Map<string, ActionDefinition<any, any>>();

const ID_PATTERN = /^[a-z][a-z0-9_]*$/;

function validateDefinition(def: ActionDefinition<any, any>): void {
  if (!def.id || !ID_PATTERN.test(def.id)) {
    throw new ActionValidationError(
      `Action id "${def.id}" is invalid. Use snake_case starting with a letter.`
    );
  }
  if (registry.has(def.id)) {
    throw new ActionValidationError(`Action "${def.id}" is already registered.`);
  }
  if (!def.description?.trim()) {
    throw new ActionValidationError(`Action "${def.id}" needs a non-empty description.`);
  }
  if (typeof def.handler !== "function") {
    throw new ActionValidationError(`Action "${def.id}" needs a handler function.`);
  }
  for (const [name, schema] of Object.entries(def.parameters ?? {})) {
    validateParameter(def.id, name, schema);
  }
}

function validateParameter(actionId: string, name: string, schema: ParameterSchema): void {
  const valid: ParameterSchema["type"][] = ["string", "number", "boolean", "array", "object"];
  if (!valid.includes(schema.type)) {
    throw new ActionValidationError(
      `Action "${actionId}" parameter "${name}" has invalid type "${schema.type}".`
    );
  }
  if (schema.type === "array" && schema.items === undefined) {
    throw new ActionValidationError(
      `Action "${actionId}" parameter "${name}" is an array but has no "items".`
    );
  }
}

/** A typed, callable handle returned by `registerAction` (handy for tests). */
export interface RegisteredAction<P extends Record<string, unknown>, R> {
  id: string;
  call: (params: P, ctx?: Partial<ActionContext>) => Promise<ActionResult<R>>;
}

/**
 * Register an action so agents can discover and call it.
 * Returns a typed handle the app can also invoke directly.
 */
export function registerAction<P extends Record<string, unknown>, R>(
  def: ActionDefinition<P, R>
): RegisteredAction<P, R> {
  validateDefinition(def);
  registry.set(def.id, def as ActionDefinition<any, any>);
  return {
    id: def.id,
    call: (params: P, ctx?: Partial<ActionContext>) =>
      dispatchAction(def.id, params, { caller: "human", ...ctx }) as Promise<ActionResult<R>>,
  };
}

/** Look up and run a registered action by id (used when an agent calls a tool). */
export async function dispatchAction(
  id: string,
  params: Record<string, unknown>,
  ctx: ActionContext
): Promise<ActionResult> {
  const def = registry.get(id);
  if (!def) {
    return { success: false, error: `Unknown action "${id}".` };
  }
  const issues = validateParams(params, def.parameters);
  if (issues.length > 0) {
    return { success: false, error: `Invalid parameters: ${formatIssues(issues)}.` };
  }
  try {
    return await def.handler(params, ctx);
  } catch (err) {
    return { success: false, error: err instanceof Error ? err.message : String(err) };
  }
}

/** Emit MCP-compatible tool schemas for every registered action (no handlers). */
export function getToolSchemas(): ToolSchema[] {
  return [...registry.values()].map((def) => ({
    name: def.id,
    version: def.version ?? 1,
    description: def.description,
    permissions: def.permissions ?? [],
    inputSchema: {
      type: "object" as const,
      properties: def.parameters,
      required: Object.entries(def.parameters)
        .filter(([, s]) => s.required)
        .map(([n]) => n),
    },
  }));
}

/** Test/utility helpers. */
export function listActionIds(): string[] {
  return [...registry.keys()];
}

export function clearRegistry(): void {
  registry.clear();
}
