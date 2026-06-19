# CONTINUE — the "advance kriya by one roadmap item" prompt

This is the **copy-paste prompt** to start a fresh session (or drive the daily routine — see the
bottom). Paste the block below as your first message. It's self-orienting: it points at the canonical
docs, picks the next item, builds it with real tests, and keeps the docs honest.

---

## ▶ Paste this

> You are continuing work on **kriya** (the governed in-process action layer; see `CLAUDE.md`).
> Advance the roadmap by **one well-scoped item**, to a shippable, tested state.
>
> **Orient first (don't skip):** read `CLAUDE.md`, `docs/ROADMAP.md`, and `docs/PRODUCT_GAPS.md`. If
> the task touches a past decision, read `docs/DECISIONS.md` (local-only). For any
> strategy/positioning question, read `docs/strategy/` (local-only) before researching anything new.
>
> **Pick the item:** if I named one (e.g. "do R9"), do that. Otherwise take the **top unblocked
> item by tier** — P1 before P2 before P3. As of 2026-06-19 the public-repo candidates are:
> **R9** (resume-ability UI + persist the approval queue), **R10** (OpenAI backend + retry/backoff +
> frontier-escalation fallback), **R11** (audit-receipt tamper tests + finish the budget /
> api-calls-hr cap), **R20** (durable host signing identity + tamper-evident log chaining — see
> `docs/SECURITY.md`), then the language bindings **R18** (C#/.NET) / **R19** (JVM) — but those are
> *demand-pulled*, only ahead of R9–R11 if I've named a design partner. The paid-tier items **R6**
> (live budget controls, inc 4) and **R8** (enterprise identity) live in the **private**
> `../kriya-console` repo, not here.
>
> **Build it** in small, verifiable steps. Match the surrounding code's style. New behavior needs new
> tests; nothing may regress. **Verify before claiming done** — run the full suite:
>
> ```bash
> # JS/TS
> npm run test --workspace kriya-core
> ( cd apps/note-app && npx tsc --noEmit ) && ( cd apps/task-manager && npx tsc --noEmit )
> # Python binding (R17)
> ( cd bindings/python && PYTHONPATH=src python3 -m unittest discover -s tests )
> # Rust (through note-app's lockfile; needs: source $HOME/.cargo/env)
> ( cd apps/note-app/src-tauri && cargo test -p kriya --locked && cargo check --locked )
> ( cd apps/task-manager/src-tauri && cargo check --locked )
> ```
>
> For anything the in-browser preview can exercise, run the app and confirm it works — don't ask me
> to check by hand.
>
> **Document + record:** move the shipped item to **Done** in `docs/ROADMAP.md` with the commit SHA,
> update `docs/PRODUCT_GAPS.md`, log any decision in `docs/DECISIONS.md`, and keep `README.md` honest.
>
> **Commit discipline:** one logical change per commit, with the `Co-Authored-By: Claude Opus 4.8
> <noreply@anthropic.com>` trailer; verify each before committing. **Strategy docs and
> `docs/DECISIONS.md` are gitignored — never try to commit them; the pushable record is the roadmap.**
>
> **Guardrails:** don't relicense the open SDK; don't copy private `kriya-console` code into this
> repo; **don't run `npm publish` / `cargo publish` / `twine upload`** — staging version bumps is
> fine, but the planner runs every publish ([D-004](PUBLISHING.md)). Keep the wedge (governed
> in-process layer for no-API local apps, [D-009](DECISIONS.md)); don't drift to "generic governance."

---

## Unattended mode (the daily routine)

The same prompt drives the daily routine, with one change for safety: **work on a fresh branch and
open a PR — never commit to `main` directly, and never publish.** The routine is configured to:

1. Pull latest `main`, orient, and pick the top unblocked item (as above).
2. Implement it on a new branch `auto/<item>-<date>`, with tests.
3. Run the full verify suite; only open the PR if it's green (otherwise open a **draft** PR titled
   `WIP: <item>` describing where it got stuck).
4. Open the PR with a summary of what changed and the verify output. Stop there — the planner
   reviews and merges.

To (re)create or change the routine, run the `/schedule` skill. To run one iteration right now in
this session, just paste the block above.
