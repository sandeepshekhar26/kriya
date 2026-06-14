import { nextId, store, type Note } from "./store";

/** Five intentionally-uncategorized notes for the Phase 0 organize task. */
const SEED: Array<Pick<Note, "title" | "content">> = [
  { title: "Buy groceries", content: "Milk, eggs, bread, and coffee beans from the store." },
  { title: "Q3 planning meeting", content: "Prep slides for the client project deadline review." },
  { title: "Call the dentist", content: "Schedule a checkup appointment for next week." },
  { title: "App idea: tide tracker", content: "Maybe build a small surf-forecast app someday." },
  { title: "Reply to Sam's email", content: "Send the updated report to the client by Friday." },
];

export function seedNotes() {
  store.set(() => ({
    notes: SEED.map((s, i) => ({
      id: nextId(),
      title: s.title,
      content: s.content,
      category: "",
      createdAt: Date.now() + i,
    })),
  }));
}
