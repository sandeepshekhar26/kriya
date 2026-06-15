import { afterEach, describe, expect, it } from "vitest";
import { clearRegistry, dispatchAction, getToolSchemas, listActionIds } from "../src/registry.js";
import { wrapAction } from "../src/wrap.js";

afterEach(() => clearRegistry());

describe("wrapAction", () => {
  it("registers an existing function and normalizes its return into an ActionResult", async () => {
    // A plain app function: takes an options object, returns a domain value, no framework knowledge.
    const createNote = (opts: { title: string }) => ({ id: 1, title: opts.title });

    const action = wrapAction(createNote, {
      id: "create_note",
      description: "Create a note.",
      parameters: { title: { type: "string", required: true } },
    });

    expect(listActionIds()).toEqual(["create_note"]);
    const res = await action.call({ title: "Groceries" });
    expect(res).toEqual({ success: true, data: { id: 1, title: "Groceries" } });
  });

  it("exposes the wrapped action in the MCP tool schemas", () => {
    wrapAction(() => 42, {
      id: "answer",
      description: "The answer.",
      permissions: ["read:meaning"],
    });
    const tool = getToolSchemas().find((t) => t.name === "answer");
    expect(tool).toBeDefined();
    expect(tool?.permissions).toEqual(["read:meaning"]);
  });

  it("maps the params object onto positional arguments", async () => {
    const seen: unknown[] = [];
    const addItem = (name: string, qty: number) => {
      seen.push([name, qty]);
      return `${qty}x ${name}`;
    };

    wrapAction(addItem, {
      id: "add_item",
      description: "Add an item.",
      parameters: { name: { type: "string", required: true }, qty: { type: "number" } },
      mapParams: (p: { name: string; qty: number }) => [p.name, p.qty],
    });

    const res = await dispatchAction("add_item", { name: "milk", qty: 2 }, { caller: "agent" });
    expect(seen).toEqual([["milk", 2]]);
    expect(res).toEqual({ success: true, data: "2x milk" });
  });

  it("awaits async functions", async () => {
    const fetchThing = async (id: number) => {
      await Promise.resolve();
      return { id, ok: true };
    };
    wrapAction(fetchThing, {
      id: "fetch_thing",
      description: "Fetch.",
      parameters: { id: { type: "number", required: true } },
      mapParams: (p: { id: number }) => [p.id],
    });
    const res = await dispatchAction("fetch_thing", { id: 7 }, { caller: "agent" });
    expect(res).toEqual({ success: true, data: { id: 7, ok: true } });
  });

  it("applies mapResult to the return value", async () => {
    const createUser = (name: string) => ({ name, secretToken: "xyz", id: 99 });
    wrapAction(createUser, {
      id: "create_user",
      description: "Create a user.",
      parameters: { name: { type: "string", required: true } },
      mapParams: (p: { name: string }) => [p.name],
      // Don't leak the token to the agent — surface only the id.
      mapResult: (u) => ({ id: (u as { id: number }).id }),
    });
    const res = await dispatchAction("create_user", { name: "Ada" }, { caller: "agent" });
    expect(res).toEqual({ success: true, data: { id: 99 } });
  });

  it("turns a thrown error into a failed ActionResult", async () => {
    const risky = () => {
      throw new Error("disk full");
    };
    wrapAction(risky, { id: "risky", description: "Risky." });
    const res = await dispatchAction("risky", {}, { caller: "agent" });
    expect(res).toEqual({ success: false, error: "disk full" });
  });

  it("still enforces parameter validation before calling the function", async () => {
    let called = false;
    const fn = () => {
      called = true;
      return "ok";
    };
    wrapAction(fn, {
      id: "needs_title",
      description: "Needs a title.",
      parameters: { title: { type: "string", required: true } },
    });
    // Missing required param → validation fails, the wrapped function is never called.
    const res = await dispatchAction("needs_title", {}, { caller: "agent" });
    expect(res.success).toBe(false);
    expect(res.error).toMatch(/Invalid parameters/);
    expect(called).toBe(false);
  });

  it("defaults to passing the whole params object as the single argument", async () => {
    const handler = (opts: Record<string, unknown>) => Object.keys(opts).length;
    wrapAction(handler, {
      id: "count_keys",
      description: "Count keys.",
      parameters: { a: { type: "string" }, b: { type: "string" } },
    });
    const res = await dispatchAction("count_keys", { a: "1", b: "2" }, { caller: "agent" });
    expect(res).toEqual({ success: true, data: 2 });
  });
});
