/**
 * `GovernClient` — route an in-process agent framework's tool calls through kriya's action gates
 * and get a signed receipt, **without an MCP hop and without inverting control**.
 *
 * It spawns the runtime's `kriya-govern` binary once (per {@link GovernClient}) and speaks its
 * two-op line protocol over stdio:
 *   - `check`  → policy → approval → budget, returns `allow` | `denied` | `not_approved` |
 *     `budget_exceeded`. Signs nothing on a deny — the decision IS the record (parity with the
 *     in-process governor).
 *   - `record` → signs the Ed25519, hash-chained receipt for a call the framework executed.
 *
 * **The one-Signer law (design law 3).** This file contains NO cryptography, NO policy engine, and
 * NO chain writer. Every trust decision and every signature is made by `kriya-govern`, which reuses
 * the exact `Policy` / `BudgetTracker` / `ApprovalGate` / `Signer` primitives the in-process host and
 * `kriya-mcp` use. This is a thin transport, nothing more.
 *
 * **Honest ceiling.** In-process governance is **cooperative**: a hostile agent process can simply
 * not call this. That is what launch-under containment (kriya-gateway run --, B14) is for. And because
 * the tool runs in the framework's process — not in `kriya-govern` — this lane governs the **action
 * tier** (policy / approval / budget) and signs the receipt; it does NOT see the tool's own outbound
 * network calls, so egress-tier governance is out of scope for it (use the gateway / containment
 * lanes for that).
 */

import { spawn, type ChildProcess } from "node:child_process";
import { createInterface, type Interface } from "node:readline";

/** The decision `check` returns. `allow` is the only one that should proceed to execution. */
export type GovernDecision = "allow" | "denied" | "not_approved" | "budget_exceeded";

export interface CheckResult {
  decision: GovernDecision;
  /** Present for `budget_exceeded` (the human-readable cap reason). */
  reason?: string;
}

/** A signed receipt as `kriya-govern` returns it — the runtime's frozen `SignedReceipt` shape. */
export interface SignedReceipt {
  step_id: string;
  action_id: string;
  params: Record<string, unknown>;
  success: boolean;
  ts_ms: number;
  actor?: { agent: string; user: string } | null;
  prev_hash?: string | null;
  public_key: string;
  signature: string;
}

export interface GovernClientOptions {
  /** Path to the `kriya-govern` binary. Defaults to `"kriya-govern"` (resolved on `PATH`). */
  binaryPath?: string;
  /** Path to a YAML policy file. Omit for the runtime's safe built-in default. */
  policyPath?: string;
  /** Agent identity stamped into every receipt's `actor` (e.g. `"langgraph"`). */
  actor?: string;
  /** Operator identity (defaults, in the binary, to `$USER`). Only used with `actor`. */
  user?: string;
  /**
   * Where the signed receipts are appended. Point it at `~/.kriya/audit/<name>.jsonl` for the
   * Console to tail + re-verify them. Omit for the binary's temp-file default.
   */
  auditLog?: string;
  /**
   * How `require_approval` actions are decided by the binary: `deny` (default — the fail-closed
   * headless posture), `tty`, `gui` (macOS), or `auto` (trusted/testing only).
   */
  approval?: "deny" | "tty" | "gui" | "auto";
}

/** Thrown when `check` returns a non-`allow` decision — surface it as the framework's tool error. */
export class GovernDenied extends Error {
  constructor(
    readonly actionId: string,
    readonly decision: GovernDecision,
    readonly reason?: string,
  ) {
    super(
      `kriya denied "${actionId}": ${decision}${reason ? ` — ${reason}` : ""}`,
    );
    this.name = "GovernDenied";
  }
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
}

/**
 * A live connection to a `kriya-govern` subprocess. Requests are answered strictly in order (the
 * binary reads one line, writes one line), so a FIFO queue correlates each response to its request
 * even under concurrent calls. One client = one govern session, so the budget cap spans the run.
 */
export class GovernClient {
  readonly #child: ChildProcess;
  readonly #rl: Interface;
  readonly #pending: Pending[] = [];
  #closed = false;

