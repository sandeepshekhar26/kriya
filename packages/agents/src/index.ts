/**
 * kriya-agents — govern in-process agent-framework tool calls with kriya.
 *
 * Every tool call an agent makes is routed through kriya's action gates (policy → approval → budget)
 * and, when it runs, signs an Ed25519, hash-chained receipt — **without an MCP hop and without
 * inverting the framework's control flow**. The framework keeps driving its own loop; kriya just
 * governs and signs each call by delegating to the runtime's `kriya-govern` binary (the one Signer).
 *
 * Start with {@link govern} (framework-agnostic) or a per-framework adapter:
 *   - `kriya-agents/langgraph` — wrap a LangChain/LangGraph tool.
 *
 * Honest ceiling: in-process governance is cooperative (a hostile process can skip it — that's what
 * containment/B14 is for), and this lane governs the action tier + signs; it does not see the tool's
 * own egress.
 */

export {
  GovernClient,
  govern,
  GovernDenied,
  type GovernClientOptions,
  type GovernDecision,
  type CheckResult,
  type SignedReceipt,
  type GovernHooks,
} from "./govern-client.js";
