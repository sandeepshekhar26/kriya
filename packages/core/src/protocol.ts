/**
 * The agent loop protocol shared by the frontend and the Rust agent host.
 *
 * Transport is Tauri IPC (`invoke` for app→host calls, `event` for host→app pushes),
 * but the message shapes below are transport-agnostic and mirror JSON-RPC 2.0 semantics:
 * each agent turn is a request/response pair keyed by `stepId`.
 *
 * Flow for one task:
 *   app  --invoke "agent_start"--------▶ host        { goal, state, tools }
 *   host --event  "agent://action"-----▶ app         { stepId, actionId, params, reasoning }
 *   app  --invoke "agent_action_result"▶ host        { stepId, success, data, state }
 *   ...repeat per step...
 *   host --event  "agent://done"-------▶ app         { summary, steps }
 *   host --event  "agent://log"--------▶ app         (inspector telemetry, any time)
 */

import type { ToolSchema } from "./types.js";

/** Application state snapshot the agent reasons over. App-defined, JSON-serializable. */
export type AppState = Record<string, unknown>;

/** app → host: begin an autonomous task. */
export interface AgentStartRequest {
  goal: string;
  state: AppState;
  tools: ToolSchema[];
}

/** host → app: the host (on behalf of the agent) wants an action executed. */
export interface AgentActionRequest {
  stepId: string;
  actionId: string;
  params: Record<string, unknown>;
  /** The agent's natural-language justification, surfaced in the inspector. */
  reasoning: string;
}

/** app → host: result of executing the requested action, plus the refreshed state. */
export interface AgentActionResult {
  stepId: string;
  success: boolean;
  data?: unknown;
  error?: string;
  /** The new app state after the handler ran, so the host can plan the next step. */
  state: AppState;
}

/** host → app: the task is complete. */
export interface AgentDone {
  summary: string;
  /** Number of action steps taken. */
  steps: number;
}

/** host → app: structured telemetry for the inspector (reasoning, decisions, errors). */
export interface AgentLog {
  stepId?: string;
  level: "info" | "decision" | "warn" | "error";
  message: string;
  detail?: unknown;
}

/** Tauri event channel names (host → app). */
export const AgentEvents = {
  Action: "agent://action",
  Done: "agent://done",
  Log: "agent://log",
} as const;

/** Tauri command names (app → host). */
export const AgentCommands = {
  Start: "agent_start",
  ActionResult: "agent_action_result",
} as const;
