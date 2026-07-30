import type { CopilotConfig } from "./config.js";
import type { AuditEntry, Draft, Platform } from "./types.js";
import { auditPath, setStatus } from "./queue.js";
import { appendLine, fullHash, nowIso } from "./util.js";

// The governed posting stage. This is where the human-in-the-loop guarantee
// lives: a draft is REFUSED unless it is explicitly `approved`. Every attempt —
// refused, attempted, or succeeded — is written to an append-only audit log
// with a hash of the body, so you can always prove what went out and what was
// blocked. This mirrors kriya's own permission → approval → audit gate; in
// production these post() adapters are the natural thing to wrap with kriya's
// `wrapAction(post, { permissions: ["post:x"] })` so the SAME governance the
// product sells also governs our marketing. (Dogfood seam — see README.)

export interface PostResult {
  draftId: string;
  platform: Platform;
  outcome: "posted" | "refused" | "error";
  detail: string;
}

/** A platform adapter: given a body, actually publish it and return a URL/id. */
type Adapter = (body: string, config: CopilotConfig) => Promise<string>;

// Real adapters need credentials the user supplies via env (see README). Until
// those are wired, every adapter runs in "dry" mode: it records the post to the
// audit log and returns a marker instead of hitting a live API. This keeps the
// approval + audit path real and testable without risking an accidental post.
function dryAdapter(platform: Platform): Adapter {
  return async (_body, _config) => `dry://${platform}/posted`;
}

const ADAPTERS: Record<Platform, Adapter> = {
  x: dryAdapter("x"),
  linkedin: dryAdapter("linkedin"),
  medium: dryAdapter("medium"),
  reddit: dryAdapter("reddit"),
  discord: dryAdapter("discord"),
};

function audit(config: CopilotConfig, entry: AuditEntry): void {
  appendLine(auditPath(config), JSON.stringify(entry));
}

/**
 * Attempt to post a single draft. The gate: only `approved` drafts ship.
 * `live` must be explicitly true to use a real adapter; default is dry-run.
 */
export async function postDraft(
  config: CopilotConfig,
  draft: Draft,
  opts: { live: boolean },
): Promise<PostResult> {
  const bodyHash = fullHash(draft.body);

  if (draft.status !== "approved") {
    audit(config, {
      at: nowIso(),
      event: "post.refused",
      platform: draft.platform,
      draftId: draft.id,
      bodyHash,
      detail: `status is "${draft.status}", not "approved"`,
    });
    return {
      draftId: draft.id,
      platform: draft.platform,
      outcome: "refused",
      detail: `Not approved (status: ${draft.status}). Run: approve ${draft.id}`,
    };
  }

  audit(config, {
    at: nowIso(),
    event: "post.attempt",
    platform: draft.platform,
    draftId: draft.id,
    bodyHash,
    detail: opts.live ? "live" : "dry-run",
  });

  try {
    const adapter = ADAPTERS[draft.platform];
    // In dry mode (default) we never touch a live API regardless of `live`,
    // because no real adapter is wired yet — this is the safe default.
    const ref = await adapter(draft.body, config);
    setStatus(config, draft.id, "posted");
    audit(config, {
      at: nowIso(),
      event: "post.success",
      platform: draft.platform,
      draftId: draft.id,
      bodyHash,
      detail: ref,
    });
    return {
      draftId: draft.id,
      platform: draft.platform,
      outcome: "posted",
      detail: ref,
    };
  } catch (err) {
    const msg = (err as Error).message;
    audit(config, {
      at: nowIso(),
      event: "post.refused",
      platform: draft.platform,
      draftId: draft.id,
      bodyHash,
      detail: `error: ${msg}`,
    });
    return { draftId: draft.id, platform: draft.platform, outcome: "error", detail: msg };
  }
}
