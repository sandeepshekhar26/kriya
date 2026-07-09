# Security — the tamper-evident audit trail

> How kriya makes "what the agent did" **provable**, and — just as important — an honest account of
> what that proof does and does **not** guarantee today. The mechanism described here lives entirely
> in the open `kriya` crate (MIT); nothing here is hidden. If you're evaluating kriya for a regulated
> deployment, read the [Threat model](#threat-model--what-is-and-isnt-detected) and
> [Limitations](#limitations--planned-hardening) sections in full.

## TL;DR

Every action the host actually runs is recorded as an **Ed25519-signed receipt** appended to an
append-only JSONL log. The host holds the signing key; **the agent never sees it**. Anyone with the
public key can re-derive the signed bytes and verify each receipt offline — on their own machine,
with no network and no trust in kriya. Changing *any* signed field of a retained receipt (its
params, its result flag, *who* did it) invalidates that receipt's signature.

What this buys you: **a misbehaving or jailbroken agent cannot rewrite history, and no one can
silently edit a receipt after the fact without the key.** Receipts are also **hash-chained** (R20),
so whole-receipt **deletion / truncation / reorder** is detectable — turning "no retained receipt
was altered" into "the log is complete," and the host key can be made **durable** so the trust
anchor is stable across runs. What it does *not* buy you: protection against a *fully compromised
host process* (code running inside the trusted host can sign with the real key). That residual gap
is [called out below](#threat-model--what-is-and-isnt-detected).

## What gets signed (and what doesn't)

The signed unit is the **receipt** ([`crates/kriya/src/audit.rs`](../crates/kriya/src/audit.rs)):

| Field | Meaning |
|---|---|
| `step_id` | The agent step / call this receipt is for |
| `action_id` | Which typed action ran (e.g. `delete_transaction`) |
| `params` | The exact arguments it ran with |
| `success` | Whether the handler reported success |
| `ts_ms` | When it ran (epoch ms) |
| `actor` *(optional)* | **Who** did it — `{ agent, user }` (R8). Signed *inside* the receipt, so rewriting attribution breaks the signature just like rewriting params would. |
| `prev_hash` *(optional)* | **Chain pointer** — SHA-256 of the previous receipt line (R20). Signed, so the chain can't be silently re-pointed; absent on the genesis receipt (and on pre-R20 receipts, which therefore sign byte-identically). |

Two deliberate choices:

- **Only cleared actions get receipts.** A call the policy *denies*, or one a human *rejects* at the
  approval gate, never reaches the executor and is **never signed**. The audit log attests to *what
  actually ran*, not to what was attempted — blocked attempts are surfaced as `log` telemetry, not
  as receipts. (So "absence of a receipt" is not by itself evidence an action didn't happen — see the
  deletion caveat below.)
- **The result payload and app state are not signed.** Receipts bind the *decision and its inputs*
  (action + params + who + when + success), not the returned data. This keeps the canonical bytes
  small and stable.

The host also emits one special receipt type: the **on-device attestation** (R13), an
`action_id` of `kriya.attestation.on_device`, signed the same way — a verifiable record that a run
executed under a sealed policy with no remote egress.

## The cryptography

- **Algorithm:** Ed25519 (`ed25519-dalek`). 64-byte detached signatures, 32-byte public keys.
- **The signed message is canonical bytes.** The host signs `serde_json::to_vec(&receipt)` — compact
  JSON (no whitespace) of the unsigned receipt. Two rules make those bytes reproducible by any
  verifier:
  1. **Struct fields serialize in declaration order** — `step_id, action_id, params, success, ts_ms`,
     then `actor` *last and only if present*. An absent `actor` is omitted entirely, so a receipt
     with no attribution signs **byte-identically to the pre-R8 format** (older receipts and older
     verifiers keep validating unchanged).
  2. **`params` object keys serialize sorted** — R21 applies an **explicit recursive key-sort**
     before signing (so `{"b":1,"a":2}` always canonicalizes to `{"a":2,"b":1}`, at every nesting
     level), making the bytes reproducible regardless of any build's serde_json `preserve_order`
     feature; the offline verifier applies the identical sort.

  Reproduce those two rules and you get the exact bytes that were signed — the only way the signature
  can verify. (kriya's verifiers do exactly this: the Rust `verify-receipts` re-serializes the struct;
  the Console's TypeScript verifier re-emits the bytes by hand and pins SHA-512 via WebCrypto.)
- **The key lives in the host, out of the agent's reach.** The `Signer` holds the `SigningKey` in the
  host process. The agent only ever proposes `{action_id, params}`; it cannot read or use the key, so
  it cannot forge a receipt — the signature is applied by the host *after* the gates clear, on the
  bytes the host assembled.

## The audit log format

One JSON object per line (JSONL), appended, never rewritten. Each line is a `SignedReceipt`: the
receipt fields, flattened, plus the signer's `public_key` and the `signature` (both lowercase hex):

```json
{"step_id":"…","action_id":"delete_transaction","params":{"id":"txn-1"},"success":true,"ts_ms":1781528596227,"actor":{"agent":"claude-desktop","user":"alice"},"public_key":"f662…79a0","signature":"8025…1007"}
```

Default path: the **standard on-device location `~/.kriya/audit/`** for the `kriya-gateway` product —
the directory the Kriya Console app auto-discovers and tails — with a stable per-front filename; the
`kriya-host` developer CLI defaults to `$TMPDIR/kriya-audit.jsonl`. Either is overridable with
`--audit-log`. The host prints its public key at startup (`audit pubkey=…`) so an operator can
capture it for pinning (see next).

## Verifying offline

Two independent verifiers re-derive the canonical bytes and check every signature — neither needs
the network, and neither trusts kriya:

- **[`tools/verify-receipts`](../tools/verify-receipts/)** — a standalone Rust CLI.
  `verify-receipts [path]` exits `0` if all signatures verify, `1` if any fail or any line is
  malformed. Re-serializes the `Receipt` struct in declaration order — byte-for-byte what
  `audit.rs` signed.
- **The Console's TypeScript verifier** — verifies the same receipts in the browser via
  `@noble/ed25519`, byte-identical, and is cross-checked against real Rust-signed fixtures.

> **⚠️ Pin the public key.** Both verifiers check each receipt against the `public_key` *embedded in
> that receipt*. That detects modification and forgery of any line **whose key you trust** — but a
> verifier that accepts *any* embedded key would also accept a log an attacker re-signed wholesale
> with a key they generated. **A meaningful audit verifies two things: (1) the signature is valid, and
> (2) the `public_key` equals the host key you expect.** Capture the host's public key out of band
> (from its startup banner / your provisioning) and compare. The Console's "distinct signers" view
> exists for exactly this — it surfaces every key that signed your logs so an unexpected one stands
> out.

## Threat model — what is and isn't detected

**Detected (the signature fails):**

- ✅ **Editing a retained receipt** — changing `params`, `success`, `ts_ms`, `action_id`, or the
  `actor` of any receipt that stays in the log. (Covered by tamper tests in `audit.rs`.)
- ✅ **Forging a receipt** under a key you've pinned — you can't produce a valid signature without the
  private key.
- ✅ **A jailbroken / misbehaving agent rewriting its own history** — it never has the key, and
  blocked proposals were never signed in the first place.
- ✅ **Cross-checking attribution** — *who* (agent + operator) is inside the signed bytes, so
  who-did-what can't be quietly reassigned.
- ✅ **Whole-receipt deletion / truncation / reorder (R20)** — receipts are **hash-chained**:
  each carries `prev_hash` = SHA-256 of the previous line, *inside* the signed bytes (so the pointer
  can't be stripped without breaking the signature). Removing, truncating, or reordering a line makes
  the next receipt's `prev_hash` mismatch; `verify-receipts` prints `CHAIN-BREAK` and exits non-zero.
  The log is now provably *complete*, not just *unaltered*. (Caveat: a chain protects chained
  receipts; a log written entirely by a pre-R20 host has no chain to check.)

**Not detected (be aware):**

- ❌ **A fully compromised host process.** The signing key is in host memory. Code that compromises the
  host can sign forged receipts with the *real* key. The guarantee is against the *agent* and against
  *post-hoc editing by anything without the key* — not against arbitrary code execution inside the
  trusted host.
- ❌ **Trusting an unpinned key.** As above — verification is only as strong as pinning the expected
  public key.

## Limitations & hardening status

Calling these out is part of being trustworthy:

1. ✅ **Durable signing identity (R20, shipped).** The host can persist its Ed25519 identity with
   `kriya-host --signing-key <path>` (written `0600`, loaded if present, error-not-overwrite on a
   bad file), so the public key an auditor pins is **stable across runs** — "prove what every agent
   did over months," not just one session. The default (no flag) is still an ephemeral per-process
   key, fine for demos/CI.
2. ✅ **Completeness via hash-chaining (R20, shipped).** Each receipt is chained to its predecessor
   (`prev_hash` = SHA-256 of the prior line, *signed*), so whole-receipt deletion/truncation/reorder
   is detectable by `verify-receipts` (above), across host restarts (a new process seeds the chain
   head from the log's last line).
3. ⏳ **Remaining hardening.** Two residual gaps: (a) a **fully compromised host** can still sign
   forged receipts with the real key — reducing this needs HSM / OS-keychain key custody; and (b) an
   attacker who controls the host long enough could rewrite the *entire* chain from a fork point —
   **external anchoring** (periodically publishing the chain head off-box) would close that. Both are
   future work.
4. ✅ **Why `kriya-hook`'s approval tier doesn't use Claude Code's native `permissionDecision:
   "ask"` (B0, doc 22 §11).** It's a deliberate choice, not an oversight: `"ask"` has documented,
   reproducible reliability gaps — it doesn't always fire in headless `claude -p` mode and can race
   with tool execution there (anthropics/claude-code#40506, #36071), and has been observed silently
   overridden by a broad `permissions.allow` rule elsewhere in a user's settings, letting the tool
   run with **no prompt at all** (anthropics/claude-code#39344, #41151). `kriya-hook` instead uses
   exit-code-2 blocking (still fully supported, unaffected by those gaps) plus its own
   self-contained `ApprovalGate` (`tty`/`gui`) for the approval tier — a mechanism it controls
   end-to-end rather than depending on Claude Code's interactive-prompt plumbing.
5. ✅ **`TtyApproval` self-bounds at 300s (B0 fix).** Claude Code's own hook timeout (600s default
   for command-type hooks) **fails OPEN** on expiry — a killed/timed-out hook is treated as no
   decision, and the tool proceeds. (An earlier revision of `kriya-hook.rs`'s doc comment
   incorrectly claimed the opposite; corrected after verifying against the current hooks
   reference.) `TtyApproval::prompt_on_tty` previously blocked on an unbounded `/dev/tty` read, so
   an unanswered prompt could silently resolve to "allowed" once Claude Code's external timeout
   killed the hook. It now self-bounds at 300s (matching `GuiApproval`'s existing `osascript …
   giving up after 300`) and denies on its own timeout — the decision is made inside this binary,
   before either side's external timeout could make it by default.
6. ✅ **Headless + subagent hook firing, empirically verified (B0).** Two real, disputed-upstream
   behaviors that can't be unit-tested: does `PreToolUse`/`PostToolUse` fire reliably under
   `claude -p` (upstream reports of unreliability: #40506, #36071), and does it fire for tool calls
   made from *within* a Task-tool subagent (upstream dispute: #34692, reported broken on some
   platforms as recently as days before this check). Verified locally with
   `tools/verify-hook-firing.sh` against **Claude Code v2.1.186 on macOS**: both fired correctly —
   headless mode fired `PreToolUse`/`PostToolUse` for a plain Bash call, and a subagent-originated
   Bash call fired the hook with `agent_id` populated, correctly distinguishing it from the main
   thread. This confirms current behavior **on this platform/version only** — #34692's history
   shows platform-specific and version-specific regressions, so this is not a claim that every
   version/OS combination behaves the same; re-run the script and update this line if the installed
   `claude` version changes materially.

The honest one-line guarantee today: *"under a pinned (optionally durable) key, no retained receipt
was altered and none was silently deleted from the chain."*

## Why this shape (the regulated story)

For local/regulated apps the audit can't live in a cloud gateway — it has to be where the data and
the human are, **on-device, and verifiable without trusting the vendor**. Offline-verifiable
Ed25519 receipts give an auditor a check they can run themselves, on the machine, with no network —
which is what **EU AI Act** record-keeping (Art. 12) and **SOC 2** expect when an agent touches real
data. The cross-app aggregation, key-pinning, signer monitoring, and compliance-evidence export that
turn this primitive into an org-wide control are the job of [kriya Console](https://kriyanative.com)
— *the engine is open; the cockpit is paid.*

## Reporting a vulnerability

Found a way to forge or silently alter a receipt that this document claims is detected? That's a real
issue — email **Sandeepshekhar26@gmail.com** with steps to reproduce. Please don't open a public issue
for a security report.
