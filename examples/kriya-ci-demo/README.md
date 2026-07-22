# kriya-ci demo — the governed CI lane (S4)

A minimal, **API-key-free** example of running a governed agent step in CI with
[`kriya-ci`](../../docs/GOVERNED-CI-LANE.md). See that doc for the full contract; this is the
runnable demo the example workflow uses.

## Pieces

- **`policy.yaml`** — the repo-committed, **deny-by-default** policy the lane enforces (allows a few
  read-only actions; everything else denies).
- **`agent.mjs`** — a deterministic "agent step": it routes a handful of tool calls through
  `kriya-govern` (the runtime's per-call govern + sign path), producing real Ed25519-signed,
  hash-chained receipts. No model, no network, no secrets — so it runs anywhere CI runs. It records
  a **blocked-attempt** receipt on a deny (the "attempts are evidence" discipline), which is what lets
  `kriya-ci` name the denied action in its verdict.

## Run it locally

```bash
# Build the binaries (from the repo root) and put them on PATH.
( cd apps/note-app/src-tauri && cargo build -p kriya --locked --bin kriya-ci --bin kriya-govern )
( cd tools/verify-receipts && cargo build )
export PATH="$PWD/apps/note-app/src-tauri/target/debug:$PWD/tools/verify-receipts/target/debug:$PATH"

# Governed CI lane: run the step, gate on the policy.
kriya-ci run --policy examples/kriya-ci-demo/policy.yaml \
             --audit-log kriya-ci-receipts.jsonl \
             -- node examples/kriya-ci-demo/agent.mjs
echo "kriya-ci exit: $?"          # 0 = clean (all allowed)

# Re-prove the evidence offline (independent of kriya-ci).
verify-receipts kriya-ci-receipts.jsonl && echo "receipts verify (exit 0)"
```

To see the **deny** path (exit `3`), remove the `http_get` allow rule from `policy.yaml` (the agent
still attempts it) and re-run — `kriya-ci` fails the job and names `http_get`, and the blocked-attempt
receipt is in the evidence.

The GitHub Actions version — build → run → verify offline → flip a byte and prove the auditor catches
it — is [`.github/workflows/kriya-ci-example.yml`](../../.github/workflows/kriya-ci-example.yml).
