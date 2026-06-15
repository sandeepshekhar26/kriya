import { describe, expect, it } from "vitest";
import { scaffoldWrappers } from "../src/codemod.js";

const opts = { importPath: "./actions.js" };

describe("scaffoldWrappers", () => {
  it("infers a single object parameter from a type literal and omits mapParams", async () => {
    const out = await scaffoldWrappers(
      `export function createNote(opts: { title: string; pinned?: boolean }) { return opts; }`,
      opts,
    );
    expect(out).toContain('import { createNote } from "./actions.js";');
    expect(out).toContain("wrapAction(createNote, {");
    expect(out).toContain('id: "create_note"');
    expect(out).toContain("title: { type: \"string\", required: true }");
    expect(out).toContain("pinned: { type: \"boolean\" }");
    // A single object param maps straight through — no mapParams.
    expect(out).not.toContain("mapParams");
  });

  it("generates mapParams for positional parameters and infers their types", async () => {
    const out = await scaffoldWrappers(
      `export function addItem(name: string, qty: number) { return name; }`,
      opts,
    );
    expect(out).toContain('id: "add_item"');
    expect(out).toContain("name: { type: \"string\", required: true }");
    expect(out).toContain("qty: { type: \"number\", required: true }");
    expect(out).toContain("mapParams: (p) => [p.name, p.qty]");
  });

  it("detects exported arrow-function consts", async () => {
    const out = await scaffoldWrappers(
      `export const deleteThing = (id: number): void => {};`,
      opts,
    );
    expect(out).toContain("wrapAction(deleteThing, {");
    expect(out).toContain('id: "delete_thing"');
    expect(out).toContain("mapParams: (p) => [p.id]");
  });

  it("ignores non-exported functions", async () => {
    const out = await scaffoldWrappers(
      `function helper() {}\nexport function realOne() {}`,
      opts,
    );
    expect(out).toContain("wrapAction(realOne, {");
    expect(out).not.toContain("helper");
  });

  it("uses a JSDoc comment as the description, else a TODO", async () => {
    const withDoc = await scaffoldWrappers(
      `/** Create a brand new note. */\nexport function createNote() {}`,
      opts,
    );
    expect(withDoc).toContain('description: "Create a brand new note."');

    const withoutDoc = await scaffoldWrappers(`export function createNote() {}`, opts);
    expect(withoutDoc).toContain("TODO: describe create_note");
  });

  it("infers array parameters with an items type", async () => {
    const out = await scaffoldWrappers(`export function setTags(tags: string[]) {}`, opts);
    expect(out).toContain('tags: { type: "array", items: "string", required: true }');
  });

  it("returns an empty string when there is nothing to wrap", async () => {
    expect(await scaffoldWrappers(`const x = 1;`, opts)).toBe("");
  });

  it("scaffolds multiple functions into one importable module", async () => {
    const out = await scaffoldWrappers(
      `export function createNote(o: { title: string }) {}\nexport function deleteNote(id: number) {}`,
      opts,
    );
    expect(out).toContain('import { createNote, deleteNote } from "./actions.js";');
    expect(out.match(/wrapAction\(/g)).toHaveLength(2);
  });
});
