/**
 * `wrapAction` — bolt the agent-native action layer onto a function an app *already has*,
 * without rewriting it. **Augment, not migrate.**
 *
 * `registerAction` expects a handler in the framework's shape: `(params, ctx) => ActionResult`.
 * Real apps already have functions like `createNote(title): Note` that take positional
 * arguments, return a plain value, and throw on failure. `wrapAction` adapts such a function
 * into a registered action — mapping the agent's `params` object onto the function's arguments
 * and normalizing its return/throw into an {@link ActionResult} — so an existing app gains a
 * governed, agent-callable surface in a few lines per handler.
 */

import { registerAction, type RegisteredAction } from "./registry.js";
import type { ActionResult, ParameterSchema, Permission } from "./types.js";

// Existing app functions have arbitrary signatures; this is the one place we accept that.
type AnyFunction = (...args: never[]) => unknown;

export interface WrapActionOptions<P extends Record<string, unknown>> {
  /** Stable action id, e.g. `"create_note"` (snake_case). */
  id: string;
  /** One-line description an agent reads to decide whether to call this. */
  description: string;
  /**
   * Parameter schemas the agent sees. Optional, but without them the agent has no contract
   * for the arguments — the codemod (`agent-native wrap`) infers these from the function
   * signature so you don't hand-write them.
   */
  parameters?: Record<string, ParameterSchema>;
  /** Permission scopes the host checks before this runs. */
  permissions?: Permission[];
  /** Schema version. Defaults to 1. */
  version?: number;
  /**
   * Map the agent's `params` object onto the wrapped function's positional arguments.
   * Defaults to passing the whole `params` object as the single argument — correct for
   * functions that already take one options object.
   *
   * @example mapParams: (p) => [p.title, p.body]   // createNote(title, body)
   */
  mapParams?: (params: P) => unknown[];
  /**
   * Transform the function's return value into {@link ActionResult.data}. Defaults to using
   * the return value as-is. Use this when the function returns something the agent shouldn't
   * see verbatim (e.g. a DB row → a public id).
   */
  mapResult?: (returnValue: unknown) => unknown;
}

/**
 * Register an existing function as an agent-callable action. Returns the same typed handle as
 * {@link registerAction}, so the app can keep calling it directly too.
 *
 * The wrapped function may be sync or async. A returned value becomes
 * `{ success: true, data }`; a thrown error becomes `{ success: false, error }`. (If your
 * function already returns an `ActionResult`, register it directly instead — `wrapAction`
 * treats the return value as opaque `data`.)
 */
export function wrapAction<P extends Record<string, unknown> = Record<string, unknown>>(
  fn: AnyFunction,
  options: WrapActionOptions<P>,
): RegisteredAction<P, unknown> {
  const { id, description, parameters = {}, permissions, version, mapParams, mapResult } = options;

  return registerAction<P, unknown>({
    id,
    version,
    description,
    parameters,
    permissions,
    handler: async (params): Promise<ActionResult> => {
      let returned: unknown;
      try {
        const args = (mapParams ? mapParams(params) : [params]) as never[];
        returned = await fn(...args);
      } catch (err) {
        // A throw from the app's own function is an expected failure mode, not a crash.
        return { success: false, error: err instanceof Error ? err.message : String(err) };
      }
      const data = mapResult ? mapResult(returned) : returned;
      return { success: true, data };
    },
  });
}
