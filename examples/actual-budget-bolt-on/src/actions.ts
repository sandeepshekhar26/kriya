/**
 * THE BOLT-ON. This is the entire integration: wrap Actual Budget's existing in-process
 * functions as governed, agent-callable actions — no rewrite, no new API. Each `wrapAction`
 * maps the agent's params onto Actual's function and normalizes the result; the host enforces
 * the policy (which of these need human approval) and signs an audit receipt per call.
 *
 * Note how little there is: a real existing app gains a governed agent surface in under 50
 * lines of wiring. That is the pitch.
 */

import { wrapAction } from "kriya-core";
import type { ActualApi } from "./actual-api.js";

const str = { type: "string", required: true } as const;

export function registerActualActions(actual: ActualApi): void {
  wrapAction(actual.getAccounts, { id: "list_accounts", description: "List all budget accounts." });

  wrapAction(actual.getTransactions, {
    id: "list_transactions",
    description: "List transactions for an account within a date range (YYYY-MM-DD).",
    parameters: { accountId: str, startDate: str, endDate: str },
    mapParams: (p) => [p.accountId, p.startDate, p.endDate],
  });

  wrapAction(actual.updateTransaction, {
    id: "categorize_transaction",
    description: "Assign a category to a transaction (the everyday reconciliation task).",
    parameters: { id: str, category: str },
    permissions: ["write:transactions"],
    mapParams: (p) => [p.id, { category: p.category }],
  });

  wrapAction(actual.setBudgetAmount, {
    id: "set_budget",
    description: "Set the budgeted amount (minor units) for a category in a month (YYYY-MM).",
    parameters: { month: str, categoryId: str, amount: { type: "number", required: true } },
    permissions: ["write:budget"],
    mapParams: (p) => [p.month, p.categoryId, p.amount],
  });

  // Guarded by the policy (require_approval): destroying a record / moving money between accounts.
  wrapAction(actual.deleteTransaction, {
    id: "delete_transaction",
    description: "Permanently delete a transaction.",
    parameters: { id: str },
    permissions: ["delete:transactions"],
    mapParams: (p) => [p.id],
  });

  wrapAction(actual.closeAccount, {
    id: "close_account",
    description: "Close an account, transferring its balance to another account.",
    parameters: { id: str, transferAccountId: { type: "string" }, transferCategoryId: { type: "string" } },
    permissions: ["move:money"],
    mapParams: (p) => [p.id, p.transferAccountId, p.transferCategoryId],
  });
}
