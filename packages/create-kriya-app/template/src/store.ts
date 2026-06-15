/**
 * Single source of truth — a plain observable read by both React and the
 * registered action handlers (which run outside the React tree).
 */

import { useSyncExternalStore } from "react";

export interface AppData {
  count: number;
}

type Listener = () => void;

let state: AppData = { count: 0 };
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

export function useCount(): number {
  return useSyncExternalStore(store.subscribe, () => store.getState().count);
}
