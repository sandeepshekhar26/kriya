/**
 * @agent-native/sidecar — host the verb agent runtime from Electron or plain Node.
 *
 * Spawns the `verb-host` binary (the Rust agent host, built from
 * `crates/agent-native-host`) and speaks its newline-delimited JSON protocol over stdio. The
 * agent loop, the inference backend, and the whole safety layer (policy, approval, budget,
 * signed audit) run **inside that separate process** — which the renderer can't tamper with —
 * while your Node/Electron main process just executes the typed actions the host asks for.
 *
 * This is the cross-shell half of the framework: the same registered actions that a Tauri app
 * exposes can now be driven by an agent hosted from any Node-based shell.
 */

import { spawn } from "node:child_process";
import type { ChildProcess } from "node:child_process";
import type { Readable, Writable } from "node:stream";

import type {
  AgentActionRequest,
  AgentActionResult,
  AgentApprovalRequest,
  AgentApprovalResponse,
  AgentAwaitStep,
  AgentDone,
  AgentLog,
  AgentStartRequest,
  AgentStepAdvance,
} from "@agent-native/core";

/** The events the host pushes back to the app, and their payload tuples. */
export interface SidecarEvents {
  /** The host wants this action executed; reply with {@link SidecarHost.sendActionResult}. */
  action: [AgentActionRequest];
  /** A guarded action needs a human; reply with {@link SidecarHost.sendApproval}. */
  approval: [AgentApprovalRequest];
  /** Step-mode pause; reply with {@link SidecarHost.sendStepAdvance}. */
  awaitStep: [AgentAwaitStep];
  /** The run finished. */
  done: [AgentDone];
  /** Inspector/telemetry line. */
  log: [AgentLog];
  /** A stdout line that wasn't valid JSON (carries the raw line). */
  parseError: [string];
  /** The sidecar process exited (carries the exit code, or null if killed by signal). */
  exit: [number | null];
}

type Listener<A extends unknown[]> = (...args: A) => void;

export interface SidecarStreams {
  /** Write inbound messages here (the sidecar's stdin). */
  stdin: Writable;
  /** Read outbound messages from here (the sidecar's stdout). */
  stdout: Readable;
  /** The owning child process, if any — closed by {@link SidecarHost.close}. */
  child?: ChildProcess;
}

export interface SpawnOptions {
  /** Path to the `verb-host` binary. */
  binaryPath: string;
  /** Extra CLI args, e.g. `["--policy", "policy.yaml", "--script", "demo.json"]`. */
  args?: string[];
  /** Environment for the child (e.g. `{ AGENT_BACKEND: "claude-cli" }`). */
  env?: NodeJS.ProcessEnv;
}

/**
 * A live connection to a `verb-host` sidecar. Construct it from raw streams (handy for tests)
 * or, more usually, with {@link SidecarHost.spawn}. Subscribe with {@link on} and push
 * decisions back with the `send*` methods.
 */
export class SidecarHost {
  readonly #stdin: Writable;
  readonly #child: ChildProcess | undefined;
  #buffer = "";

