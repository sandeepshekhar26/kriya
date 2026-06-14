/**
 * Your app's affordances, declared once with `registerAction`.
 *
 * These are the SAME entry points a human button click and an agent tool call
 * both go through. Each handler owns the business logic and mutates the store.
 * The Rust host never sees this code — it only sees the schemas emitted by
 * `getToolSchemas()`.
 */

import { registerAction } from "@agent-native/core";
import { store } from "./store";

export const increment = registerAction<{ by?: number }, { count: number }>({
  id: "increment",
  description:
    "Add to the counter. Default step is 1. Use a larger `by` to close the gap to the goal faster.",
  parameters: {
    by: {
      type: "number",
      description: "How much to add (positive integer). Defaults to 1.",
    },
  },
  permissions: ["write:counter"],
  handler: (params) => {
    const by = Math.max(1, Math.floor(params.by ?? 1));
    store.set((prev) => ({ count: prev.count + by }));
    return { success: true, data: { count: store.getState().count } };
  },
});

export const setCounter = registerAction<{ value: number }, { count: number }>({
  id: "set_counter",
  description: "Set the counter to a specific non-negative integer.",
  parameters: {
    value: {
      type: "number",
      description: "New counter value (>= 0).",
      required: true,
    },
  },
  permissions: ["write:counter"],
  handler: (params) => {
    const v = Math.floor(params.value);
    if (!Number.isFinite(v) || v < 0) {
      return { success: false, error: "value must be a non-negative integer" };
    }
    store.set(() => ({ count: v }));
    return { success: true, data: { count: v } };
  },
});

export const resetCounter = registerAction<Record<string, never>, { count: number }>({
  id: "reset_counter",
  description: "Reset the counter to 0. Destructive — requires human approval.",
  parameters: {},
  permissions: ["delete:counter"],
  handler: () => {
    store.set(() => ({ count: 0 }));
    return { success: true, data: { count: 0 } };
  },
});
