# The governed CI lane (`kriya-ci`)

Run an AI-agent step in CI **under a repo-committed policy**, **fail the build when the policy blocks
a governed action**, and keep the **signed receipts** as a build-evidence artifact that anyone can
**re-verify offline**. GitHub Actions first; the same shape drops into any CI as plain config.

> **Not "Policy CI".** This is the *enforcement gate on a live CI step*. It is **distinct from
> [Policy CI — "test before apply"](#relationship-to-policy-ci-i3)** (the counterfactual replay that
> previews a *candidate* policy's blast radius before you apply it). They compose: preview an edit
> with Policy CI, then let the governed CI lane enforce it.

## What it does

`kriya-ci run` wraps a governed agent step:

1. **Loads the committed policy — fail-closed.** A missing or invalid policy exits `4` and the step
   never runs (it never falls back to a permissive default — the B0-class bug this lane must not
   repeat).
2. **Runs the agent step**, exporting `KRIYA_POLICY` + `KRIYA_AUDIT_LOG` so the agent's governance
   lane (the Claude Code hook, `kriya-gateway`, or `kriya-govern`) writes signed receipts to one log
   under that policy.
3. **Gates.** A receipt for an action the policy **denies** — or **requires approval** for, which is
   a block in a headless run with no human — fails the job. A hash-chain break or unreadable log
   fails closed. Otherwise the job passes.
4. The receipts are uploaded as the build artifact and re-proved **offline** by `verify-receipts`
   (an independent second check — the actual tamper-evidence).

### Exit codes (stable — branch on them)

| code | meaning |
|---|---|
| `0` | **CLEAN** — the step ran; no governed action was blocked. |
| `3` | **POLICY_DENIED** — the policy blocked ≥1 governed action (named in the message). |
| `4` | **KRIYA_ERROR** — kriya's own error (missing/invalid policy, unreadable or chain-broken log). Fail-closed. |
| `5` | **STEP_FAILED** — the step exited non-zero for a reason that is *not* a recorded policy denial. |
| `2` | **USAGE** — bad arguments. |

## The honest ceiling — a **cooperative** lane

The CI runner is **cooperative**, not contained: whoever controls the workflow YAML can edit the
step out, and a hostile step could avoid routing its calls through kriya at all. So the governed CI
lane **governs + evidences a cooperative agent step; it does not contain it.** Containment is a
different capability (`kriya-gateway run -- <agent>`, a launch-under sandbox). The **tamper-evidence
comes from the offline `verify-receipts` re-prove**, not from trusting the runner — a flipped byte in
the artifact makes the auditor exit non-zero.

## GitHub Actions

The composite action lives at [`.github/actions/kriya-ci`](../.github/actions/kriya-ci/action.yml);
a full, runnable example (build → run → verify offline → tamper-detect) is
[`.github/workflows/kriya-ci-example.yml`](../.github/workflows/kriya-ci-example.yml) over
[`examples/kriya-ci-demo/`](../examples/kriya-ci-demo/).

```yaml
- name: Build the kriya binaries (put kriya-ci + the govern lane on PATH)
  # std-only + core → --no-default-features drops the tauri/glib/brotli host deps (no system packages).
  run: |
    ( cd crates/kriya && cargo build --no-default-features --bin kriya-ci --bin kriya-govern )
    echo "$PWD/crates/kriya/target/debug" >> "$GITHUB_PATH"

- name: Governed CI lane
  uses: ./.github/actions/kriya-ci
  with:
    policy: ci-policy.yaml                       # your repo-committed, deny-by-default policy
    command: node run-my-agent.mjs               # the governed agent step
    audit-log: kriya-ci-receipts.jsonl
    artifact-name: kriya-ci-receipts             # the signed evidence, uploaded

- name: Re-prove the evidence offline
  run: verify-receipts kriya-ci-receipts.jsonl   # exit 0 = authentic; a flipped byte → exit 1
```

## GitLab CI (config, not a second product)

The same lane is plain `.gitlab-ci.yml` — no GitLab-specific product, just the two binaries and the
exit-code contract:

```yaml
governed-ci-lane:
  image: rust:1.90
  script:
    - ( cd crates/kriya && cargo build --no-default-features --bin kriya-ci --bin kriya-govern )
    - ( cd tools/verify-receipts && cargo build )
    - export PATH="$PWD/crates/kriya/target/debug:$PWD/tools/verify-receipts/target/debug:$PATH"
    - kriya-ci run --policy ci-policy.yaml --audit-log kriya-ci-receipts.jsonl -- node run-my-agent.mjs
    - verify-receipts kriya-ci-receipts.jsonl           # offline re-prove
  artifacts:
    when: always                                        # the receipts are evidence even on a denial
    paths: [ kriya-ci-receipts.jsonl ]
```

(Jenkins / CircleCI / Buildkite are the same three lines: build, `kriya-ci run … -- <agent>`,
`verify-receipts`.)

## Relationship to Policy CI (I3)

| | **Governed CI lane** (`kriya-ci`, this doc) | **Policy CI — "test before apply"** (I3) |
|---|---|---|
| When | a **live CI step**, right now | **before** you apply a policy edit |
| Over what | the run's **own** receipts, under the run's **own** policy | last week's **already-verified** receipts, under a **candidate** policy |
| Question | "did this step do anything the policy blocks?" (enforce) | "how many past actions *would* this edit have blocked?" (preview) |
| Output | a build pass/fail + a signed receipts artifact | a signed `kriya.policy.sim.result` report |

Use I3 to choose a policy safely; use the governed CI lane to enforce it on every build.

## Tests

- Unit (`crates/kriya/src/bin/kriya-ci.rs`): allow → `0`; deny → `3` + named action; require-approval →
  block; governance-metadata (`kriya.*`) never gated; missing/invalid policy → `4`; chain break → `4`.
- End-to-end (`crates/kriya/tests/kriya_ci_smoke.rs`): the real `kriya-ci` + `kriya-govern` binaries
  over a scripted agent — clean exit 0 with verifiable receipts, a policy block → exit 3, missing
  policy → fail-closed exit 4.
- The workflow above is the CI acceptance: a real run producing an artifact `verify-receipts` re-proves
  offline (exit 0), plus a one-byte tamper the auditor catches (exit 1).
