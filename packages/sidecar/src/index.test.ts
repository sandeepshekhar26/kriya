import { PassThrough } from "node:stream";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, it, expect } from "vitest";
import type { AgentActionRequest } from "@kriya/core";

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
});

// Integration test against the real Rust binary. Opt-in: set VERB_HOST_BIN to the built
// `kriya-host` path. Skipped in CI (which doesn't build the Rust binary) so the JS suite
// stays self-contained.
const bin = process.env.VERB_HOST_BIN;
describe.skipIf(!bin)("kriya-host integration", () => {
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
});
