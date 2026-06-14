/**
 * Action-composition tests. A parent action's handler can call children via
 * `ctx.call(childId, params)`. Children run through the same validation + audit
 * path; the host sees only the parent call.
 */
import { afterEach, describe, expect, it } from "vitest";
import {
  MAX_COMPOSE_DEPTH,
  clearRegistry,
  dispatchAction,
  registerAction,
} from "../src/registry.js";
import type { ActionContext } from "../src/types.js";

afterEach(() => clearRegistry());

describe("action composition", () => {
  it("lets a parent action call a child via ctx.call", async () => {
    const trail: string[] = [];

    registerAction({
      id: "child",
      description: "child",
      parameters: { x: { type: "number", required: true } },
      handler: (p) => {
        trail.push(`child:${p.x}`);
        return { success: true, data: { doubled: (p.x as number) * 2 } };
      },
    });

    registerAction<{ start: number }, { steps: number }>({
      id: "parent",
      description: "parent",
      parameters: { start: { type: "number", required: true } },
      handler: async (p, ctx) => {
        trail.push(`parent:${p.start}`);
        const a = await ctx.call!<{ doubled: number }>("child", { x: p.start });
        const b = await ctx.call!<{ doubled: number }>("child", {
          x: a.data!.doubled,
        });
        return { success: true, data: { steps: 2, final: b.data!.doubled } as any };
      },
    });

    const res = await dispatchAction("parent", { start: 3 }, { caller: "human" });
    expect(res.success).toBe(true);
    expect((res.data as any).final).toBe(12);
    expect(trail).toEqual(["parent:3", "child:3", "child:6"]);
  });

  it("validates child params before running the child handler", async () => {
    registerAction({
      id: "needs_id",
      description: "d",
      parameters: { id: { type: "string", required: true } },
      handler: () => ({ success: true }),
    });
    registerAction({
      id: "wrap",
      description: "d",
      parameters: {},
      handler: async (_p, ctx) => {
        const r = await ctx.call!("needs_id", {});
        return r;
      },
    });

    const res = await dispatchAction("wrap", {}, { caller: "human" });
    expect(res.success).toBe(false);
    expect(res.error).toMatch(/Invalid parameters/);
  });

  it("returns a failed result (not a throw) on cycles", async () => {
    registerAction({
      id: "a",
      description: "d",
      parameters: {},
      handler: async (_p, ctx) => (await ctx.call!("b", {})) as any,
    });
    registerAction({
      id: "b",
      description: "d",
      parameters: {},
      handler: async (_p, ctx) => (await ctx.call!("a", {})) as any,
    });

    const res = await dispatchAction("a", {}, { caller: "human" });
    expect(res.success).toBe(false);
    expect(res.error).toMatch(/Composition cycle/);
    expect(res.error).toContain("a -> b -> a");
  });

  it("rejects calls that exceed MAX_COMPOSE_DEPTH", async () => {
    // Build a chain of N actions where each calls the next.
    const depth = MAX_COMPOSE_DEPTH + 2;
    for (let i = 0; i < depth; i++) {
      const next = `lvl_${i + 1}`;
      registerAction({
        id: `lvl_${i}`,
        description: "d",
        parameters: {},
        handler: async (_p, ctx) => {
          if (!ctx.call) return { success: true };
          return ctx.call(next, {}) as any;
        },
      });
    }
    registerAction({
      id: `lvl_${depth}`,
      description: "d",
      parameters: {},
      handler: () => ({ success: true }),
    });

    const res = await dispatchAction("lvl_0", {}, { caller: "human" });
    expect(res.success).toBe(false);
    expect(res.error).toMatch(/depth 8 exceeded/);
  });

  it("exposes the chain so a handler can inspect its origin", async () => {
    let observed: readonly string[] | undefined;
    registerAction({
      id: "leaf",
      description: "d",
      parameters: {},
      handler: (_p, ctx: ActionContext) => {
        observed = ctx.chain;
        return { success: true };
      },
    });
    registerAction({
      id: "branch",
      description: "d",
      parameters: {},
      handler: async (_p, ctx) => (await ctx.call!("leaf", {})) as any,
    });

    await dispatchAction("branch", {}, { caller: "human" });
    expect(observed).toEqual(["branch", "leaf"]);
  });

  it("captures child handler exceptions as failed results, not throws", async () => {
    registerAction({
      id: "bad",
      description: "d",
      parameters: {},
      handler: () => {
        throw new Error("nope");
      },
    });
    registerAction({
      id: "wrap_err",
      description: "d",
      parameters: {},
      handler: async (_p, ctx) => {
        const r = await ctx.call!("bad", {});
        // The wrapper just bubbles whatever the child returned.
        return r;
      },
    });
    const res = await dispatchAction("wrap_err", {}, { caller: "human" });
    expect(res).toEqual({ success: false, error: "nope" });
  });
});
