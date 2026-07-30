import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname } from "node:path";

/** Short, stable id for a string (URL, body, …). */
export function shortHash(input: string): string {
  return createHash("sha256").update(input).digest("hex").slice(0, 10);
}

export function fullHash(input: string): string {
  return createHash("sha256").update(input).digest("hex");
}

export function nowIso(): string {
  return new Date().toISOString();
}

/** True when we should avoid the network and produce deterministic output. */
export function offline(): boolean {
  return process.env.CONTENT_COPILOT_FAKE === "1";
}

export function readJson<T>(path: string, fallback: T): T {
  if (!existsSync(path)) return fallback;
  try {
    return JSON.parse(readFileSync(path, "utf8")) as T;
  } catch {
    return fallback;
  }
}

export function writeJson(path: string, value: unknown): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(value, null, 2) + "\n", "utf8");
}

export function appendLine(path: string, line: string): void {
  mkdirSync(dirname(path), { recursive: true });
  // Append manually so we don't pull in extra fs flags juggling.
  const prev = existsSync(path) ? readFileSync(path, "utf8") : "";
  writeFileSync(path, prev + line + "\n", "utf8");
}

/** Case-insensitive "does this text mention any of these topics" → matched list. */
export function matchTopics(text: string, topics: string[]): string[] {
  const haystack = text.toLowerCase();
  return topics.filter((t) => haystack.includes(t.toLowerCase()));
}
