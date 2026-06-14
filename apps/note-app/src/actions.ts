/**
 * The note app's affordances, declared once with `registerAction`.
 *
 * These are the SAME entry points a human button click and an agent tool call both go
 * through. Each handler owns the business logic and mutates the store. The Rust host
 * never sees this code — it only sees the schemas emitted by `getToolSchemas()`.
 */

import { registerAction } from "@agent-native/core";
import { nextId, store, type Note } from "./store";

export const createNote = registerAction<
  { title: string; content?: string; category?: string },
  { id: string }
>({
  id: "create_note",
  description: "Create a new note with a title and optional content and category.",
  parameters: {
    title: { type: "string", description: "Short title of the note.", required: true },
    content: { type: "string", description: "Body text of the note." },
    category: { type: "string", description: "Optional category label." },
  },
  permissions: ["write:notes"],
  handler: (params) => {
    const note: Note = {
      id: nextId(),
      title: params.title,
      content: params.content ?? "",
      category: params.category ?? "",
      createdAt: Date.now(),
    };
    store.set((prev) => ({ notes: [...prev.notes, note] }));
    return { success: true, data: { id: note.id } };
  },
});

export const editNote = registerAction<
  { id: string; title?: string; content?: string; category?: string },
  { id: string }
>({
  id: "edit_note",
  description:
    "Edit an existing note by id. Provide any of title, content, or category to change it. " +
    "Use this to assign a category when organizing notes.",
  parameters: {
    id: { type: "string", description: "Id of the note to edit.", required: true },
    title: { type: "string", description: "New title." },
    content: { type: "string", description: "New body text." },
    category: { type: "string", description: "New category label." },
  },
  permissions: ["write:notes"],
  handler: (params) => {
    let found = false;
    store.set((prev) => ({
      notes: prev.notes.map((n) => {
        if (n.id !== params.id) return n;
        found = true;
        return {
          ...n,
          title: params.title ?? n.title,
          content: params.content ?? n.content,
          category: params.category ?? n.category,
        };
      }),
    }));
    return found
      ? { success: true, data: { id: params.id } }
      : { success: false, error: `No note with id "${params.id}".` };
  },
});

export const deleteNote = registerAction<{ id: string }, { id: string }>({
  id: "delete_note",
  description: "Delete a note by id.",
  parameters: {
    id: { type: "string", description: "Id of the note to delete.", required: true },
  },
  permissions: ["delete:notes"],
  handler: (params) => {
    const before = store.getState().notes.length;
    store.set((prev) => ({ notes: prev.notes.filter((n) => n.id !== params.id) }));
    const removed = before !== store.getState().notes.length;
    return removed
      ? { success: true, data: { id: params.id } }
      : { success: false, error: `No note with id "${params.id}".` };
  },
});
