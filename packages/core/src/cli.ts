#!/usr/bin/env node
/**
 * agent-native CLI.
 *
 *   agent-native dump <entry-file>
 *     Imports <entry-file> (which is expected to call `registerAction` as a side effect) and
 *     prints every registered action as an MCP-compatible tool schema. The Rust agent host —
 *     or any external agent — can ingest this to learn an app's affordances without running it.
 *
 *   agent-native wrap <source-file> [--import <specifier>]
 *     Scans <source-file> for exported functions and prints a `wrapAction(...)` registration
 *     module for them (the codemod). Redirect it to a file, fill in the descriptions, and
 *     import it at boot — bolt the action layer onto an existing app without a rewrite.
 */

import { pathToFileURL } from "node:url";
import { basename, resolve } from "node:path";
import { readFile } from "node:fs/promises";
import { getToolSchemas } from "./registry.js";
import { scaffoldWrappers } from "./codemod.js";

const USAGE = `usage:
  agent-native dump <entry-file>
    Import <entry-file> (which registers actions) and print the MCP tool schemas as JSON.
  agent-native wrap <source-file> [--import <specifier>]
    Scaffold wrapAction(...) registrations for the file's exported functions.`;

async function dump(entry: string): Promise<void> {
  await import(pathToFileURL(resolve(entry)).href);
  process.stdout.write(JSON.stringify(getToolSchemas(), null, 2) + "\n");
}

async function wrap(source: string, importOverride?: string): Promise<void> {
  const sourceText = await readFile(resolve(source), "utf8");
  // Default import specifier: the source file's basename with an ESM .js extension.
  const importPath = importOverride ?? `./${basename(source).replace(/\.[cm]?tsx?$/, "")}.js`;
  const out = await scaffoldWrappers(sourceText, { importPath });
  if (!out) {
    console.error(`no exported functions found in ${source}`);
    process.exit(1);
  }
  process.stdout.write(out);
}

async function main(): Promise<void> {
  const [cmd, ...rest] = process.argv.slice(2);

  if (cmd === "dump") {
    const entry = rest[0];
    if (!entry) {
      console.error(USAGE);
      process.exit(1);
    }
    await dump(entry);
    return;
  }

  if (cmd === "wrap") {
    const source = rest[0];
    if (!source) {
      console.error(USAGE);
      process.exit(1);
    }
    const importIdx = rest.indexOf("--import");
    const importOverride = importIdx >= 0 ? rest[importIdx + 1] : undefined;
    await wrap(source, importOverride);
    return;
  }

  console.error(USAGE);
  process.exit(1);
}

main().catch((err) => {
  console.error(err instanceof Error ? err.message : String(err));
  process.exit(1);
});
