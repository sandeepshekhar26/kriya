#!/usr/bin/env node
// Minimal per-call action handler for the R8 actor-identity demo. kriya-mcp's
// ProcessExecutor spawns this once per cleared action, writes one line of
// {"action","params"} to our stdin, closes stdin, and reads our stdout to EOF. So we
// read the one request line, write one {"success","data"} line *synchronously* (fd 1),
// and exit — letting the parent's read-to-EOF return. The point of the demo is the
// governance + attribution around the call, not the business logic, so we just echo ok.
import { createInterface } from "node:readline";
import { writeSync } from "node:fs";

const rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  let action = "?";
  try {
    action = JSON.parse(line).action ?? "?";
  } catch {
    /* fall through to a generic ok */
  }
  writeSync(1, JSON.stringify({ success: true, data: { ran: action } }) + "\n");
  rl.close();
  process.exit(0);
});
