/**
 * Frontend half of the agent loop. The Rust host decides which action to call and why;
 * this module executes that decision through the registry and reports the result + fresh
 * state back. Collects host telemetry for the inspector panel.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  AgentCommands,
  AgentEvents,
  dispatchAction,
  getToolSchemas,
  type AgentActionRequest,
  type AgentApprovalRequest,
  type AgentDone,
  type AgentLog,
} from "kriya-core";
import { store } from "./store";
import { useSyncExternalStore } from "react";

export interface InspectorEntry extends AgentLog {
  ts: number;
}

let log: InspectorEntry[] = [];
const logListeners = new Set<() => void>();
function pushLog(entry: AgentLog) {
  log = [...log, { ...entry, ts: Date.now() }];
  for (const l of logListeners) l();
}
export function clearLog() {
  log = [];
  for (const l of logListeners) l();
}
export function useInspectorLog(): InspectorEntry[] {
  return useSyncExternalStore(
    (cb) => {
      logListeners.add(cb);
      return () => logListeners.delete(cb);
    },
    () => log
  );
}

let running = false;
const runListeners = new Set<() => void>();
function setRunning(v: boolean) {
  running = v;
  for (const l of runListeners) l();
}
export function useAgentRunning(): boolean {
  return useSyncExternalStore(
    (cb) => {
      runListeners.add(cb);
      return () => runListeners.delete(cb);
    },
    () => running
  );
}

let pendingApproval: AgentApprovalRequest | null = null;
const approvalListeners = new Set<() => void>();
function setPendingApproval(req: AgentApprovalRequest | null) {
  pendingApproval = req;
  for (const l of approvalListeners) l();
}
export function usePendingApproval(): AgentApprovalRequest | null {
  return useSyncExternalStore(
    (cb) => {
      approvalListeners.add(cb);
      return () => approvalListeners.delete(cb);
    },
    () => pendingApproval
  );
}

export async function respondToApproval(approved: boolean): Promise<void> {
  const req = pendingApproval;
  if (!req) return;
  setPendingApproval(null);
  pushLog({
    stepId: req.stepId,
    level: approved ? "info" : "warn",
    message: `human ${approved ? "approved" : "denied"} ${req.actionId}`,
  });
  await invoke(AgentCommands.ApprovalResponse, {
    response: { stepId: req.stepId, approved },
  });
}

let onDone: ((done: AgentDone) => void) | null = null;
let wired = false;
async function ensureWired() {
  if (wired) return;
  wired = true;

  await listen<AgentActionRequest>(AgentEvents.Action, async (event) => {
    const { stepId, actionId, params, reasoning } = event.payload;
    pushLog({ stepId, level: "decision", message: `${actionId}`, detail: { params, reasoning } });
    const result = await dispatchAction(actionId, params, { caller: "agent", stepId });
    if (!result.success) {
      pushLog({ stepId, level: "error", message: `${actionId} failed: ${result.error}` });
    }
    await invoke(AgentCommands.ActionResult, {
      result: {
        stepId,
        success: result.success,
        data: result.data,
        error: result.error,
        state: store.getState(),
      },
    });
  });

  await listen<AgentApprovalRequest>(AgentEvents.Approval, (event) => {
    setPendingApproval(event.payload);
  });

  await listen<AgentLog>(AgentEvents.Log, (event) => pushLog(event.payload));

  await listen<AgentDone>(AgentEvents.Done, (event) => {
    onDone?.(event.payload);
    onDone = null;
    setPendingApproval(null);
    bumpRunCount();
  });
}

// ---- completed-run counter (drives MemoryPanel refresh) ---------------------

let runCount = 0;
const runCountListeners = new Set<() => void>();
function bumpRunCount() {
  runCount += 1;
  for (const l of runCountListeners) l();
}
export function useRunCount(): number {
  return useSyncExternalStore(
    (cb) => {
      runCountListeners.add(cb);
      return () => runCountListeners.delete(cb);
    },
    () => runCount
  );
}

export async function runAgentTask(goal: string): Promise<AgentDone> {
  await ensureWired();
  setRunning(true);
  pushLog({ level: "info", message: `goal: ${goal}` });

  const done = new Promise<AgentDone>((resolve) => {
    onDone = resolve;
  });

  try {
    await invoke(AgentCommands.Start, {
      req: {
        goal,
        state: store.getState(),
        tools: getToolSchemas(),
      },
    });
    const result = await done;
    pushLog({ level: "info", message: `done — ${result.summary} (${result.steps} steps)` });
    return result;
  } catch (err) {
    pushLog({ level: "error", message: `agent run failed: ${String(err)}` });
    throw err;
  } finally {
    setRunning(false);
  }
}
