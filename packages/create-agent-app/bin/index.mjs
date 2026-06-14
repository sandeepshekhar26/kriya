#!/usr/bin/env node
// Scaffolder for agent-native desktop apps.
// Zero runtime dependencies on purpose — Node 18+ stdlib only.

import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const TEMPLATE_DIR = resolve(__dirname, "..", "template");

function usage() {
  console.log(`Usage: create-agent-app <project-name>

Scaffolds a Tauri 2 + React + Rust agent-host app into ./<project-name>.

After it finishes:
  cd <project-name>
  npm install
  npm run tauri dev`);
}

function die(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

// npm package names: lowercase, no spaces, no slashes, no leading dot/underscore.
const NAME_RE = /^[a-z][a-z0-9-]*$/;

function toSnake(name) {
  return name.replace(/-/g, "_");
}

function toTitle(name) {
  return name
    .split("-")
    .filter(Boolean)
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

const SUBS = (name) => ({
  "{{NAME}}": name,
  "{{NAME_SNAKE}}": toSnake(name),
  "{{TITLE}}": toTitle(name),
  "{{IDENTIFIER}}": `com.example.${toSnake(name)}`,
});

const TEXT_EXT = new Set([
  ".json", ".js", ".mjs", ".cjs", ".ts", ".tsx", ".html", ".css",
  ".md", ".toml", ".yaml", ".yml", ".rs", ".lock",
]);

function isTextPath(p) {
  const i = p.lastIndexOf(".");
  return i >= 0 && TEXT_EXT.has(p.slice(i));
}

function substitute(content, subs) {
  let out = content;
  for (const [k, v] of Object.entries(subs)) {
    out = out.split(k).join(v);
  }
  return out;
}

function walkAndSubstitute(dir, subs) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      walkAndSubstitute(full, subs);
      continue;
    }
    if (entry.isFile() && isTextPath(entry.name)) {
      const before = readFileSync(full, "utf8");
      const after = substitute(before, subs);
      if (after !== before) writeFileSync(full, after);
    }
  }
}

function main() {
  const [, , ...args] = process.argv;
  if (args.length === 0 || args[0] === "-h" || args[0] === "--help") {
    usage();
    process.exit(args.length === 0 ? 1 : 0);
  }
  const name = args[0];
  if (!NAME_RE.test(name)) {
    die(`"${name}" is not a valid project name. Use lowercase letters, digits, and dashes (e.g. my-app).`);
  }

  const target = resolve(process.cwd(), name);
  if (existsSync(target)) {
    const contents = statSync(target).isDirectory() ? readdirSync(target) : [];
    if (contents.length > 0) {
      die(`directory ${name}/ already exists and is not empty.`);
    }
  } else {
    mkdirSync(target, { recursive: true });
  }

  if (!existsSync(TEMPLATE_DIR)) {
    die(`template not found at ${TEMPLATE_DIR}. Reinstall create-agent-app.`);
  }

  // Copy the whole template tree.
  cpSync(TEMPLATE_DIR, target, { recursive: true });

  // Substitute placeholders in text files.
  walkAndSubstitute(target, SUBS(name));

  // npm strips `.gitignore` from published tarballs; the template ships it as
  // `gitignore` and we rename on copy.
  const gi = join(target, "gitignore");
  if (existsSync(gi)) renameSync(gi, join(target, ".gitignore"));

  console.log(`
✓ Created ${name}/ from agent-native template.

Next:
  cd ${name}
  npm install
  npm run tauri dev    # first run compiles the Rust host (~few minutes)

Inside the app: click "+1" a few times, then "Run agent" to watch the
agent reach the target by calling the same typed actions a human would.

Backends (set AGENT_BACKEND):
  deterministic     (default, no setup)
  claude-cli        (uses your local 'claude' CLI)
  ollama            (OLLAMA_MODEL=llama3 ...)
  anthropic         (ANTHROPIC_API_KEY=...)
`);
}

main();
