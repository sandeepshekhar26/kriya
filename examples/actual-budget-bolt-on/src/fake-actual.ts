/**
 * An in-memory stand-in for Actual's API, used when `ACTUAL_FAKE=1`. It lets anyone run the
 * full governed demo — policy, approval, budget, signed audit, all flowing through kriya-mcp —
 * without installing `@actual-app/api` or setting up a real budget. The governance behaviour is
 * identical; only the data store is fake. For the real thing, unset ACTUAL_FAKE and `init()`
 * against a live budget.
 */

import type { Account, ActualApi, Category, Transaction } from "./actual-api.js";

export function fakeActual(): ActualApi {
  const accounts: Account[] = [{ id: "acct-checking", name: "Checking" }];
  const categories: Category[] = [
    { id: "cat-groceries", name: "Groceries", group_id: "grp-everyday" },
  ];
  const transactions: Transaction[] = [
    { id: "txn-1", account: "acct-checking", date: "2026-06-01", amount: -4231, payee_name: "Whole Foods", category: null },
    { id: "txn-2", account: "acct-checking", date: "2026-06-02", amount: -1899, payee_name: "Shell", category: null },
  ];

  return {
    async init() {},
    async downloadBudget() {},
    async sync() {},
    async shutdown() {},
    async getAccounts() {
      return accounts;
    },
    async getTransactions(accountId) {
      return transactions.filter((t) => t.account === accountId);
    },
    async addTransactions(accountId, txns) {
      const ids = txns.map((t, i) => t.id ?? `txn-new-${i}`);
      txns.forEach((t, i) => transactions.push({ id: ids[i]!, account: accountId, date: t.date ?? "", amount: t.amount ?? 0, ...t }));
      return ids;
    },
    async updateTransaction(id, fields) {
      const t = transactions.find((x) => x.id === id);
      if (!t) throw new Error(`no such transaction: ${id}`);
      Object.assign(t, fields);
      return null;
    },
    async deleteTransaction(id) {
      const i = transactions.findIndex((x) => x.id === id);
      if (i < 0) throw new Error(`no such transaction: ${id}`);
      transactions.splice(i, 1);
      return null;
    },
    async getCategories() {
      return categories;
    },
    async createCategory(category) {
      const id = `cat-${category.name.toLowerCase().replace(/\s+/g, "-")}`;
      categories.push({ id, ...category });
      return id;
    },
    async setBudgetAmount() {
      return null;
    },
    async closeAccount(id) {
      const a = accounts.find((x) => x.id === id);
      if (a) a.closed = true;
      return null;
    },
  };
}
