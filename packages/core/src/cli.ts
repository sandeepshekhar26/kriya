#!/usr/bin/env node
/**
 * agent-native CLI.
 *
 *   agent-native dump <entry-file>
 *
 * Imports <entry-file> (which is expected to call `registerAction` as a side effect) and
 * prints every registered action as an MCP-compatible tool schema. The Rust agent host —
 * or any external agent — can ingest this to learn an app's affordances without running it.
 */

import { pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { getToolSchemas } from "./registry.js";

const USAGE = `usage: agent-native dump <entry-file>
  Imports <entry-file> (which registers actions) and prints the MCP tool schemas as JSON.`;

async function main(): Promise<void> {
  const [cmd, entry] = process.argv.slice(2);
  if (cmd !== "dump" || !entry) {
    console.error(USAGE);
    process.exit(1);
  }
  await import(pathToFileURL(resolve(entry)).href);
  process.stdout.write(JSON.stringify(getToolSchemas(), null, 2) + "\n");
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
