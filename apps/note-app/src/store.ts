/**
 * The note store: the app's single source of truth.
 *
 * It is a plain observable so that both React (via `useStore`) and the registered
 * action handlers (which run outside the React tree) read and mutate the same state.
 */

import { useSyncExternalStore } from "react";

export interface Note {
  id: string;
  title: string;
  content: string;
  /** Assigned by a human or by the agent via `edit_note`. Empty until categorized. */
  category: string;
  createdAt: number;
}

export interface AppData {
  notes: Note[];
}

type Listener = () => void;

let state: AppData = { notes: [] };
const listeners = new Set<Listener>();

function emit() {
  for (const l of listeners) l();
}

export const store = {
  getState(): AppData {
    return state;
  },
  /** Replace state immutably and notify subscribers. */
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
export function nextId(prefix = "note"): string {
  counter += 1;
  return `${prefix}-${Date.now().toString(36)}-${counter}`;
}

/** React binding. */
export function useNotes(): Note[] {
  return useSyncExternalStore(store.subscribe, () => store.getState().notes);
}
