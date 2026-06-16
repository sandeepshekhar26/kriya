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

  // Actual stores payee/category as foreign-key UUIDs. Agents need names to decide, so
  // enrich every returned transaction with `payee_name` and `category_name` in a single fetch.
  const listTransactions = async (
    accountId: string,
    startDate: string,
    endDate: string,
  ) => {
    const [txs, payees, categories] = await Promise.all([
      actual.getTransactions(accountId, startDate, endDate),
      actual.getPayees(),
      actual.getCategories(),
    ]);
    const payeeName = new Map(payees.map((p) => [p.id, p.name]));
    const categoryName = new Map(categories.map((c) => [c.id, c.name]));
    return txs.map((t) => ({
      ...t,
      payee_name: t.payee_name ?? (t as { payee?: string }).payee
        ? payeeName.get((t as { payee?: string }).payee ?? "") ?? null
        : null,
      category_name: t.category ? categoryName.get(t.category) ?? null : null,
    }));
  };

  wrapAction(listTransactions, {
    id: "list_transactions",
    description:
      "List transactions for an account within a date range (YYYY-MM-DD). Each row includes payee_name and category_name resolved from Actual's internal UUIDs.",
    parameters: { accountId: str, startDate: str, endDate: str },
    mapParams: (p) => [p.accountId, p.startDate, p.endDate],
  });

  wrapAction(actual.getCategories, {
    id: "list_categories",
    description: "List all budget categories with their ids and names.",
  });

  // Resolve a human-readable category name to Actual's UUID before updating, so the agent
  // can pass either an id or a name (which it learns from list_categories / list_transactions).
  const uuidLike = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  const categorize = async (id: string, category: string) => {
    let categoryId = category;
    if (!uuidLike.test(category)) {
      const all = await actual.getCategories();
      const match = all.find((c) => c.name.toLowerCase() === category.toLowerCase());
      if (!match) {
        throw new Error(
          `unknown category "${category}". Call list_categories first to see valid names.`,
        );
      }
      categoryId = match.id;
    }
    return actual.updateTransaction(id, { category: categoryId });
  };

  wrapAction(categorize, {
    id: "categorize_transaction",
    description:
      "Assign a category to a transaction. `category` may be either a category UUID or a category name (matched case-insensitively).",
    parameters: { id: str, category: str },
    permissions: ["write:transactions"],
    mapParams: (p) => [p.id, p.category],
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
