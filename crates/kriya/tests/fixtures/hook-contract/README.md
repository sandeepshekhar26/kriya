# Claude Code hook payload contract — pinned fixtures (W0-3 / S3)

These JSON files pin the **PreToolUse / PostToolUse** stdin payload shape that `kriya-hook` consumes,
so that the run-correlation fields it emits (`kriya.corr.run_id` from `session_id`,
`kriya.corr.agent_id` from `agent_id`) stay wired to a real, versioned contract — if the contract
drifts, `kriya_hook_smoke.rs` breaks loudly instead of silently emitting nothing.

## Provenance (be honest about how this was pinned)

- **Verified 2026-07-22** against the official Claude Code hooks reference
  (`https://code.claude.com/docs/en/hooks`) and cross-checked against this repo's own long-standing
  `HookPayload` struct + `kriya_hook_smoke.rs` shape.
- A **live, model-driven capture on this Mac was blocked**: a nested `claude -p` cannot reuse the
  host session's brokered credentials (it reports "Not logged in" / 401), so these payloads are
  authored to the documented contract rather than captured from a live run. The **binary they drive
  is the real one**, so the receipts are genuinely signed and chained — only the payload *source* is
  the pinned contract, exactly as the pre-existing smoke test already does.

## The fields (documented)

Common to `PreToolUse` and `PostToolUse`: `session_id`, `transcript_path`, `cwd`, `permission_mode`,
`hook_event_name`, `tool_name`, `tool_input`, and a per-agent `agent_id` (+ `agent_type`).
`PostToolUse` adds the tool result (`tool_response`, the field this binary reads).

## Sub-agent lineage — the load-bearing finding

When the main agent spawns a subagent (Task/Agent tool) and that subagent makes its own tool calls:

- the subagent's tool calls carry the **same `session_id`** as the parent (→ one `run_id`), and
- a **different `agent_id`** than the main agent (→ the sub-agent discriminator), but
- **no parent pointer** exists anywhere in the payload — there is no `parent_session_id`,
  `parent_tool_use_id`, `parent_agent_id`, or `isSubagent` field.

So the hook honestly emits `run_id` + `agent_id` and **never** a `parent_step_id` it cannot see
(doc 24 locus discipline: absent is honest, guessed is not). The middleware lane, which owns a real
parent step, is where `parent_step_id` gets populated.
