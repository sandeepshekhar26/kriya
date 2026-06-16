import { PassThrough } from "node:stream";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, it, expect } from "vitest";
import type { AgentActionRequest } from "kriya-core";

import { SidecarHost, runTask } from "./index.js";

/** A SidecarHost wired to in-memory streams, plus a capture of everything written to stdin. */
function harness() {
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  const written: string[] = [];
  stdin.on("data", (d: Buffer) => written.push(d.toString()));
  const host = new SidecarHost({ stdin, stdout });
  // Parse the inbound (app→host) lines the host wrote, newest tolerant of partials.
  const inbound = () =>
    written
      .join("")
      .split("\n")
      .filter((l) => l.trim())
      .map((l) => JSON.parse(l) as { type: string; data: unknown });
  return { host, stdout, inbound };
}

describe("SidecarHost framing", () => {
  it("parses a complete outbound line into a typed event", () => {
    const { host, stdout } = harness();
    const seen: AgentActionRequest[] = [];
    host.on("action", (req) => seen.push(req));

    stdout.write(
      JSON.stringify({
        type: "action",
        data: { stepId: "s1", actionId: "create_note", params: { title: "hi" }, reasoning: "r" },
      }) + "\n",
    );

    expect(seen).toHaveLength(1);
    expect(seen[0]?.actionId).toBe("create_note");
  });

  it("reassembles a message split across chunks", () => {
    const { host, stdout } = harness();
    let got: string | undefined;
    host.on("done", (d) => (got = d.summary));

    const line = JSON.stringify({ type: "done", data: { summary: "all done", steps: 2 } }) + "\n";
    stdout.write(line.slice(0, 10));
    stdout.write(line.slice(10));

    expect(got).toBe("all done");
  });

  it("emits parseError for a non-JSON line and an unknown type", () => {
    const { host, stdout } = harness();
    const errors: string[] = [];
    host.on("parseError", (l) => errors.push(l));

    stdout.write("this is not json\n");
    stdout.write(JSON.stringify({ type: "mystery", data: {} }) + "\n");

    expect(errors).toHaveLength(2);
  });

  it("serializes start/approval as tagged inbound lines", () => {
    const { host, inbound } = harness();
    host.start({ goal: "g", state: {}, tools: [] });
    host.sendApproval({ stepId: "s1", approved: true });

    const msgs = inbound();
    expect(msgs[0]).toMatchObject({ type: "start", data: { goal: "g" } });
    expect(msgs[1]).toMatchObject({ type: "approval_response", data: { stepId: "s1", approved: true } });
  });
});

describe("runTask", () => {
  it("dispatches actions, answers approvals, and resolves on done", async () => {
    const { host, stdout, inbound } = harness();

    // Simulate the host's side: when it receives an action_result, push the next line.
    let step = 0;
    host.on("action", () => {
      // After we reply to the action, the host would emit done.
      queueMicrotask(() => {
        if (step === 0) {
          step = 1;
          stdout.write(JSON.stringify({ type: "done", data: { summary: "ok", steps: 1 } }) + "\n");
        }
      });
    });

    const promise = runTask(
      host,
      { goal: "make a note", state: { notes: [] }, tools: [] },
      {
        dispatch: (req) => ({ stepId: req.stepId, success: true, state: { notes: ["n"] } }),
      },
    );

    // The host pushes one action; runTask should dispatch + reply, then we send done.
    stdout.write(
      JSON.stringify({
        type: "action",
        data: { stepId: "s1", actionId: "create_note", params: {}, reasoning: "r" },
      }) + "\n",
    );

    const done = await promise;
    expect(done).toEqual({ summary: "ok", steps: 1 });
    // start + action_result were written inbound.
    const types = inbound().map((m) => m.type);
    expect(types).toContain("start");
    expect(types).toContain("action_result");
  });

  it("answers a step-mode pause via onStep (stop)", async () => {
    const { host, stdout, inbound } = harness();
    const promise = runTask(
      host,
      { goal: "g", state: {}, tools: [], stepMode: true },
      {
        dispatch: (req) => ({ stepId: req.stepId, success: true, state: {} }),
        onStep: () => false, // a human/UI says "stop" at the first pause
      },
    );

    // Host pauses before step 1; the helper must answer (not hang).
    stdout.write(JSON.stringify({ type: "await_step", data: { gateId: "g1", stepNumber: 1 } }) + "\n");
    await new Promise((r) => setTimeout(r, 0));
    expect(inbound().find((m) => m.type === "step_advance")).toMatchObject({
      type: "step_advance",
      data: { gateId: "g1", proceed: false },
    });

    stdout.write(JSON.stringify({ type: "done", data: { summary: "stopped", steps: 0 } }) + "\n");
    expect((await promise).summary).toBe("stopped");
  });

  it("auto-advances a step-mode pause when no onStep is given (never hangs)", async () => {
    const { host, stdout, inbound } = harness();
    const promise = runTask(
      host,
      { goal: "g", state: {}, tools: [], stepMode: true },
      { dispatch: (req) => ({ stepId: req.stepId, success: true, state: {} }) },
    );

    stdout.write(JSON.stringify({ type: "await_step", data: { gateId: "g2", stepNumber: 1 } }) + "\n");
    await new Promise((r) => setTimeout(r, 0));
    expect(inbound().find((m) => m.type === "step_advance")).toMatchObject({
      type: "step_advance",
      data: { gateId: "g2", proceed: true },
    });

    stdout.write(JSON.stringify({ type: "done", data: { summary: "ok", steps: 1 } }) + "\n");
    expect((await promise).summary).toBe("ok");
  });
});

