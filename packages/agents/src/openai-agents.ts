/**
 * `kriya-agents/openai-agents` — govern an OpenAI Agents SDK (JS/TS) function tool with kriya.
 *
 * The OpenAI Agents SDK defines a function tool with
 * `tool({ name, description, parameters, execute })`, where the SDK parses the model's arguments
 * against `parameters` (a zod schema) and calls `execute(input, context?, details?)` with the parsed
 * object (verified 2026-07-22 against the installed `@openai/agents` — `FunctionTool.invoke` parses
 * the arg JSON, then calls `execute`). Governing it is governing that `execute` — this adapter is a
 * thin shim over the framework-agnostic {@link govern} with no dependency on `@openai/agents`: you
 * wrap your `execute`, then pass it to the SDK's `tool()` yourself, so name/parameters (what the
 * model sees) are unchanged.
 *
 * ```ts
 * import { tool } from "@openai/agents";
 * import { z } from "zod";
 * import { GovernClient } from "kriya-agents";
 * import { governExecute } from "kriya-agents/openai-agents";
 *
 * const client = new GovernClient({ actor: "openai-agents", auditLog: "…/.kriya/audit/oai.jsonl" });
 * const add = tool({
 *   name: "add", description: "Add two numbers",
 *   parameters: z.object({ a: z.number(), b: z.number() }),
 *   execute: governExecute(client, "add", async ({ a, b }) => `${a + b}`),
 * });
 * ```
 *
 * A denied / approval-refused / over-budget call throws {@link GovernDenied}; the SDK surfaces it as
 * that tool's error (or routes it through your tool's `errorFunction`), so the model can adapt.
 *
 * Honest ceiling (state it in your app too): in-process governance is cooperative — a hostile agent
 * process can skip it (that is what launch-under containment / B14 is for) — and this lane governs the
 * action tier + signs the receipt; it does not see the tool's own outbound network calls.
 */

import { type GovernClient, type GovernHooks, govern } from "./govern-client.js";

/** The OpenAI Agents SDK `execute` shape: `(input, context?, details?) => result`. */
export type OpenAIToolExecute<A, R> = (input: A, context?: unknown, details?: unknown) => R | Promise<R>;

/**
 * Wrap an OpenAI Agents SDK tool `execute` so every invocation is policy / approval / budget gated
 * and (when it runs) signs a receipt. `actionId` is the id policy rules match on — use the tool's
 * `name`. The `context`/`details` the SDK passes are forwarded to your original `execute` untouched.
 */
export function governExecute<A extends Record<string, unknown>, R>(
  client: GovernClient,
  actionId: string,
  execute: OpenAIToolExecute<A, R>,
  hooks?: GovernHooks,
): OpenAIToolExecute<A, R> {
  return (input: A, context?: unknown, details?: unknown): Promise<R> =>
    // Reuse the framework-agnostic core; bind the SDK's context/details per call so the original
    // execute still receives them. A non-allow decision throws GovernDenied (the SDK surfaces it).
    govern<A, R>(client, actionId, (params: A) => execute(params, context, details), hooks)(input);
}
