import type { CopilotConfig } from "./config.js";
import type { Candidate } from "./types.js";
import { matchTopics, nowIso, offline, shortHash } from "./util.js";

// The listening stage. It SURFACES relevant discussions/blogs — it never posts.
// Sources are deliberately free + keyless (HN Algolia, Reddit's public JSON, RSS)
// so it runs with zero setup and stays on the user's "local/free" preference.

const HTTP_TIMEOUT_MS = 8000;

async function getJson(url: string): Promise<unknown> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), HTTP_TIMEOUT_MS);
  try {
    const res = await fetch(url, {
      signal: ctrl.signal,
      headers: { "user-agent": "content-copilot (kriya marketing co-pilot)" },
    });
    if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
    return await res.json();
  } finally {
    clearTimeout(timer);
  }
}

async function getText(url: string): Promise<string> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), HTTP_TIMEOUT_MS);
  try {
    const res = await fetch(url, {
      signal: ctrl.signal,
      headers: { "user-agent": "content-copilot (kriya marketing co-pilot)" },
    });
    if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
    return await res.text();
  } finally {
    clearTimeout(timer);
  }
}

function toCandidate(
  source: string,
  title: string,
  url: string,
  snippet: string,
  topics: string[],
): Candidate | null {
  const matched = matchTopics(`${title} ${snippet}`, topics);
  if (matched.length === 0) return null;
  return {
    id: shortHash(url),
    source,
    title: title.trim(),
    url,
    snippet: snippet.slice(0, 280).trim(),
    matchedTopics: matched,
    score: matched.length,
    foundAt: nowIso(),
  };
}

async function searchHackerNews(topics: string[]): Promise<Candidate[]> {
  const out: Candidate[] = [];
  for (const topic of topics) {
    const url =
      "https://hn.algolia.com/api/v1/search_by_date?tags=(story,comment)&hitsPerPage=15&query=" +
      encodeURIComponent(topic);
    try {
      const data = getHits(await getJson(url));
      for (const hit of data) {
        const title = str(hit.title) || str(hit.story_title) || str(hit.comment_text);
        const link =
          str(hit.url) ||
          str(hit.story_url) ||
          `https://news.ycombinator.com/item?id=${str(hit.objectID)}`;
        if (!title || !link) continue;
        const c = toCandidate("hackernews", title, link, str(hit.comment_text), topics);
        if (c) out.push(c);
      }
    } catch (err) {
      warn(`HN search "${topic}" failed: ${(err as Error).message}`);
    }
  }
  return out;
}

async function searchReddit(subs: string[], topics: string[]): Promise<Candidate[]> {
  const out: Candidate[] = [];
  const query = topics.join(" OR ");
  for (const sub of subs) {
    const url =
      `https://www.reddit.com/r/${encodeURIComponent(sub)}/search.json` +
      `?restrict_sr=1&sort=new&limit=15&q=${encodeURIComponent(query)}`;
    try {
      const data = await getJson(url);
      const children = asArray(get(get(data, "data"), "children"));
      for (const child of children) {
        const d = get(child, "data");
        const title = str(get(d, "title"));
        const permalink = str(get(d, "permalink"));
        if (!title || !permalink) continue;
        const c = toCandidate(
          `reddit:${sub}`,
          title,
          `https://www.reddit.com${permalink}`,
          str(get(d, "selftext")),
          topics,
        );
        if (c) out.push(c);
      }
    } catch (err) {
      warn(`Reddit r/${sub} search failed: ${(err as Error).message}`);
    }
  }
  return out;
}

async function searchRss(feeds: string[], topics: string[]): Promise<Candidate[]> {
  const out: Candidate[] = [];
  for (const feed of feeds) {
    try {
      const xml = await getText(feed);
      const host = safeHost(feed);
      // Crude but dependency-free: pull <item>/<entry> title+link pairs.
      const items = xml.split(/<item[ >]|<entry[ >]/i).slice(1);
      for (const item of items) {
        const title = decodeXml(firstGroup(item, /<title[^>]*>([\s\S]*?)<\/title>/i));
        const link =
          firstGroup(item, /<link[^>]*href="([^"]+)"/i) ||
          decodeXml(firstGroup(item, /<link[^>]*>([\s\S]*?)<\/link>/i));
        if (!title || !link) continue;
        const desc = decodeXml(
          firstGroup(item, /<description[^>]*>([\s\S]*?)<\/description>/i),
        );
        const c = toCandidate(`rss:${host}`, title, link, desc, topics);
        if (c) out.push(c);
      }
    } catch (err) {
      warn(`RSS ${feed} failed: ${(err as Error).message}`);
    }
  }
  return out;
}

/** Deterministic samples so `research` runs (and is testable) without a network. */
function sampleCandidates(topics: string[]): Candidate[] {
  const seeds: Array<[string, string, string, string]> = [
    [
      "hackernews",
      "Show HN: I gave an AI agent typed actions instead of screen-scraping",
      "https://news.ycombinator.com/item?id=00000001",
      "Been experimenting with MCP and local-first AI — screen scraping is too flaky for real apps.",
    ],
    [
      "reddit:mcp",
      "How do you handle agent governance / human approval with MCP?",
      "https://www.reddit.com/r/mcp/comments/0000/agent_governance/",
      "Building a desktop agent and worried about tool calling with no audit trail or approval step.",
    ],
    [
      "rss:example.com",
      "The case for AI agent safety in desktop software",
      "https://example.com/blog/ai-agent-safety",
      "Computer use is impressive but ungoverned. Local-first AI needs permissioned actions.",
    ],
  ];
  return seeds
    .map(([source, title, url, snippet]) => toCandidate(source, title, url, snippet, topics))
    .filter((c): c is Candidate => c !== null);
}

/** Run all configured sources and return deduped, score-sorted candidates. */
export async function research(config: CopilotConfig): Promise<Candidate[]> {
  let found: Candidate[];
  if (offline()) {
    found = sampleCandidates(config.topics);
  } else {
    const batches = await Promise.all([
      config.sources.hackernews ? searchHackerNews(config.topics) : Promise.resolve([]),
      config.sources.reddit.length
        ? searchReddit(config.sources.reddit, config.topics)
        : Promise.resolve([]),
      config.sources.rss.length
        ? searchRss(config.sources.rss, config.topics)
        : Promise.resolve([]),
    ]);
    found = batches.flat();
  }

  const byId = new Map<string, Candidate>();
  for (const c of found) {
    if (!byId.has(c.id)) byId.set(c.id, c);
  }
  return [...byId.values()].sort((a, b) => b.score - a.score);
}

// --- tiny, dependency-free helpers for poking at untyped JSON / XML ---

function get(obj: unknown, key: string): unknown {
  return obj && typeof obj === "object" ? (obj as Record<string, unknown>)[key] : undefined;
}
function asArray(v: unknown): unknown[] {
  return Array.isArray(v) ? v : [];
}
function getHits(data: unknown): Record<string, unknown>[] {
  const hits = get(data, "hits");
  return asArray(hits).filter(
    (h): h is Record<string, unknown> => !!h && typeof h === "object",
  );
}
function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}
function firstGroup(text: string, re: RegExp): string {
  const m = re.exec(text);
  return m && m[1] ? m[1].trim() : "";
}
function decodeXml(s: string): string {
  return s
    .replace(/<!\[CDATA\[([\s\S]*?)\]\]>/g, "$1")
    .replace(/<[^>]+>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, "&")
    .trim();
}
function safeHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "feed";
  }
}
function warn(msg: string): void {
  process.stderr.write(`  ! ${msg}\n`);
}