describe("recentMemory", () => {
  it("resolves with the episodes from the correlated memory reply", async () => {
    const { host, stdout, inbound } = harness();
    const promise = host.recentMemory(5);
    await new Promise((r) => setTimeout(r, 0));

    const request = inbound().find((m) => m.type === "memory_recent") as {
      type: string;
      data: { requestId: string; limit?: number };
    };
    expect(request).toBeTruthy();
    expect(request.data.limit).toBe(5);

    stdout.write(
      JSON.stringify({
        type: "memory",
        data: {
          requestId: request.data.requestId,
          episodes: [
            {
              tsMs: 1,
              actionId: "create_note",
              params: "{}",
              success: true,
              reasoning: "r",
              signature: "sig",
              runId: "run1",
              goal: "g",
            },
          ],
        },
      }) + "\n",
    );

    const episodes = await promise;
    expect(episodes).toHaveLength(1);
    expect(episodes[0]?.actionId).toBe("create_note");
  });

  it("rejects when the host reports an error", async () => {
    const { host, stdout, inbound } = harness();
    const promise = host.recentMemory();
    await new Promise((r) => setTimeout(r, 0));
    const requestId = (
      inbound().find((m) => m.type === "memory_recent") as { data: { requestId: string } }
    ).data.requestId;

    stdout.write(
      JSON.stringify({ type: "memory", data: { requestId, episodes: [], error: "db locked" } }) +
        "\n",
    );
    await expect(promise).rejects.toThrow("db locked");
  });

  it("does not cross replies between concurrent queries", async () => {
    const { host, stdout, inbound } = harness();
    const first = host.recentMemory(1);
    const second = host.recentMemory(2);
    await new Promise((r) => setTimeout(r, 0));

    const requests = inbound().filter((m) => m.type === "memory_recent") as {
      data: { requestId: string; limit?: number };
    }[];
    expect(requests).toHaveLength(2);
    const [r1, r2] = requests;

    // Answer the second one first — correlation must still route it correctly.
    stdout.write(
      JSON.stringify({
        type: "memory",
        data: { requestId: r2!.data.requestId, episodes: [{ actionId: "second" }] },
      }) + "\n",
    );
    stdout.write(
      JSON.stringify({
        type: "memory",
        data: { requestId: r1!.data.requestId, episodes: [{ actionId: "first" }] },
      }) + "\n",
    );

    expect((await first)[0]?.actionId).toBe("first");
    expect((await second)[0]?.actionId).toBe("second");
  });
});

// Integration test against the real Rust binary. Opt-in: set KRIYA_HOST_BIN to the built
// `kriya-host` path. Skipped in CI (which doesn't build the Rust binary) so the JS suite
// stays self-contained.
const bin = process.env.KRIYA_HOST_BIN;
describe.skipIf(!bin)("kriya-host integration (real binary)", () => {
  it("drives a scripted run end to end", async () => {
    const dir = mkdtempSync(join(tmpdir(), "kriya-sidecar-"));
    const script = join(dir, "script.json");
    writeFileSync(
      script,
      JSON.stringify([
        { action: "create_note", params: { title: "From Node" }, reasoning: "seed" },
        { done: true, summary: "done" },
      ]),
    );

    // Confirm the binary at least runs before spawning the streaming host.
    expect(spawnSync(bin!, ["--help"]).status).not.toBeNull();

    const host = SidecarHost.spawn({ binaryPath: bin!, args: ["--script", script] });
    const notes: unknown[] = [];
    try {
      const done = await runTask(
        host,
        { goal: "make a note", state: { notes: [] }, tools: [] },
        {
          dispatch: (req) => {
            notes.push(req.params);
            return { stepId: req.stepId, success: true, state: { notes } };
          },
        },
      );
      expect(done.steps).toBe(1);
      expect(notes).toHaveLength(1);
    } finally {
      host.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("holds a guarded action for approval, then surfaces both actions via recentMemory", async () => {
    const dir = mkdtempSync(join(tmpdir(), "kriya-sidecar-"));
    const script = join(dir, "script.json");
    // create_* runs directly; delete_* needs approval under the default policy.
    writeFileSync(
      script,
      JSON.stringify([
        { action: "create_note", params: { title: "keep" }, reasoning: "seed" },
        { action: "delete_note", params: { id: 1 }, reasoning: "cleanup" },
        { done: true, summary: "done" },
      ]),
    );

    const host = SidecarHost.spawn({ binaryPath: bin!, args: ["--script", script] });
    // Unique goal so memory recall can isolate this run's episodes from the shared store.
    const goal = `p05-itest-${Date.now()}`;
    const approvalsAsked: string[] = [];
    try {
      const done = await runTask(
        host,
        { goal, state: { notes: [] }, tools: [] },
        {
          dispatch: (req) => ({ stepId: req.stepId, success: true, state: { notes: [] } }),
          approve: (req) => {
            approvalsAsked.push(req.actionId);
            return true; // grant the held delete
          },
        },
      );

      // The delete was held for a human; create was not. Both ran (delete only after approval).
      expect(approvalsAsked).toEqual(["delete_note"]);
      expect(done.steps).toBe(2);

      // Memory recall over the protocol — the Electron-parity feature. Filter by our unique
      // goal since the durable store accumulates across every run.
      const mine = (await host.recentMemory(50))
        .filter((e) => e.goal === goal)
        .map((e) => e.actionId);
      expect(mine).toContain("create_note");
      expect(mine).toContain("delete_note");
    } finally {
      host.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
