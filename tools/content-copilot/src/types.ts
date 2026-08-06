// Shared shapes for the content co-pilot. Kept in one place so the research,
// drafting, queue, and posting stages all speak the same vocabulary.

/** The channels we draft for. Posting adapters are keyed off these. */
export type Platform = "x" | "linkedin" | "medium" | "reddit" | "discord";

export const ALL_PLATFORMS: readonly Platform[] = [
  "x",
  "linkedin",
  "medium",
  "reddit",
  "discord",
];

/** A discussion/blog the listening stage surfaced as relevant. */
export interface Candidate {
  /** Stable id (hash of the URL) so re-runs dedupe instead of pile up. */
  id: string;
  /** Where it came from, e.g. "hackernews", "reddit:mcp", "rss:example.com". */
  source: string;
  title: string;
  url: string;
  snippet: string;
  /** Which configured topics matched — the reason it surfaced. */
  matchedTopics: string[];
  /** Higher = more relevant. Simple: number of matched topics (+recency). */
  score: number;
  foundAt: string; // ISO timestamp
}

/** A post or reply, drafted and held for human approval before it can ship. */
export interface Draft {
  id: string;
  platform: Platform;
  /** "post" = original content about a topic; "reply" = a comment on a thread. */
  kind: "post" | "reply";
  /** The topic, or the title of the thread we're replying to. */
  subject: string;
  /** Set for replies — the thread/blog we'd respond to. */
  sourceUrl?: string;
  body: string;
  status: DraftStatus;
  /** How the body was produced — for honesty in the queue view. */
  generator: "llm" | "template";
  createdAt: string;
  approvedAt?: string;
  rejectedAt?: string;
  postedAt?: string;
}

export type DraftStatus = "draft" | "approved" | "rejected" | "posted";

/** One line in the append-only audit trail every post writes. */
export interface AuditEntry {
  at: string;
  event: "post.attempt" | "post.success" | "post.refused";
  platform: Platform;
  draftId: string;
  /** Hash of the posted body — lets you prove what went out without storing it twice. */
  bodyHash: string;
  detail?: string;
}
