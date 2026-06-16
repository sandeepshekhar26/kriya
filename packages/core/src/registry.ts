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
import { paramsToJSONSchema, type McpTool } from "./jsonschema.js";

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

/**
 * Maximum nested-composition depth. A handler that calls a child handler that
 * calls another, etc., is capped here so a buggy graph can't blow the stack.
 */
export const MAX_COMPOSE_DEPTH = 8;

/** Look up and run a registered action by id (used when an agent calls a tool). */
export async function dispatchAction(
  id: string,
  params: Record<string, unknown>,
  ctx: ActionContext
): Promise<ActionResult> {
  return dispatchWithChain(id, params, ctx, ctx.chain ?? []);
}

async function dispatchWithChain(
  id: string,
  params: Record<string, unknown>,
  ctx: ActionContext,
  parentChain: readonly string[]
): Promise<ActionResult> {
  // Cycle and depth guards. Caught here so both top-level and composed calls
  // benefit, and so the failure surfaces as a normal ActionResult rather than
  // a thrown exception.
  if (parentChain.includes(id)) {
    return {
      success: false,
      error: `Composition cycle detected: ${[...parentChain, id].join(" -> ")}.`,
    };
  }
  if (parentChain.length >= MAX_COMPOSE_DEPTH) {
    return {
      success: false,
      error: `Composition depth ${MAX_COMPOSE_DEPTH} exceeded at "${id}" (chain: ${parentChain.join(" -> ")}).`,
    };
  }

  const def = registry.get(id);
  if (!def) {
    return { success: false, error: `Unknown action "${id}".` };
  }
  const issues = validateParams(params, def.parameters);
  if (issues.length > 0) {
    return { success: false, error: `Invalid parameters: ${formatIssues(issues)}.` };
  }

  const newChain: readonly string[] = [...parentChain, id];
  // Build the child-call function bound to *this* call's chain.
  const childCtx: ActionContext = {
    caller: ctx.caller,
    stepId: ctx.stepId,
    chain: newChain,
    call: <C>(childId: string, childParams: Record<string, unknown>) =>
      dispatchWithChain(childId, childParams, { ...ctx, chain: newChain }, newChain) as Promise<
        ActionResult<C>
      >,
  };

  try {
    return await def.handler(params, childCtx);
  } catch (err) {
    return { success: false, error: err instanceof Error ? err.message : String(err) };
  }
}

/**
 * Emit MCP-compatible tool schemas for every registered action (no handlers).
 *
 * `inputSchema` is standards-compliant JSON Schema: the per-property `required` hint on our
 * internal {@link ParameterSchema} is lifted into the object-level `required` array and never
 * emitted as a boolean — strict validators (e.g. the Anthropic tool API) reject the latter.
 */
export function getToolSchemas(): ToolSchema[] {
  return [...registry.values()].map((def) => ({
    name: def.id,
    version: def.version ?? 1,
    description: def.description,
    permissions: def.permissions ?? [],
    inputSchema: paramsToJSONSchema(def.parameters),
  }));
}

/**
 * Like {@link getToolSchemas}, but as the bare MCP tool shape (no kriya `permissions`) — for
 * MCP clients that want only name/version/description/inputSchema.
 */
export function getMcpToolSchemas(): McpTool[] {
  return getToolSchemas().map((t) => ({
    name: t.name,
    version: t.version,
    description: t.description,
    inputSchema: t.inputSchema,
  }));
}

/** Test/utility helpers. */
export function listActionIds(): string[] {
  return [...registry.keys()];
}

export function clearRegistry(): void {
  registry.clear();
}
