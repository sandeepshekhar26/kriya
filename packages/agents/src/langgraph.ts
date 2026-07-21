/**
 * `kriya-agents/langgraph` — govern a LangGraph / LangChain.js tool with kriya.
 *
 * LangChain's `tool(fn, config)` wraps a plain async **function** into an agent-callable
 * `StructuredTool`; LangGraph's `ToolNode` then `invoke`s it. So governing LangGraph is governing
 * that function — this adapter is a thin shim over the framework-agnostic {@link govern}, with no
 * dependency on `@langchain/*` (it duck-types the tool interface, so it works across versions and
 * without pulling LangChain into this package).
 *
 * Two ways in:
 *   1. Wrap the function BEFORE you build the tool (recommended — version-proof):
 *        import { tool } from "@langchain/core/tools";
 *        import { governTool } from "kriya-agents/langgraph";
 *        const search = tool(governTool(client, "web_search", rawSearch),
 *                            { name: "web_search", description: "…", schema });
 *   2. Wrap an already-built tool instance (its `.invoke` is governed; name/schema/description are
 *      preserved so the model still sees the same tool):
 *        const governed = governLangGraphTool(client, existingTool);
 *
 * Honest ceiling (state it in your app too): in-process governance is cooperative — a hostile agent
 * process can skip it (that is what launch-under containment / B14 is for) — and this lane governs
 * the action tier + signs the receipt; it does not see the tool's own outbound network calls.
 */

import { govern, type GovernClient, type GovernHooks } from "./govern-client.js";

/**
 * Wrap the function you hand to LangChain's `tool()` so every invocation is policy/approval/budget
 * gated and (when it runs) signs a receipt. `actionId` is the stable id policy rules match on — use
 * the same string as the tool's `name`. Identical to {@link govern}; named for discoverability.
 */
export function governTool<P extends Record<string, unknown>, R>(
  client: GovernClient,
  actionId: string,
  fn: (input: P) => R | Promise<R>,
  hooks?: GovernHooks,
): (input: P) => Promise<R> {
  return govern(client, actionId, fn, hooks);
}

/** The minimal shape this adapter needs from a LangChain tool — a name and an `invoke`. */
export interface LangGraphToolLike {
  name: string;
  invoke: (input: unknown, ...rest: unknown[]) => unknown;
}

/**
 * Govern an already-constructed LangChain/LangGraph tool: returns a proxy whose `.invoke` is gated +
 * receipted, with every other property (`name`, `description`, `schema`, …) preserved so the model
 * and the `ToolNode` see an identical tool. `actionId` defaults to the tool's `name`.
 */
export function governLangGraphTool<T extends LangGraphToolLike>(
  client: GovernClient,
  tool: T,
  actionId: string = tool.name,
  hooks?: GovernHooks,
): T {
  const original = (input: unknown, ...rest: unknown[]) => tool.invoke(input, ...rest);
  return new Proxy(tool, {
    get(target, prop, receiver) {
      if (prop === "invoke") {
        // LangChain calls `tool.invoke(input, config?)`. The input is the receipt's params; the
        // config is passed through to the original untouched.
        return (input: unknown, ...rest: unknown[]) => {
          const params = (input && typeof input === "object" ? input : { input }) as Record<
            string,
            unknown
          >;
          return govern(client, actionId, () => original(input, ...rest), hooks)(params);
        };
      }
      return Reflect.get(target, prop, receiver);
    },
  });
}
