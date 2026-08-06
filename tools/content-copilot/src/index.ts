#!/usr/bin/env node
import { existsSync, readFileSync } from "node:fs";
import { loadConfig, type CopilotConfig } from "./config.js";
import { draftAll, replyRequest } from "./draft.js";
import { PLATFORMS } from "./platforms.js";
import { postDraft } from "./post.js";
import {
  auditPath,
  getCandidate,
  getDraft,
  listCandidates,
  listDrafts,
  saveCandidates,
  saveDrafts,
  setStatus,
} from "./queue.js";
import { research } from "./research.js";
import { ALL_PLATFORMS, type Draft, type Platform } from "./types.js";

// ---- tiny arg parser (no deps) -------------------------------------------

interface Args {
  positionals: string[];
  flags: Map<string, string | true>;
}
function parseArgs(argv: string[]): Args {
  const positionals: string[] = [];
  const flags = new Map<string, string | true>();
  for (let i = 0; i < argv.length; i++) {
    const tok = argv[i];
    if (tok === undefined) continue;
    if (tok.startsWith("--")) {
      const name = tok.slice(2);
      const next = argv[i + 1];
      if (next !== undefined && !next.startsWith("--")) {
        flags.set(name, next);
        i++;
      } else {
        flags.set(name, true);
      }
    } else {
      positionals.push(tok);
    }
  }
  return { positionals, flags };
}

function pickPlatforms(args: Args, config: CopilotConfig): Platform[] {
  const raw = args.flags.get("platform");
  if (typeof raw !== "string") return config.platforms;
  const wanted = raw
    .split(",")
    .map((s) => s.trim())
    .filter((s): s is Platform => (ALL_PLATFORMS as readonly string[]).includes(s));
  return wanted.length > 0 ? wanted : config.platforms;
}

// ---- output helpers -------------------------------------------------------

const out = (s = "") => process.stdout.write(s + "\n");
const statusIcon: Record<Draft["status"], string> = {
  draft: "✎ draft",
  approved: "✓ approved",
  rejected: "✗ rejected",
  posted: "● posted",
};

function printDraft(d: Draft, full: boolean): void {
  out(`  ${statusIcon[d.status]}  [${PLATFORMS[d.platform].label}]  ${d.id}  (${d.generator})`);
  out(`    ${d.kind === "reply" ? "reply to" : "about"}: ${d.subject}`);
  if (d.sourceUrl) out(`    source: ${d.sourceUrl}`);
  if (full) {
    out("    ┌─");
    for (const line of d.body.split("\n")) out(`    │ ${line}`);
    out("    └─");
  }
}

// ---- commands -------------------------------------------------------------

async function cmdResearch(args: Args, config: CopilotConfig): Promise<void> {
  out(`Scanning sources for: ${config.topics.join(", ")}`);
  const candidates = await research(config);
  saveCandidates(config, candidates);
  if (candidates.length === 0) {
    out("No relevant discussions found. (Try CONTENT_COPILOT_FAKE=1 for a sample run.)");
    return;
  }
  out(`\nFound ${candidates.length} candidate(s), saved to the queue:\n`);
  for (const c of candidates) {
    out(`  [${c.score}] ${c.id}  ${c.source}`);
    out(`        ${c.title}`);
    out(`        ${c.url}`);
    out(`        matched: ${c.matchedTopics.join(", ")}`);
  }
  out(`\nDraft a reply to one with:  content-copilot draft --reply <candidateId>`);
}

function cmdCandidates(config: CopilotConfig): void {
  const candidates = listCandidates(config);
  if (candidates.length === 0) return void out("No candidates yet. Run: content-copilot research");
  for (const c of candidates) {
    out(`  [${c.score}] ${c.id}  ${c.source}  —  ${c.title}`);
  }
}

async function cmdDraft(args: Args, config: CopilotConfig): Promise<void> {
  const platforms = pickPlatforms(args, config);
  const reply = args.flags.get("reply");

  let drafts: Draft[];
  if (typeof reply === "string") {
    // Reply to a surfaced candidate (by id) or a raw URL.
    const candidate = getCandidate(config, reply);
    if (candidate) {
      drafts = await draftAll(replyRequest(config, platforms, candidate));
    } else {
      drafts = await draftAll({
        config,
        platforms,
        kind: "reply",
        subject: reply,
        sourceUrl: reply,
      });
    }
  } else {
    const subject = args.positionals.join(" ").trim();
    drafts = await draftAll({ config, platforms, kind: "post", subject });
  }

  saveDrafts(config, drafts);
  out(`Drafted ${drafts.length} item(s) — review, then approve:\n`);
  for (const d of drafts) printDraft(d, true);
  out(`\nApprove with:  content-copilot approve <id>`);
}

