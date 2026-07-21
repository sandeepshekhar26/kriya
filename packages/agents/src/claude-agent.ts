/**
 * `kriya-agents/claude-agent` — govern a Claude Agent SDK (TypeScript) custom tool with kriya.
 *
 * The Claude Agent SDK defines an in-process tool with
 * `tool(name, description, inputSchema, handler)`, where `handler(args, extra) => { content, isError? }`
 * (verified 2026-07-21 against the Claude Agent SDK TS reference). Governing it is governing that
 * `handler` — this adapter is a thin shim over the framework-agnostic {@link govern} with no
 * dependency on `@anthropic-ai/claude-agent-sdk`: you wrap your handler, then pass it to the SDK's
 * `tool()` yourself, so name/description/schema (what the model sees) are unchanged.
 *
 * ```ts
 * import { tool } from "@anthropic-ai/claude-agent-sdk";
 * import { z } from "zod";
 * import { GovernClient } from "kriya-agents";
 * import { governClaudeHandler } from "kriya-agents/claude-agent";
 *
 * const client = new GovernClient({ actor: "claude-agent", auditLog: "…/.kriya/audit/claude.jsonl" });
 * const search = tool(
 *   "search", "Search the web", { query: z.string() },
 *   governClaudeHandler(client, "search", async ({ query }) => ({
 *     content: [{ type: "text", text: `Results for: ${query}` }],
 *   })),
 * );
 * ```
 *
 * Honest ceiling (state it in your app too): in-process governance is cooperative — a hostile agent
 * process can skip it (that is what launch-under containment / B14 is for) — and this lane governs the
 * action tier + signs the receipt; it does not see the tool's own outbound network calls.
 */

import { type GovernClient, type GovernHooks, govern } from "./govern-client.js";

/** The `CallToolResult` a Claude Agent SDK tool handler returns. */
export interface ClaudeToolResult {
  content: unknown[];
  isError?: boolean;
  [key: string]: unknown;
}

/** A Claude Agent SDK tool handler: `(args, extra) => CallToolResult`. */
export type ClaudeToolHandler<A> = (args: A, extra: unknown) => ClaudeToolResult | Promise<ClaudeToolResult>;

/**
 * Wrap a Claude Agent SDK tool handler so every invocation is policy / approval / budget gated and
 * (when it runs) signs a receipt. `actionId` is the id policy rules match on — use the tool's name.
 * A denied / approval-refused / over-budget call returns an `isError` `CallToolResult` (so the model
 * sees the tool errored and adapts) — not a thrown exception, which is the idiomatic MCP tool shape.
 */
export function governClaudeHandler<A extends Record<string, unknown>>(
  client: GovernClient,
  actionId: string,
  handler: ClaudeToolHandler<A>,
  hooks?: GovernHooks,
): ClaudeToolHandler<A> {
  // Reuse the framework-agnostic core for the check/record + receipt semantics, then map its
  // GovernDenied throw onto the SDK's `{ isError: true }` result shape.
  const run = govern<A, ClaudeToolResult>(
    client,
    actionId,
    // The wrapped "tool" is the handler; its `isError` result counts as a failed call for the receipt.
    async (args: A) => {
      const result = await handler(args, undefined);
      if (result.isError) {
        // Signal failure to `govern` (so the receipt records success:false) without losing the
        // handler's own error content — re-thrown and re-wrapped below.
        throw new ClaudeToolError(result);
      }
      return result;
    },
    hooks,
  );
  return async (args: A, _extra: unknown): Promise<ClaudeToolResult> => {
    try {
      return await run(args);
    } catch (err) {
      if (err instanceof ClaudeToolError) return err.result; // the handler's own isError result
      // A GovernDenied (or any other pre-execution error) → an isError result the model can adapt to.
      const text = err instanceof Error ? err.message : String(err);
      return { content: [{ type: "text", text }], isError: true };
    }
  };
}

/** Internal: carries a handler's own `isError` result through `govern`'s failure path. */
class ClaudeToolError extends Error {
  constructor(readonly result: ClaudeToolResult) {
    super("claude tool returned isError");
    this.name = "ClaudeToolError";
  }
}
