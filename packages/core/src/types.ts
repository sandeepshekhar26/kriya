/**
 * Core type definitions for agent-native actions.
 *
 * An "action" is the unit an agent can call and a human can trigger. It is a typed,
 * permission-scoped, self-describing affordance — the desktop analogue of an MCP tool.
 */

/** The subset of JSON Schema we use to describe a parameter. Deliberately small. */
export type ParameterType = "string" | "number" | "boolean" | "array" | "object";

export interface ParameterSchema {
  type: ParameterType;
  /** Human/agent-readable description of the parameter. */
  description?: string;
  /** Whether the parameter must be supplied. Defaults to false. */
  required?: boolean;
  /** For `type: "array"`, the element type. */
  items?: ParameterType | ParameterSchema;
  /** For `type: "object"`, the nested property schemas. */
  properties?: Record<string, ParameterSchema>;
  /** Restrict to an enumerated set of values. */
  enum?: Array<string | number>;
}

/** A permission scope string, e.g. `"write:notes"` or `"delete:notes"`. */
export type Permission = string;

/** Context handed to an action handler at execution time. */
export interface ActionContext {
  /** Who triggered this action. */
  caller: "human" | "agent";
  /** Opaque id of the agent step this call belongs to, when caller is an agent. */
  stepId?: string;
}

/** What an action handler returns. */
export interface ActionResult<T = unknown> {
  success: boolean;
  /** Action-specific payload (e.g. the created note's id). */
  data?: T;
  /** Present when `success` is false. */
  error?: string;
}

/** The definition a developer passes to `registerAction`. */
export interface ActionDefinition<
  P extends Record<string, unknown> = Record<string, unknown>,
  R = unknown
> {
  /** Stable unique id, e.g. `"create_note"`. */
  id: string;
  /** Schema version of this action. Bump when parameters change incompatibly. Defaults to 1. */
  version?: number;
  /** One-line description an agent reads to decide whether to call this. */
  description: string;
  /** Named parameter schemas. */
  parameters: Record<string, ParameterSchema>;
  /** Permission scopes this action requires. Checked by the Rust host. */
  permissions?: Permission[];
  /** The app's business logic. Runs in the frontend; mutates app state. */
  handler: (params: P, ctx: ActionContext) => Promise<ActionResult<R>> | ActionResult<R>;
}

/**
 * MCP-compatible tool schema. This is what the registry emits for the agent host
 * to ingest — it carries everything an agent needs *except* the handler.
 */
export interface ToolSchema {
  name: string;
  version: number;
  description: string;
  permissions: Permission[];
  inputSchema: {
    type: "object";
    properties: Record<string, ParameterSchema>;
    required: string[];
  };
}
