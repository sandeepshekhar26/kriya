import { nextId, store, type Task } from "./store";

const SEED: Array<Pick<Task, "title" | "priority" | "done">> = [
  { title: "Ship the v0.2 release", priority: "high", done: false },
  { title: "Reply to investor email", priority: "high", done: false },
  { title: "Refactor the auth module", priority: "medium", done: false },
  { title: "Write blog post draft", priority: "medium", done: true },
  { title: "Pick up dry cleaning", priority: "low", done: false },
  { title: "Update LinkedIn bio", priority: "low", done: true },
];

export function seedTasks() {
  store.set(() => ({
    tasks: SEED.map((s, i) => ({
      id: nextId(),
      title: s.title,
      priority: s.priority,
      done: s.done,
      createdAt: Date.now() + i,
    })),
  }));
}
