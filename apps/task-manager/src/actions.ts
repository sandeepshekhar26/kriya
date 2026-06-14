/**
 * The task-manager affordances, declared once with `registerAction`.
 *
 * Same entry points a human button click and an agent tool call both go through.
 * Each handler owns the business logic and mutates the store.
 */

import { registerAction } from "@agent-native/core";
import { nextId, store, type Priority, type Task } from "./store";

const PRIORITIES: Priority[] = ["low", "medium", "high"];

export const createTask = registerAction<
  { title: string; priority?: Priority },
  { id: string }
>({
  id: "create_task",
  description: "Create a new task with a title and an optional priority (low/medium/high).",
  parameters: {
    title: { type: "string", description: "Short title of the task.", required: true },
    priority: {
      type: "string",
      description: "Priority of the task.",
      enum: PRIORITIES,
    },
  },
  permissions: ["write:tasks"],
  handler: (params) => {
    const task: Task = {
      id: nextId(),
      title: params.title,
      done: false,
      priority: params.priority ?? "medium",
      createdAt: Date.now(),
    };
    store.set((prev) => ({ tasks: [...prev.tasks, task] }));
    return { success: true, data: { id: task.id } };
  },
});

export const completeTask = registerAction<{ id: string }, { id: string }>({
  id: "complete_task",
  description: "Mark a task as done by id.",
  parameters: {
    id: { type: "string", description: "Id of the task to complete.", required: true },
  },
  permissions: ["write:tasks"],
  handler: (params) => {
    let found = false;
    store.set((prev) => ({
      tasks: prev.tasks.map((t) => {
        if (t.id !== params.id) return t;
        found = true;
        return { ...t, done: true };
      }),
    }));
    return found
      ? { success: true, data: { id: params.id } }
      : { success: false, error: `No task with id "${params.id}".` };
  },
});

export const setPriority = registerAction<
  { id: string; priority: Priority },
  { id: string }
>({
  id: "set_priority",
  description: "Change a task's priority (low/medium/high) by id.",
  parameters: {
    id: { type: "string", description: "Id of the task.", required: true },
    priority: {
      type: "string",
      description: "New priority level.",
      enum: PRIORITIES,
      required: true,
    },
  },
  permissions: ["write:tasks"],
  handler: (params) => {
    let found = false;
    store.set((prev) => ({
      tasks: prev.tasks.map((t) => {
        if (t.id !== params.id) return t;
        found = true;
        return { ...t, priority: params.priority };
      }),
    }));
    return found
      ? { success: true, data: { id: params.id } }
      : { success: false, error: `No task with id "${params.id}".` };
  },
});

export const deleteTask = registerAction<{ id: string }, { id: string }>({
  id: "delete_task",
  description: "Delete a task by id. Destructive — requires human approval.",
  parameters: {
    id: { type: "string", description: "Id of the task to delete.", required: true },
  },
  permissions: ["delete:tasks"],
  handler: (params) => {
    const before = store.getState().tasks.length;
    store.set((prev) => ({ tasks: prev.tasks.filter((t) => t.id !== params.id) }));
    const removed = before !== store.getState().tasks.length;
    return removed
      ? { success: true, data: { id: params.id } }
      : { success: false, error: `No task with id "${params.id}".` };
  },
});

export const clearCompleted = registerAction<Record<string, never>, { removed: number }>({
  id: "clear_completed",
  description:
    "Remove every completed task in one call. Bulk destructive — requires human approval.",
  parameters: {},
  permissions: ["delete:tasks"],
  handler: () => {
    const before = store.getState().tasks.length;
    store.set((prev) => ({ tasks: prev.tasks.filter((t) => !t.done) }));
    const removed = before - store.getState().tasks.length;
    return { success: true, data: { removed } };
  },
});