  constructor(options: GovernClientOptions = {}) {
    const args: string[] = [];
    if (options.policyPath) args.push("--policy", options.policyPath);
    if (options.approval) args.push("--approval", options.approval);
    if (options.actor) args.push("--actor", options.actor);
    if (options.user) args.push("--user", options.user);
    if (options.auditLog) args.push("--audit-log", options.auditLog);

    this.#child = spawn(options.binaryPath ?? "kriya-govern", args, {
      stdio: ["pipe", "pipe", "inherit"], // stderr inherited so the banner + governance log show
    });
    if (!this.#child.stdin || !this.#child.stdout) {
      throw new Error("kriya-govern did not expose stdio pipes");
    }
    this.#rl = createInterface({ input: this.#child.stdout });
    this.#rl.on("line", (line) => this.#onLine(line));
    this.#child.on("exit", (code) => this.#failAll(`kriya-govern exited (code ${code})`));
    this.#child.on("error", (err) => this.#failAll(`kriya-govern failed to start: ${err.message}`));
  }

  #onLine(line: string): void {
    const trimmed = line.trim();
    if (!trimmed) return;
    const waiter = this.#pending.shift();
    if (!waiter) return; // an unexpected extra line — nothing is waiting for it
    let parsed: unknown;
    try {
      parsed = JSON.parse(trimmed);
    } catch (e) {
      waiter.reject(new Error(`kriya-govern wrote invalid JSON: ${(e as Error).message}`));
      return;
    }
    const obj = parsed as { op?: string; error?: string };
    if (obj.op === "error") {
      waiter.reject(new Error(`kriya-govern: ${obj.error ?? "unknown error"}`));
      return;
    }
    waiter.resolve(parsed);
  }

  #failAll(reason: string): void {
    this.#closed = true;
    while (this.#pending.length) this.#pending.shift()!.reject(new Error(reason));
  }

  #send<T>(request: object): Promise<T> {
    if (this.#closed) return Promise.reject(new Error("GovernClient is closed"));
    return new Promise<T>((resolve, reject) => {
      this.#pending.push({ resolve: resolve as (v: unknown) => void, reject });
      this.#child.stdin!.write(`${JSON.stringify(request)}\n`);
    });
  }

  /** Ask whether `actionId` may run (policy → approval → budget). Signs nothing. */
  async check(actionId: string, params: Record<string, unknown>): Promise<CheckResult> {
    const res = await this.#send<{ decision: GovernDecision; reason?: string }>({
      op: "check",
      action_id: actionId,
      params,
    });
    return { decision: res.decision, reason: res.reason };
  }

  /** Sign the receipt for a call the framework executed (success or failure). */
  async record(
    actionId: string,
    params: Record<string, unknown>,
    success: boolean,
  ): Promise<SignedReceipt> {
    const res = await this.#send<{ receipt: SignedReceipt }>({
      op: "record",
      action_id: actionId,
      params,
      success,
    });
    return res.receipt;
  }

  /** Close stdin (signals shutdown) and terminate the subprocess. Idempotent. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#rl.close();
    this.#child.stdin?.end();
    this.#child.kill();
  }
}

/** Optional hooks for observing governed calls (e.g. the framework's own logging/telemetry). */
export interface GovernHooks {
  /** Called with the signed receipt after a governed call is recorded. */
  onReceipt?: (receipt: SignedReceipt) => void;
  /** Called when `check` denies a call (before {@link GovernDenied} is thrown). */
  onDenied?: (actionId: string, result: CheckResult) => void;
}

/**
 * Wrap any async tool function into a **governed** one: `check` first, run the real tool only on
 * `allow`, then `record` the outcome (success OR failure — a thrown tool error still signs a
 * `success:false` receipt, exactly like the in-process governor). A non-`allow` decision throws
 * {@link GovernDenied} so the framework surfaces it as that tool's native error and the model can
 * adapt. Framework-agnostic — the adapters (LangGraph, OpenAI Agents, Claude Agent SDK) are thin
 * shims over this.
 */
export function govern<P extends Record<string, unknown>, R>(
  client: GovernClient,
  actionId: string,
  fn: (params: P) => R | Promise<R>,
  hooks: GovernHooks = {},
): (params: P) => Promise<R> {
  return async (params: P): Promise<R> => {
    const gate = await client.check(actionId, params);
    if (gate.decision !== "allow") {
      hooks.onDenied?.(actionId, gate);
      throw new GovernDenied(actionId, gate.decision, gate.reason);
    }
    let success = false;
    let result: R | undefined;
    let thrown: unknown;
    try {
      result = await fn(params);
      success = true;
    } catch (e) {
      thrown = e;
    }
    // Record success OR failure — the receipt is the record of what was attempted, not only of
    // what worked (parity with the in-process governor, which signs failed calls too).
    const receipt = await client.record(actionId, params, success);
    hooks.onReceipt?.(receipt);
    if (!success) throw thrown;
    return result as R;
  };
}
