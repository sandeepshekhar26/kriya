import type { CopilotConfig } from "./config.js";
import { PLATFORMS } from "./platforms.js";
import type { Candidate, Draft, Platform } from "./types.js";
import { nowIso, offline, shortHash } from "./util.js";

// The drafting stage. Produces a Draft per platform, NEVER posts. If an
// ANTHROPIC_API_KEY is present (and we're not in offline mode) it asks Claude
// for a tailored draft; otherwise it falls back to the deterministic template.
// Either way the result lands in the queue as `status: "draft"` for review.

const MODEL = "claude-opus-4-8";

export interface DraftRequest {
  config: CopilotConfig;
  platforms: Platform[];
  kind: "post" | "reply";
  subject: string;
  sourceUrl?: string;
}

export async function draftAll(req: DraftRequest): Promise<Draft[]> {
  const out: Draft[] = [];
  for (const platform of req.platforms) {
    out.push(await draftOne(platform, req));
  }
  return out;
}

async function draftOne(platform: Platform, req: DraftRequest): Promise<Draft> {
  const spec = PLATFORMS[platform];
  const templateBody = spec.template({
    config: req.config,
    kind: req.kind,
    subject: req.subject,
    sourceUrl: req.sourceUrl,
  });

  let body = templateBody;
  let generator: Draft["generator"] = "template";

  const key = process.env.ANTHROPIC_API_KEY;
  if (!offline() && key) {
    const llm = await tryLlm(key, platform, req, templateBody).catch((err) => {
      process.stderr.write(`  ! LLM draft for ${platform} failed: ${(err as Error).message}\n`);
      return null;
    });
    if (llm) {
      body = llm;
      generator = "llm";
    }
  }

  return {
    id: `${platform}-${shortHash(`${req.subject}|${req.sourceUrl ?? ""}|${body}`)}`,
    platform,
    kind: req.kind,
    subject: req.subject,
    sourceUrl: req.sourceUrl,
    body: body.trim(),
    status: "draft",
    generator,
    createdAt: nowIso(),
  };
}

async function tryLlm(
  apiKey: string,
  platform: Platform,
  req: DraftRequest,
  templateBody: string,
): Promise<string> {
  const spec = PLATFORMS[platform];
  const c = req.config;
  const context =
    req.kind === "reply"
      ? `You are drafting a REPLY to this discussion titled "${req.subject}"` +
        (req.sourceUrl ? ` (${req.sourceUrl})` : "") +
        `. Add genuine value first; the promotion is secondary and must feel earned.`
      : `You are drafting an ORIGINAL post about: ${req.subject || "the product"}.`;

  const prompt =
    `${context}\n\n` +
    `Product: ${c.product.name}\nPitch: ${c.product.pitch}\nRepo: ${c.product.repoUrl}\n\n` +
    `Channel: ${spec.label}. Style: ${spec.llmStyle}\n` +
    `Keep under ~${spec.maxChars} characters. Be honest, specific, no hype, no invented stats.\n` +
    `Here is a baseline draft to improve on (match its honesty, beat its phrasing):\n` +
    `"""\n${templateBody}\n"""\n\n` +
    `Return ONLY the final post text, nothing else.`;

  const res = await fetch("https://api.anthropic.com/v1/messages", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-api-key": apiKey,
      "anthropic-version": "2023-06-01",
    },
    body: JSON.stringify({
      model: MODEL,
      max_tokens: 1024,
      messages: [{ role: "user", content: prompt }],
    }),
  });

  if (!res.ok) throw new Error(`Anthropic API HTTP ${res.status}`);
  const data: unknown = await res.json();
  const text = extractText(data);
  if (!text) throw new Error("empty LLM response");
  return text;
}

function extractText(data: unknown): string {
  const content =
    data && typeof data === "object" ? (data as Record<string, unknown>).content : undefined;
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const block of content) {
    if (block && typeof block === "object") {
      const t = (block as Record<string, unknown>).text;
      if (typeof t === "string") parts.push(t);
    }
  }
  return parts.join("").trim();
}

/** Convenience: draft replies to a surfaced candidate across the given platforms. */
export function replyRequest(
  config: CopilotConfig,
  platforms: Platform[],
  candidate: Candidate,
): DraftRequest {
  return {
    config,
    platforms,
    kind: "reply",
    subject: candidate.title,
    sourceUrl: candidate.url,
  };
}
