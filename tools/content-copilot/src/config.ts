import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { ALL_PLATFORMS, type Platform } from "./types.js";

export interface CopilotConfig {
  product: {
    name: string;
    repoUrl: string;
    /** One-breath pitch reused across drafts. */
    pitch: string;
  };
  /** Keywords used to FIND and SCORE discussions. Order doesn't matter. */
  topics: string[];
  sources: {
    hackernews: boolean;
    /** Subreddits to search (names only, no "r/"). */
    reddit: string[];
    /** Arbitrary RSS/Atom feed URLs to scan. */
    rss: string[];
  };
  /** Which channels to draft for by default. */
  platforms: Platform[];
  /** Where the queue + audit log live (relative to cwd). */
  queueDir: string;
}

export const DEFAULT_CONFIG: CopilotConfig = {
  product: {
    name: "kriya",
    repoUrl: "https://github.com/your-org/kriya",
    pitch:
      "A governed in-process action layer for local, API-less apps: expose your app's real " +
      "typed actions to any agent over MCP, with permission, human approval, budget, and " +
      "signed audit built in, on-device.",
  },
  topics: [
    "MCP",
    "model context protocol",
    "agent governance",
    "local-first AI",
    "computer use",
    "AI agent safety",
    "desktop agent",
    "tool calling",
  ],
  sources: {
    hackernews: true,
    reddit: ["LocalLLaMA", "mcp", "ArtificialIntelligence"],
    rss: [],
  },
  platforms: ["x", "linkedin", "reddit"],
  queueDir: ".content-copilot",
};

const CONFIG_FILE = "content-copilot.config.json";

/** Load config from CWD, shallow-merging onto the defaults. Missing file = defaults. */
export function loadConfig(cwd: string = process.cwd()): CopilotConfig {
  const path = resolve(cwd, CONFIG_FILE);
  if (!existsSync(path)) return DEFAULT_CONFIG;

  let parsed: Partial<CopilotConfig> = {};
  try {
    parsed = JSON.parse(readFileSync(path, "utf8")) as Partial<CopilotConfig>;
  } catch (err) {
    throw new Error(`Failed to parse ${CONFIG_FILE}: ${(err as Error).message}`);
  }

  const platforms = (parsed.platforms ?? DEFAULT_CONFIG.platforms).filter(
    (p): p is Platform => (ALL_PLATFORMS as readonly string[]).includes(p),
  );

  return {
    product: { ...DEFAULT_CONFIG.product, ...parsed.product },
    topics: parsed.topics ?? DEFAULT_CONFIG.topics,
    sources: { ...DEFAULT_CONFIG.sources, ...parsed.sources },
    platforms: platforms.length > 0 ? platforms : DEFAULT_CONFIG.platforms,
    queueDir: parsed.queueDir ?? DEFAULT_CONFIG.queueDir,
  };
}
