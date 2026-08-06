import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import type { CopilotConfig } from "./config.js";
import type { Candidate, Draft, DraftStatus } from "./types.js";
import { nowIso, readJson, writeJson } from "./util.js";

// File-based queue. Drafts live one-per-file so the human can edit a draft's
// body by hand before approving — the whole point is that nothing auto-ships.

function root(config: CopilotConfig): string {
  return resolve(process.cwd(), config.queueDir);
}
function draftsDir(config: CopilotConfig): string {
  return join(root(config), "drafts");
}
function draftPath(config: CopilotConfig, id: string): string {
  return join(draftsDir(config), `${id}.json`);
}
function candidatesPath(config: CopilotConfig): string {
  return join(root(config), "candidates.json");
}
export function auditPath(config: CopilotConfig): string {
  return join(root(config), "audit.log.jsonl");
}

export function saveCandidates(config: CopilotConfig, candidates: Candidate[]): void {
  writeJson(candidatesPath(config), candidates);
}
export function listCandidates(config: CopilotConfig): Candidate[] {
  return readJson<Candidate[]>(candidatesPath(config), []);
}
export function getCandidate(config: CopilotConfig, id: string): Candidate | undefined {
  return listCandidates(config).find((c) => c.id === id);
}

export function saveDraft(config: CopilotConfig, draft: Draft): void {
  writeJson(draftPath(config, draft.id), draft);
}
export function saveDrafts(config: CopilotConfig, drafts: Draft[]): void {
  for (const d of drafts) saveDraft(config, d);
}

export function listDrafts(config: CopilotConfig): Draft[] {
  const dir = draftsDir(config);
  if (!existsSync(dir)) return [];
  const drafts: Draft[] = [];
  for (const file of readdirSync(dir)) {
    if (!file.endsWith(".json")) continue;
    try {
      drafts.push(JSON.parse(readFileSync(join(dir, file), "utf8")) as Draft);
    } catch {
      // Skip a malformed draft file rather than crash the whole queue view.
    }
  }
  return drafts.sort((a, b) => a.createdAt.localeCompare(b.createdAt));
}

export function getDraft(config: CopilotConfig, id: string): Draft | undefined {
  const path = draftPath(config, id);
  if (!existsSync(path)) return undefined;
  try {
    return JSON.parse(readFileSync(path, "utf8")) as Draft;
  } catch {
    return undefined;
  }
}

/** Transition a draft's status, stamping the matching timestamp. */
export function setStatus(
  config: CopilotConfig,
  id: string,
  status: DraftStatus,
): Draft | undefined {
  const draft = getDraft(config, id);
  if (!draft) return undefined;
  draft.status = status;
  const stamp = nowIso();
  if (status === "approved") draft.approvedAt = stamp;
  if (status === "rejected") draft.rejectedAt = stamp;
  if (status === "posted") draft.postedAt = stamp;
  saveDraft(config, draft);
  return draft;
}
