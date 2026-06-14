/**
 * Single source of truth for the task list — a plain observable read by both React
 * and the registered action handlers (which run outside the React tree).
 */

import { useSyncExternalStore } from "react";

export type Priority = "low" | "medium" | "high";

export interface Task {
  id: string;
  title: string;
  done: boolean;
  priority: Priority;
  createdAt: number;
}

export interface AppData {
  tasks: Task[];
}

type Listener = () => void;

let state: AppData = { tasks: [] };
const listeners = new Set<Listener>();

function emit() {
  for (const l of listeners) l();
}

export const store = {
  getState(): AppData {
    return state;
  },
  set(updater: (prev: AppData) => AppData) {
    state = updater(state);
    emit();
  },
  subscribe(listener: Listener): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
  },
};

let counter = 0;
export function nextId(prefix = "task"): string {
  counter += 1;
  return `${prefix}-${Date.now().toString(36)}-${counter}`;
}

export function useTasks(): Task[] {
  return useSyncExternalStore(store.subscribe, () => store.getState().tasks);
}
