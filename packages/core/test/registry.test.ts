import { afterEach, describe, expect, it } from "vitest";
import {
  ActionValidationError,
  clearRegistry,
  dispatchAction,
  getToolSchemas,
  listActionIds,
  registerAction,
} from "../src/registry.js";

afterEach(() => clearRegistry());

describe("registerAction", () => {
  it("registers and returns a callable handle", async () => {
    const action = registerAction<{ title: string }, { id: string }>({
      id: "create_thing",
      description: "Create a thing.",
      parameters: { title: { type: "string", required: true } },
      handler: (p) => ({ success: true, data: { id: `thing:${p.title}` } }),
    });
    expect(listActionIds()).toEqual(["create_thing"]);
    const res = await action.call({ title: "x" });
    expect(res).toEqual({ success: true, data: { id: "thing:x" } });
  });

  it("rejects invalid ids", () => {
    expect(() =>
      registerAction({ id: "Bad-Id", description: "d", parameters: {}, handler: () => ({ success: true }) })
    ).toThrow(ActionValidationError);
  });

  it("rejects duplicate ids", () => {
    const def = { id: "dup", description: "d", parameters: {}, handler: () => ({ success: true }) };
    registerAction(def);
    expect(() => registerAction(def)).toThrow(/already registered/);
  });
});

describe("dispatchAction", () => {
  it("validates params before running the handler", async () => {
    let ran = false;
    registerAction({
      id: "needs_title",
      description: "d",
      parameters: { title: { type: "string", required: true } },
      handler: () => {
        ran = true;
        return { success: true };
      },
    });
    const res = await dispatchAction("needs_title", {}, { caller: "agent" });
    expect(res.success).toBe(false);
    expect(res.error).toMatch(/Invalid parameters/);
    expect(ran).toBe(false);
  });

  it("returns a clean error for unknown actions", async () => {
    const res = await dispatchAction("nope", {}, { caller: "agent" });
    expect(res).toEqual({ success: false, error: 'Unknown action "nope".' });
  });

  it("captures handler exceptions as failed results", async () => {
    registerAction({
      id: "boom",
      description: "d",
      parameters: {},
      handler: () => {
        throw new Error("kaboom");
      },
    });
    const res = await dispatchAction("boom", {}, { caller: "agent" });
    expect(res).toEqual({ success: false, error: "kaboom" });
  });
});

describe("getToolSchemas", () => {
  it("emits standards-compliant JSON Schema: required is a name list, never a per-property boolean", () => {
    registerAction({
      id: "edit_thing",
      version: 2,
      description: "Edit a thing.",
      parameters: { id: { type: "string", required: true }, name: { type: "string" } },
      permissions: ["write:things"],
      handler: () => ({ success: true }),
    });
    expect(getToolSchemas()).toEqual([
      {
        name: "edit_thing",
        version: 2,
        description: "Edit a thing.",
        permissions: ["write:things"],
        inputSchema: {
          type: "object",
          // the per-property `required: true` hint is lifted to the object-level array below,
          // not emitted inside the property (strict validators reject the boolean form).
          properties: { id: { type: "string" }, name: { type: "string" } },
          required: ["id"],
        },
      },
    ]);
  });
});
