/**
 * The slice of Actual Budget's in-process API (`@actual-app/api`) this bolt-on uses.
 *
 * Actual has **no HTTP API** — it ships an npm package you call in-process, backed by a local
 * SQLite budget loaded on `init()`. That's exactly why verb fits: there's no endpoint for a
 * cloud agent to hit; the only way to drive it is an in-process action layer, governed
 * on-device. We type just the functions we wrap (signatures from actualbudget.org/docs/api)
 * and load the real module at runtime, so this example builds without pulling Actual's native
 * deps into the monorepo. `npm install @actual-app/api` to run it for real.
 */

export interface Transaction {
  id: string;
  account: string;
  date: string;
  amount: number;
  payee_name?: string;
  category?: string | null;
  notes?: string;
}

export interface Account {
  id: string;
  name: string;
  closed?: boolean;
}

export interface Category {
  id: string;
  name: string;
  group_id: string;
}

export interface InitConfig {
  dataDir?: string;
  serverURL?: string;
  password?: string;
}

/** The functions we wrap. A structural subset of the real `@actual-app/api`. */
export interface ActualApi {
  init(config?: InitConfig): Promise<void>;
  shutdown(): Promise<void>;
  getAccounts(): Promise<Account[]>;
  getTransactions(accountId: string, startDate: string, endDate: string): Promise<Transaction[]>;
  addTransactions(accountId: string, transactions: Partial<Transaction>[]): Promise<string[]>;
  updateTransaction(id: string, fields: Partial<Transaction>): Promise<null>;
  deleteTransaction(id: string): Promise<null>;
  getCategories(): Promise<Category[]>;
  createCategory(category: { name: string; group_id: string }): Promise<string>;
  setBudgetAmount(month: string, categoryId: string, amount: number): Promise<null>;
  closeAccount(
    id: string,
    transferAccountId?: string,
    transferCategoryId?: string,
  ): Promise<null>;
}

/** Load the real `@actual-app/api` at runtime (structurally an {@link ActualApi}). */
export async function loadActual(): Promise<ActualApi> {
  const mod = (await import("@actual-app/api")) as unknown as { default?: ActualApi } & ActualApi;
  return mod.default ?? mod;
}