function cmdQueue(config: CopilotConfig): void {
  const drafts = listDrafts(config);
  if (drafts.length === 0) return void out("Queue empty. Run: content-copilot draft <topic>");
  const byStatus = (s: Draft["status"]) => drafts.filter((d) => d.status === s);
  for (const s of ["draft", "approved", "posted", "rejected"] as const) {
    const group = byStatus(s);
    if (group.length === 0) continue;
    out(`\n${statusIcon[s]} (${group.length})`);
    for (const d of group) printDraft(d, false);
  }
}

function cmdShow(args: Args, config: CopilotConfig): void {
  const id = args.positionals[0];
  if (!id) return fail("Usage: content-copilot show <id>");
  const d = getDraft(config, id);
  if (!d) return fail(`No draft with id ${id}`);
  printDraft(d, true);
}

function cmdSetStatus(args: Args, config: CopilotConfig, status: Draft["status"]): void {
  const id = args.positionals[0];
  if (!id) return fail(`Usage: content-copilot ${status === "approved" ? "approve" : "reject"} <id>`);
  const d = setStatus(config, id, status);
  if (!d) return fail(`No draft with id ${id}`);
  out(`${id} → ${statusIcon[status]}`);
}

async function cmdPost(args: Args, config: CopilotConfig): Promise<void> {
  const live = args.flags.get("live") === true;
  const id = args.positionals[0];
  let targets: Draft[];
  if (args.flags.get("all") === true) {
    targets = listDrafts(config).filter((d) => d.status === "approved");
    if (targets.length === 0) return void out("Nothing approved to post.");
  } else if (id) {
    const d = getDraft(config, id);
    if (!d) return fail(`No draft with id ${id}`);
    targets = [d];
  } else {
    return fail("Usage: content-copilot post <id> | post --all   [--live]");
  }

  for (const d of targets) {
    const r = await postDraft(config, d, { live });
    const mark = r.outcome === "posted" ? "●" : r.outcome === "refused" ? "⛔" : "✗";
    out(`  ${mark} ${r.platform}/${r.draftId}: ${r.outcome} — ${r.detail}`);
  }
  out(`\nAudit trail: ${auditPath(config)}`);
}

function cmdAudit(config: CopilotConfig): void {
  const path = auditPath(config);
  if (!existsSync(path)) return void out("No audit entries yet.");
  process.stdout.write(readFileSync(path, "utf8"));
}

function cmdConfig(config: CopilotConfig): void {
  out(JSON.stringify(config, null, 2));
}

function fail(msg: string): void {
  process.stderr.write(msg + "\n");
  process.exitCode = 1;
}

const HELP = `content-copilot — human-in-the-loop marketing co-pilot

  research                 Find relevant discussions/blogs (free sources) → queue
  candidates               List surfaced discussions
  draft <topic...>         Draft a post about a topic for your platforms
  draft --reply <id|url>   Draft replies to a surfaced candidate (or any URL)
  queue                    Show all drafts grouped by status
  show <id>                Print a draft's full body
  approve <id>             Mark a draft approved (REQUIRED before it can post)
  reject <id>              Mark a draft rejected
  post <id> | post --all   Post approved draft(s). Refuses anything not approved.
  audit                    Print the append-only post audit log
  config                   Print the effective config

  Flags: --platform x,linkedin,reddit,medium,discord   --live
  Offline/sample mode: CONTENT_COPILOT_FAKE=1
  Live drafting: set ANTHROPIC_API_KEY (uses claude-opus-4-8)
`;

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const cmd = argv[0];
  const args = parseArgs(argv.slice(1));
  const config = loadConfig();

  switch (cmd) {
    case "research":
      return cmdResearch(args, config);
    case "candidates":
      return cmdCandidates(config);
    case "draft":
      return cmdDraft(args, config);
    case "queue":
      return cmdQueue(config);
    case "show":
      return cmdShow(args, config);
    case "approve":
      return cmdSetStatus(args, config, "approved");
    case "reject":
      return cmdSetStatus(args, config, "rejected");
    case "post":
      return cmdPost(args, config);
    case "audit":
      return cmdAudit(config);
    case "config":
      return cmdConfig(config);
    case undefined:
    case "help":
    case "--help":
    case "-h":
      out(HELP);
      return;
    default:
      fail(`Unknown command: ${cmd}\n\n${HELP}`);
  }
}

main().catch((err) => {
  process.stderr.write(`error: ${(err as Error).message}\n`);
  process.exitCode = 1;
});
