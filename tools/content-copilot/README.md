# content-copilot

A **human-in-the-loop** marketing co-pilot for kriya. It does three things:

1. **Listens** — scans free, no-credential sources (Hacker News, Reddit, RSS) for discussions and
   blogs about your topics, scores them by relevance, and saves the matches.
2. **Drafts** — writes platform-tailored posts and replies (X, LinkedIn, Medium, Reddit, Discord),
   using Claude when an API key is set, or honest deterministic templates offline.
3. **Posts only what you approve** — every draft is held in a queue; the `post` command **refuses**
   anything you haven't explicitly approved, and writes an append-only, hashed audit log of every
   attempt.

> **Why it works this way.** Auto-posting promo and auto-commenting "try our repo" on other people's
> threads is spam — it gets accounts banned and torches a dev-tool brand. This tool deliberately keeps
> a human in the loop: it surfaces and drafts; **you** approve and post. That's the same
> permission → approval → audit discipline kriya sells, applied to our own marketing. See
> [docs/USE_CASES.md](../../docs/USE_CASES.md) for the product framing.

## Quick start

```bash
cd tools/content-copilot
npm install          # only @types/node + typescript (no runtime deps)
npm run build

# Try it fully offline (sample data, template drafts — no network, no keys):
CONTENT_COPILOT_FAKE=1 node dist/index.js research
CONTENT_COPILOT_FAKE=1 node dist/index.js draft --reply <candidateId>
node dist/index.js queue
node dist/index.js approve <draftId>
node dist/index.js post <draftId>      # dry-run by default; refuses if not approved
node dist/index.js audit
```

## Commands

| Command | What it does |
|---|---|
| `research` | Find relevant discussions/blogs → queue (`candidates.json`) |
| `candidates` | List surfaced discussions |
| `draft <topic...>` | Draft an original post about a topic |
| `draft --reply <id\|url>` | Draft value-first replies to a surfaced candidate (or any URL) |
| `queue` | Show all drafts grouped by status |
| `show <id>` | Print a draft's full body (edit the JSON by hand before approving if you like) |
| `approve <id>` / `reject <id>` | Gate a draft. **Approval is required before posting.** |
| `post <id>` / `post --all` | Post approved draft(s). Refuses anything not approved. |
| `audit` | Print the append-only post audit log |
| `config` | Print the effective config |

Flags: `--platform x,linkedin,reddit,medium,discord`, `--live`.

## Configuration

Copy [`content-copilot.config.json`](content-copilot.config.json) and edit `topics`, `sources`,
`platforms`, and your real `product.repoUrl`. Missing fields fall back to sensible defaults.

## Modes

- **Offline / sample** — `CONTENT_COPILOT_FAKE=1`: no network, deterministic sample candidates,
  template drafts. Good for trying it and for CI.
- **Live drafting** — set `ANTHROPIC_API_KEY`: drafts are written by `claude-opus-4-8`, improving on
  the template baseline. Listening still uses the free public sources.

## Posting is dry-run until you wire credentials (on purpose)

The platform adapters in [`src/post.ts`](src/post.ts) currently run in **dry mode**: an approved post
is recorded to the audit log and marked `posted`, but no live API is called. This keeps the
approve → audit path real and testable with zero risk of an accidental post. To go live, implement a
real adapter per platform (LinkedIn / X / Reddit / Discord all have official APIs; Medium and Reddit
should stay manual-paste to respect their norms) and read credentials from env.

## Dogfood seam → kriya

The `post()` adapters are the natural place to wrap with kriya itself:

```ts
import { wrapAction } from "kriya-core";
const postToX = wrapAction(realPostToX, {
  id: "post_x",
  description: "Publish an approved draft to X.",
  permissions: ["post:x"],   // policy gates it; high-stakes posts pause for approval
});
```

Then our own marketing runs through the exact governance the product sells — a clean internal
reference + a credible "we dogfood it" line for launch.
