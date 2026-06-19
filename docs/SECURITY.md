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
silently edit a receipt after the fact without the key.** What it does *not* yet buy you: detection
of *whole-receipt deletion*, or protection against a *fully compromised host process*. Those are
[called out below](#threat-model--what-is-and-isnt-detected) and are on the
[hardening roadmap](#limitations--planned-hardening).

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
  2. **`params` object keys serialize sorted** — `params` is a `serde_json::Value`, whose maps are
     ordered, so `{"b":1,"a":2}` always canonicalizes to `{"a":2,"b":1}`.

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

Default path: `$TMPDIR/kriya-audit.jsonl` (override with `--audit-log`). The host prints its public
key at startup (`audit pubkey=…`) so an operator can capture it for pinning (see next).

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

**Not detected today (be aware):**

- ❌ **Whole-receipt deletion or truncation.** Receipts are *independently* signed lines; there is no
  hash chain linking line *N* to line *N-1*. An attacker with write access to the log file can delete
  or truncate entire receipts and every *remaining* line still verifies. Signatures prove *no retained
  receipt was altered*; they do **not** prove *completeness* of the log. (Closing this is on the
  roadmap — see below.)
- ❌ **A fully compromised host process.** The signing key is in host memory. Code that compromises the
  host can sign forged receipts with the *real* key. The guarantee is against the *agent* and against
  *post-hoc editing by anything without the key* — not against arbitrary code execution inside the
  trusted host.
- ❌ **Trusting an unpinned key.** As above — verification is only as strong as pinning the expected
  public key.

## Limitations & planned hardening

These are known, and tracked — calling them out is part of being trustworthy:

1. **Ephemeral signing key (today).** The host currently mints a fresh Ed25519 keypair per process
   (`rand::random()` at startup); it is not persisted. So the trust anchor *rotates every run*, and
   pinning is per-run rather than per-deployment. Fine for a demo or a single session; insufficient
   for "prove what every agent did over months."
2. **No completeness guarantee.** Independently-signed lines can't detect deletion (above).

Both are addressed by roadmap item **R20 (durable host signing identity + tamper-evident log
chaining)**: a persisted (optionally OS-keychain / hardware-backed) host identity key so the trust
anchor is stable across runs, plus hash-chaining each receipt to its predecessor (and optional
external anchoring) so deletion/truncation becomes detectable. Until R20 lands, treat the guarantee
as *"no retained receipt under a pinned key was altered,"* not *"the log is complete and the signer is
permanent."*

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
