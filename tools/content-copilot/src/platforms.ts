import type { CopilotConfig } from "./config.js";
import type { Platform } from "./types.js";

export interface PlatformSpec {
  label: string;
  /** Soft length guidance, surfaced to the LLM and used to trim templates. */
  maxChars: number;
  /** Offline, deterministic draft used when no LLM key is set. */
  template: (args: TemplateArgs) => string;
  /** Extra steering appended to the LLM prompt for this channel's voice. */
  llmStyle: string;
}

export interface TemplateArgs {
  config: CopilotConfig;
  kind: "post" | "reply";
  subject: string;
  sourceUrl?: string;
}

const sig = (c: CopilotConfig) => `${c.product.name} — ${c.product.repoUrl}`;

export const PLATFORMS: Record<Platform, PlatformSpec> = {
  x: {
    label: "X / Twitter",
    maxChars: 280,
    llmStyle:
      "Punchy, lowercase-friendly, one idea, a hook in the first line, no hashtags spam (0-1 max). " +
      "If it's a reply, lead with genuine value, then a soft mention of the repo.",
    template: ({ config, kind, subject }) =>
      kind === "reply"
        ? `good thread on ${subject}. the missing piece for local apps is governed, typed ` +
          `actions (not screen-scraping) — permission + human approval + signed audit, on-device. ` +
          `we open-sourced it: ${config.product.repoUrl}`
        : `agents are becoming users of your app. for local, api-less software the only safe way ` +
          `in is governed typed actions — not screenshots. permissioned, human-approved, ` +
          `audited on-device.\n\n${config.product.name}: ${config.product.repoUrl}`,
  },
  linkedin: {
    label: "LinkedIn",
    maxChars: 1300,
    llmStyle:
      "Professional but not corporate. 2-4 short paragraphs, a clear takeaway, no emoji walls. " +
      "Frame around the business problem (governance/compliance for on-device agents).",
    template: ({ config, subject }) =>
      `AI agents are starting to operate the software we use every day. But for local, ` +
      `API-less apps — POS, CRM, regulated workstations — the usual answer (wrap your REST API) ` +
      `doesn't exist, and screen-scraping is too flaky and ungoverned for real data.\n\n` +
      `The alternative: expose the app's real typed actions to any agent, with permission, ` +
      `human approval, budget, and signed audit built in — on-device. Same handler serves a ` +
      `human's click and an agent's call.\n\n` +
      `That's what we're building with ${config.product.name}${subject ? ` (on ${subject})` : ""}. ` +
      `It's open source: ${config.product.repoUrl}`,
  },
  medium: {
    label: "Medium / blog",
    maxChars: 4000,
    llmStyle:
      "Return a blog OUTLINE: a working title, a one-line hook, then 4-6 section headings each " +
      "with a one-sentence summary. Technical, honest, no hype.",
    template: ({ config, subject }) =>
      `# Why your local app needs a governed action layer, not a screen-scraping agent\n\n` +
      `_Hook: agents are becoming users of software — and for local, API-less apps the safe ` +
      `entry point doesn't exist yet._\n\n` +
      `1. The problem: capabilities that live only inside a running local app (no API to wrap).\n` +
      `2. Why the obvious options fail: REST+MCP (no API), computer-use (flaky, ungoverned), ` +
      `cloud agent (data can't leave the device).\n` +
      `3. The third option: expose the app's real typed actions over MCP.\n` +
      `4. Governance as the moat: permission, human approval, budget, signed audit — on-device.\n` +
      `5. ${subject ? `Worked example: ${subject}.` : `A worked example (bolt-on in <50 LOC).`}\n` +
      `6. The honest limit: apps with a good cloud API + non-private data don't need this.\n\n` +
      `Repo: ${config.product.repoUrl}`,
  },
  reddit: {
    label: "Reddit",
    maxChars: 1500,
    llmStyle:
      "Value-FIRST, no marketing voice — Reddit punishes promotion. Answer the actual question or " +
      "add a genuine insight, then disclose the repo plainly ('full disclosure, I built this'). " +
      "Never lead with the link.",
    template: ({ config, kind, subject }) =>
      kind === "reply"
        ? `On ${subject}: the thing that finally worked for us was treating agent access as ` +
          `*typed actions through the app*, not screen-scraping. Each action is permissioned, ` +
          `high-stakes ones pause for human approval, and everything's signed + audited — all ` +
          `on-device, so private data never leaves.\n\nFull disclosure, I'm building an ` +
          `open-source layer for exactly this: ${config.product.repoUrl}. Happy to answer questions.`
        : `Sharing something we open-sourced for local-first AI apps: a governed action layer so ` +
          `an on-device agent can drive your app through typed, permissioned, audited actions ` +
          `(instead of screen-scraping). Full disclosure, I built it — feedback welcome.\n\n` +
          `${config.product.repoUrl}`,
  },
  discord: {
    label: "Discord",
    maxChars: 1000,
    llmStyle:
      "Casual, friendly, conversational. Short. Like talking to other builders. One soft link.",
    template: ({ config, kind, subject }) =>
      kind === "reply"
        ? `oh this is right up our alley — ${subject}. we went the typed-actions route (perms + ` +
          `human approval + audit, all on-device) instead of screen-scraping. open source if ` +
          `useful: ${config.product.repoUrl}`
        : `hey folks — been building ${config.product.name}, a governed action layer that lets an ` +
          `on-device agent safely drive local/api-less apps (typed actions, approval, audit). ` +
          `would love feedback: ${config.product.repoUrl}`,
  },
};

export function platformSig(config: CopilotConfig): string {
  return sig(config);
}