  // Fully-keyed listener registry — typed, no `any`, every event present.
  readonly #listeners: { [K in keyof SidecarEvents]: Set<Listener<SidecarEvents[K]>> } = {
    action: new Set(),
    approval: new Set(),
    awaitStep: new Set(),
    done: new Set(),
    log: new Set(),
    parseError: new Set(),
    exit: new Set(),
  };

  constructor(streams: SidecarStreams) {
    this.#stdin = streams.stdin;
    this.#child = streams.child;
    streams.stdout.on("data", (chunk: Buffer | string) => this.#onData(chunk));
    this.#child?.on("exit", (code) => this.#emit("exit", code));
  }

  /** Spawn the `verb-host` binary and connect to it. stderr is inherited so the host's
   * banner and governance log show up in your console. */
  static spawn(options: SpawnOptions): SidecarHost {
    const child = spawn(options.binaryPath, options.args ?? [], {
      stdio: ["pipe", "pipe", "inherit"],
      env: options.env ?? process.env,
    });
    if (!child.stdin || !child.stdout) {
      throw new Error("verb-host did not expose stdio pipes");
    }
    return new SidecarHost({ stdin: child.stdin, stdout: child.stdout, child });
  }

  on<K extends keyof SidecarEvents>(event: K, listener: Listener<SidecarEvents[K]>): this {
    this.#listeners[event].add(listener);
    return this;
  }

  off<K extends keyof SidecarEvents>(event: K, listener: Listener<SidecarEvents[K]>): this {
    this.#listeners[event].delete(listener);
    return this;
  }

  /** Begin an autonomous run. */
  start(request: AgentStartRequest): void {
    this.#send("start", request);
  }

  /** Report the result of an action the host asked you to run. */
  sendActionResult(result: AgentActionResult): void {
    this.#send("action_result", result);
  }

  /** Answer a pending approval request. */
  sendApproval(response: AgentApprovalResponse): void {
    this.#send("approval_response", response);
  }

  /** Advance or stop a step-mode run. */
  sendStepAdvance(advance: AgentStepAdvance): void {
    this.#send("step_advance", advance);
  }

  /** Close stdin (signals shutdown) and kill the child if we own it. */
  close(): void {
    this.#stdin.end();
    this.#child?.kill();
  }

  #send(type: string, data: unknown): void {
    this.#stdin.write(`${JSON.stringify({ type, data })}\n`);
  }

  #onData(chunk: Buffer | string): void {
    this.#buffer += chunk.toString();
    // The host emits one JSON object per line; a chunk may hold many lines or a partial one.
    let newline = this.#buffer.indexOf("\n");
    while (newline >= 0) {
      const line = this.#buffer.slice(0, newline).trim();
      this.#buffer = this.#buffer.slice(newline + 1);
      if (line) this.#dispatch(line);
      newline = this.#buffer.indexOf("\n");
    }
  }

  #dispatch(line: string): void {
    let message: { type?: string; data?: unknown };
    try {
      message = JSON.parse(line) as { type?: string; data?: unknown };
    } catch {
      this.#emit("parseError", line);
      return;
    }
    switch (message.type) {
      case "action":
        this.#emit("action", message.data as AgentActionRequest);
        break;
      case "approval":
        this.#emit("approval", message.data as AgentApprovalRequest);
        break;
      case "await_step":
        this.#emit("awaitStep", message.data as AgentAwaitStep);
        break;
      case "done":
        this.#emit("done", message.data as AgentDone);
        break;
      case "log":
        this.#emit("log", message.data as AgentLog);
        break;
      default:
        this.#emit("parseError", line);
    }
  }

  #emit<K extends keyof SidecarEvents>(event: K, ...args: SidecarEvents[K]): void {
    for (const listener of this.#listeners[event]) listener(...args);
  }
}

/** How {@link runTask} executes one action and how it answers approvals. */
export interface RunTaskHandlers {
  /** Execute the action against your app and return the result + refreshed state. */
  dispatch: (request: AgentActionRequest) => AgentActionResult | Promise<AgentActionResult>;
  /** Decide a guarded action. Defaults to denying (the safe choice). */
  approve?: (request: AgentApprovalRequest) => boolean | Promise<boolean>;
  /** Optional log sink (host telemetry + governance decisions). */
  onLog?: (entry: AgentLog) => void;
}

/**
 * Drive a single run to completion: send `start`, execute each requested action via
 * `handlers.dispatch`, answer approvals via `handlers.approve`, and resolve with the
 * {@link AgentDone} summary. This is the loop most Electron/Node apps want; for finer control,
 * use {@link SidecarHost} directly.
 */
export function runTask(
  host: SidecarHost,
  request: AgentStartRequest,
  handlers: RunTaskHandlers,
): Promise<AgentDone> {
  return new Promise<AgentDone>((resolve) => {
    host.on("action", (req) => {
      void Promise.resolve(handlers.dispatch(req)).then((result) => host.sendActionResult(result));
    });
    host.on("approval", (req) => {
      const decide = handlers.approve ?? (() => false);
      void Promise.resolve(decide(req)).then((approved) =>
        host.sendApproval({ stepId: req.stepId, approved }),
      );
    });
    if (handlers.onLog) host.on("log", handlers.onLog);
    host.on("done", (done) => resolve(done));
    host.start(request);
  });
}
