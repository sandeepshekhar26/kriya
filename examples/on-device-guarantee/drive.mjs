#!/usr/bin/env node
// Minimal raw-NDJSON driver for the kriya-host sidecar (R3). Speaks the documented
// {"type","data"} protocol directly — no kriya-sidecar package or dist build needed.
//
//   node drive.mjs <kriya-host-binary> [host args...]
//
// It sends one `start`, auto-runs whatever actions the host *clears* (echoing success), and
// resolves on `done`. Used by demo.sh to drive a sealed run two ways: with an on-device
// backend (proceeds + attests) and with an egressing one (refused before any action).
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const [, , binary, ...hostArgs] = process.argv;
if (!binary) {
  console.error("usage: node drive.mjs <kriya-host-binary> [host args...]");
  process.exit(2);
}

const child = spawn(binary, hostArgs, { stdio: ["pipe", "pipe", "inherit"], env: process.env });
const rl = createInterface({ input: child.stdout });
const send = (obj) => child.stdin.write(JSON.stringify(obj) + "\n");

rl.on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  switch (msg.type) {
    case "log":
      // Surface the host's governance logs — including the on-device decision.
      console.error(`   [host:${msg.data.level}] ${msg.data.message}`);
      break;
    case "action":
      // The action already cleared every gate; run it (here: echo success, unchanged state).
      // The result protocol uses camelCase `stepId` (see crates/kriya/src/protocol.rs).
      send({
        type: "action_result",
        data: { stepId: msg.data.stepId, success: true, data: {}, error: null, state: {} },
      });
      break;
    case "approval":
      send({ type: "approval_response", data: { stepId: msg.data.stepId, approved: false } });
      break;
    case "done":
      console.log(`   done: ${msg.data.summary}`);
      child.stdin.end(); // EOF → the sidecar exits
      break;
  }
});

child.on("exit", (code) => process.exit(code ?? 0));

// Kick off the run.
send({ type: "start", data: { goal: process.env.DEMO_GOAL ?? "sealed run", state: {}, tools: [] } });
