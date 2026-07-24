//! Offline verifier for the `kriya` Ed25519-signed audit log.
//!
//! The agent host appends one JSON object per line to `kriya-audit.jsonl`.
//! Each line is a [`SignedReceipt`]: the unsigned [`Receipt`] fields flattened,
//! followed by `public_key` and `signature` (both lowercase hex). This binary
//! re-derives the canonical message bytes and verifies every signature.
//!
//! # Usage
//!
//! ```text
//! verify-receipts [path]
//! ```
//!
//! `path` defaults to `$TMPDIR/kriya-audit.jsonl` (same as the host).
//! Exit code 0 when all signatures verify; 1 when any FAIL or parse error occurs.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Data types — field order MUST match audit.rs exactly (it determines the
// canonical serialization order that was signed).
// ---------------------------------------------------------------------------

/// Who took the action (R8). Mirrors `kriya::audit::Actor` — serialized in declaration
/// order (`agent`, then `user`), which is also alphabetical, so it matches the host's
/// canonical bytes whether re-derived by struct order or by sorted-key order.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Actor {
    agent: String,
    user: String,
}

/// The unsigned portion of a receipt. Field order is load-bearing: serde_json
/// serializes struct fields in declaration order, and the host signs
/// `serde_json::to_vec(&receipt)` over this exact shape.
///
/// Note: the host declares `ts_ms` as `u128`, but all realistic epoch-millisecond
/// timestamps fit in `u64`. Standard serde_json cannot deserialize `u128` values;
/// the serialized bytes are identical for both types while the value fits in `u64`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    step_id: String,
    action_id: String,
    params: Value,
    success: bool,
    ts_ms: u64,
    /// Optional identity attribution (R8). Declared LAST and skipped when absent so the
    /// re-derived canonical bytes are byte-identical to the host's for both the original
    /// (actor-less) receipts and the new attributed ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor: Option<Actor>,
    /// Hash of the previous receipt LINE in the log (R20). Declared LAST and skipped when absent so
    /// an unchained (genesis / pre-R20) receipt re-derives byte-identically. Part of the signed
    /// bytes; the chain is verified against the SHA-256 of the preceding raw line in `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prev_hash: Option<String>,
}

/// A full JSONL line as written by the host: the Receipt fields flattened,
/// then `public_key` and `signature`.
#[derive(Debug, Serialize, Deserialize)]
struct SignedReceipt {
    #[serde(flatten)]
    receipt: Receipt,
    public_key: String,
    signature: String,
    /// A5 (design D2): additive top-level wire siblings — mirrors `kriya::audit::SignedReceipt`
    /// exactly. `None` (omitted from the wire) on every pre-A5 / non-PQ receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pq_alg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pq_public_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pq_sig: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pq_key_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Outcome of verifying one line.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Fail(String),
}

/// Verify one JSONL line. Returns `Outcome::Ok` when the signature is valid,
/// `Outcome::Fail(reason)` otherwise.
fn verify_line(line: &str) -> (String, String, Outcome) {
    // ── parse ──────────────────────────────────────────────────────────────
    let signed: SignedReceipt = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return (
                "(parse error)".to_string(),
                "(parse error)".to_string(),
                Outcome::Fail(format!("JSON parse: {e}")),
            );
        }
    };

    let action_id = signed.receipt.action_id.clone();
    let step_id = signed.receipt.step_id.clone();

    // ── decode hex ─────────────────────────────────────────────────────────
    let pub_bytes: [u8; 32] = match hex::decode(&signed.public_key)
        .ok()
        .and_then(|b| b.try_into().ok())
    {
        Some(b) => b,
        None => {
            return (
                action_id,
                step_id,
                Outcome::Fail("invalid public_key hex (need 32 bytes)".to_string()),
            );
        }
    };

    let sig_bytes: [u8; 64] = match hex::decode(&signed.signature)
        .ok()
        .and_then(|b| b.try_into().ok())
    {
        Some(b) => b,
        None => {
            return (
                action_id,
                step_id,
                Outcome::Fail("invalid signature hex (need 64 bytes)".to_string()),
            );
        }
    };

    // Captured before `signed.receipt` is moved below — used by the PQ require-if-present check
    // (design §5) after the Ed25519 verify. Cheap clone of up to four short hex strings.
    #[cfg(feature = "pq-crypto")]
    let (pq_alg_owned, pq_pk_owned, pq_sig_owned, pq_key_id_owned) = (
        signed.pq_alg.clone(),
        signed.pq_public_key.clone(),
        signed.pq_sig.clone(),
        signed.pq_key_id.clone(),
    );
    #[cfg(feature = "pq-crypto")]
    let signed_pq = PqSiblings {
        pq_alg: &pq_alg_owned,
        pq_public_key: &pq_pk_owned,
        pq_sig: &pq_sig_owned,
        pq_key_id: &pq_key_id_owned,
    };

    // ── canonical message — must byte-match what audit.rs signed ───────────
    // `serde_json::to_vec` of the unsigned Receipt struct (struct fields serialize in declaration
    // order: step_id, action_id, params, success, ts_ms, actor), with `params` object keys sorted
    // by the identical canonicalization audit.rs applies (R21) — so the bytes match regardless of
    // either build's serde_json `preserve_order` setting.
    let mut receipt = signed.receipt;
    receipt.params = canonical_value(&receipt.params);
    let msg = match serde_json::to_vec(&receipt) {
        Ok(v) => v,
        Err(e) => {
            return (
                action_id,
                step_id,
                Outcome::Fail(format!("failed to serialize receipt: {e}")),
            );
        }
    };

    // ── verify (A4: routed through the shared crypto facade, kriya::crypto) ────────────────
    if !kriya::crypto::verify(&pub_bytes, &msg, &sig_bytes) {
        return (
            action_id,
            step_id,
            Outcome::Fail("signature does not match receipt".to_string()),
        );
    }

    // A5 (design §5 verification matrix, rows 3/5/6): require-if-present PQ check. When this
    // binary is NOT built with `pq-crypto`, `pq_*` siblings are ignored entirely (axiom §1.5 —
    // old verifiers ignore unknown fields; this offline CLI without the feature behaves exactly
    // like a pre-A5 verifier). Only checked when the feature IS compiled in.
    #[cfg(feature = "pq-crypto")]
    if let Outcome::Fail(reason) = pq_check(&signed_pq, &msg) {
        return (action_id, step_id, Outcome::Fail(reason));
    }

    (action_id, step_id, Outcome::Ok)
}

/// A5 (design §5): the require-if-present PQ verdict for one line's `pq_*` siblings against the
/// identical canonical `msg` bytes the Ed25519 signature covers. Absent-entirely is `Ok` (row 1);
/// a complete, valid set is `Ok` (row 2); anything else is `Fail` with the design's exact reason
/// string (rows 3/5/6). Free function (not a `SignedReceipt` method) so the caller can pass just
/// the four `pq_*` fields — kept separate from `verify_line`'s early-return control flow above.
#[cfg(feature = "pq-crypto")]
struct PqSiblings<'a> {
    pq_alg: &'a Option<String>,
    pq_public_key: &'a Option<String>,
    pq_sig: &'a Option<String>,
    pq_key_id: &'a Option<String>,
}

#[cfg(feature = "pq-crypto")]
fn pq_check(pq: &PqSiblings, msg: &[u8]) -> Outcome {
    let any_present =
        pq.pq_alg.is_some() || pq.pq_public_key.is_some() || pq.pq_sig.is_some() || pq.pq_key_id.is_some();
    if !any_present {
        return Outcome::Ok; // row 1 — no PQ material at all.
    }
    let (Some(alg), Some(pk_hex), Some(sig_hex), Some(key_id)) =
        (pq.pq_alg, pq.pq_public_key, pq.pq_sig, pq.pq_key_id)
    else {
        let missing = if pq.pq_alg.is_none() {
            "pq_alg"
        } else if pq.pq_public_key.is_none() {
            "pq_public_key"
        } else if pq.pq_sig.is_none() {
            "pq_sig"
        } else {
            "pq_key_id"
        };
        return Outcome::Fail(format!(
            "incomplete or inconsistent PQ signature ({missing})"
        ));
    };
    if alg != "ML-DSA-87" {
        return Outcome::Fail(format!("unsupported pq_alg: {alg} (expected ML-DSA-87)"));
    }
    let Ok(pk) = hex::decode(pk_hex) else {
        return Outcome::Fail("incomplete or inconsistent PQ signature (pq_public_key)".to_string());
    };
    let Ok(sig) = hex::decode(sig_hex) else {
        return Outcome::Fail("incomplete or inconsistent PQ signature (pq_sig)".to_string());
    };
    let expected_key_id = sha256_hex(&pk)[..16].to_string();
    if key_id != &expected_key_id {
        return Outcome::Fail("incomplete or inconsistent PQ signature (pq_key_id)".to_string());
    }
    if kriya::crypto::pq_verify(&pk, msg, &sig) {
        Outcome::Ok
    } else {
        Outcome::Fail("pq_sig (ML-DSA-87) does not match receipt".to_string())
    }
}

/// Recursively sort object keys so the re-derived canonical bytes are independent of serde_json's
/// `preserve_order` feature — byte-for-byte identical to `kriya::audit`'s canonicalization (R21).
fn canonical_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonical_value(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}

/// Lowercase-hex SHA-256 — must match `kriya::audit`'s chain hash (over the exact raw line) so the
/// chain can be re-checked offline (R20).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let path: PathBuf = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("kriya-audit.jsonl"));

    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: cannot read {:?}: {e}", path);
            process::exit(1);
        }
    };

    let (ok_count, fail_count, chain_breaks) = verify_log(&content);
    println!("verified {ok_count}, failed {fail_count}, chain breaks {chain_breaks}");

    if fail_count > 0 || chain_breaks > 0 {
        process::exit(1);
    }
}

/// Verify every line of an audit log: (1) **signatures** — no retained receipt was altered; and
/// (2) the **hash chain** — the log is complete (no whole-receipt deletion/truncation/reorder).
/// Prints a per-line report and returns `(ok, failed, chain_breaks)`. Pure over its input so it is
/// unit-testable without a file or `process::exit`.
fn verify_log(content: &str) -> (u32, u32, u32) {
    let mut ok_count: u32 = 0;
    let mut fail_count: u32 = 0;
    let mut chain_breaks: u32 = 0;
    // SHA-256 of the previous non-empty line, to check the next receipt's prev_hash against (R20).
    let mut prev_line_hash: Option<String> = None;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // 1) signature: no retained receipt was altered.
        let (action_id, step_id, outcome) = verify_line(line);
        match outcome {
            Outcome::Ok => {
                println!("OK   {action_id} {step_id}");
                ok_count += 1;
            }
            Outcome::Fail(reason) => {
                println!("FAIL {action_id} {step_id}  [{reason}]");
                fail_count += 1;
            }
        }

        // 2) chain: a receipt that declares a prev_hash must match the SHA-256 of the preceding
        //    line. prev_hash is inside the signed bytes, so it can't be stripped without failing the
        //    signature check above. Unchained receipts (genesis, or pre-R20 logs) carry no prev_hash
        //    → not chain-checked (backward compatible).
        if let Ok(signed) = serde_json::from_str::<SignedReceipt>(line) {
            // A retention epoch-checkpoint (doc 24 §6-P2) is a SIGNED, legitimate sealed chain point:
            // it seals a pruned prefix, so at the head of the log its `prev_hash` records the prior
            // head H whose line was intentionally pruned. Accept it as a seal rather than flagging a
            // truncation — the checkpoint is itself signed (verified above) and its `prior_head_hash`
            // param records H. (The kriya crate names this `RETENTION_CHECKPOINT`.)
            let is_checkpoint = signed.receipt.action_id == "kriya.retention.checkpoint";
            match (&signed.receipt.prev_hash, &prev_line_hash) {
                (Some(claimed), Some(actual)) if claimed != actual => {
                    println!("CHAIN-BREAK {action_id} {step_id}  [prev_hash != previous line — a preceding receipt was deleted, reordered, or altered]");
                    chain_breaks += 1;
                }
                (Some(_), None) if is_checkpoint => {
                    println!("RETENTION-SEAL {action_id} {step_id}  [retention checkpoint — the prior prefix was pruned per policy; H recorded]");
                }
                (Some(_), None) => {
                    println!("CHAIN-BREAK {action_id} {step_id}  [first line claims a predecessor — the head of the log was truncated]");
                    chain_breaks += 1;
                }
                _ => {} // genesis, exact match, or legacy-unchained → no break
            }
        }
        prev_line_hash = Some(sha256_hex(line.as_bytes()));
    }

    (ok_count, fail_count, chain_breaks)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kriya::crypto::SigningKey;
    use serde_json::json;

    // Fixed 32-byte seed keys — deterministic, no RNG crate needed.
    // (These are the RFC 8037 test vectors, safe to use in tests.)
    const KEY_A: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
        0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
        0x1c, 0xae, 0x7f, 0x60,
    ];
    const KEY_B: [u8; 32] = [
        0x4c, 0xcd, 0x08, 0x9b, 0x28, 0xff, 0x96, 0xda, 0x9d, 0xb6, 0xc3, 0x46, 0xec, 0x11,
        0x4e, 0x0f, 0x5b, 0x8a, 0x31, 0x9f, 0x35, 0xab, 0xa6, 0x24, 0xda, 0x8c, 0xf6, 0xed,
        0x4d, 0x0b, 0x24, 0x55,
    ];

    /// Replicate audit.rs's `Signer::record` so test signing is byte-identical.
    fn sign_receipt(key: &SigningKey, receipt: &Receipt) -> SignedReceipt {
        let msg = serde_json::to_vec(receipt).expect("serialize receipt");
        let signature = hex::encode(key.sign(&msg));
        let public_key = hex::encode(key.public_key());
        SignedReceipt {
            receipt: receipt.clone(),
            public_key,
            signature,
            pq_alg: None,
            pq_public_key: None,
            pq_sig: None,
            pq_key_id: None,
        }
    }

    /// A5: sign `receipt` with BOTH Ed25519 (`key`) and ML-DSA-87 (`pq_key`) — the per-receipt
    /// dual-sign wire shape (design D2).
    #[cfg(feature = "pq-crypto")]
    fn sign_receipt_dual(
        key: &SigningKey,
        pq_key: &kriya::crypto::PqSigningKey,
        receipt: &Receipt,
    ) -> SignedReceipt {
        let mut signed = sign_receipt(key, receipt);
        let msg = serde_json::to_vec(receipt).expect("serialize receipt");
        let pq_sig = pq_key.sign(&msg);
        let pq_public_key = pq_key.public_key();
        signed.pq_alg = Some("ML-DSA-87".to_string());
        signed.pq_key_id = Some(sha256_hex(&pq_public_key)[..16].to_string());
        signed.pq_public_key = Some(hex::encode(pq_public_key));
        signed.pq_sig = Some(hex::encode(pq_sig));
        signed
    }

    fn make_receipt() -> Receipt {
        Receipt {
            step_id: "step-abc".to_string(),
            action_id: "edit_note".to_string(),
            params: json!({ "id": "note-1", "category": "work" }),
            success: true,
            ts_ms: 1_700_000_000_000_u64,
            actor: None,
            prev_hash: None,
        }
    }

    fn make_receipt_with_actor() -> Receipt {
        Receipt {
            step_id: "step-xyz".to_string(),
            action_id: "delete_transaction".to_string(),
            params: json!({ "id": "txn-1" }),
            success: true,
            ts_ms: 1_700_000_000_500_u64,
            actor: Some(Actor { agent: "claude-desktop".to_string(), user: "alice".to_string() }),
            prev_hash: None,
        }
    }

    // ── round-trip test ────────────────────────────────────────────────────

    #[test]
    fn round_trip_ok() {
        let key = SigningKey::from_seed(&KEY_A);
        let receipt = make_receipt();
        let signed = sign_receipt(&key, &receipt);

        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        assert_eq!(outcome, Outcome::Ok, "round-trip signature must verify");
    }

    #[test]
    fn round_trip_with_actor_ok() {
        // An attributed receipt (R8) must re-derive byte-identically and verify.
        let key = SigningKey::from_seed(&KEY_A);
        let signed = sign_receipt(&key, &make_receipt_with_actor());
        let line = serde_json::to_string(&signed).unwrap();
        assert!(line.contains("\"actor\":{\"agent\":\"claude-desktop\",\"user\":\"alice\"}"));
        let (_, _, outcome) = verify_line(&line);
        assert_eq!(outcome, Outcome::Ok, "actor-bearing receipt must verify");
    }

    #[test]
    fn tampered_actor_fails() {
        // Swapping the operator after signing must invalidate the receipt — attribution
        // is inside the signed bytes, so it cannot be forged.
        let key = SigningKey::from_seed(&KEY_A);
        let signed = sign_receipt(&key, &make_receipt_with_actor());

        let mut obj: serde_json::Map<String, Value> =
            serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
        obj.insert("actor".to_string(), json!({ "agent": "claude-desktop", "user": "mallory" }));
        let line = serde_json::to_string(&obj).unwrap();

        let (_, _, outcome) = verify_line(&line);
        assert!(matches!(outcome, Outcome::Fail(_)), "tampered actor must not verify");
    }

    // ── tamper tests ───────────────────────────────────────────────────────

    #[test]
    fn tampered_params_fails() {
        let key = SigningKey::from_seed(&KEY_A);
        let receipt = make_receipt();
        let signed = sign_receipt(&key, &receipt);

        // Serialise to a JSON map, mutate params, re-serialise.
        let mut obj: serde_json::Map<String, Value> =
            serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
        obj.insert("params".to_string(), json!({ "id": "note-1", "category": "EVIL" }));
        let line = serde_json::to_string(&obj).unwrap();

        let (_, _, outcome) = verify_line(&line);
        assert!(
            matches!(outcome, Outcome::Fail(_)),
            "tampered params must not verify"
        );
    }

    #[test]
    fn tampered_success_fails() {
        let key = SigningKey::from_seed(&KEY_A);
        let receipt = make_receipt();
        let signed = sign_receipt(&key, &receipt);

        let mut obj: serde_json::Map<String, Value> =
            serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
        obj.insert("success".to_string(), Value::Bool(false));
        let line = serde_json::to_string(&obj).unwrap();

        let (_, _, outcome) = verify_line(&line);
        assert!(
            matches!(outcome, Outcome::Fail(_)),
            "tampered success must not verify"
        );
    }

    #[test]
    fn tampered_action_id_fails() {
        let key = SigningKey::from_seed(&KEY_A);
        let receipt = make_receipt();
        let signed = sign_receipt(&key, &receipt);

        let mut obj: serde_json::Map<String, Value> =
            serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
        obj.insert("action_id".to_string(), json!("delete_all_notes"));
        let line = serde_json::to_string(&obj).unwrap();

        let (_, _, outcome) = verify_line(&line);
        assert!(
            matches!(outcome, Outcome::Fail(_)),
            "tampered action_id must not verify"
        );
    }

    // ── parse-error path ───────────────────────────────────────────────────

    #[test]
    fn malformed_json_is_fail() {
        let (_, _, outcome) = verify_line("{not valid json}");
        assert!(
            matches!(outcome, Outcome::Fail(_)),
            "malformed JSON must produce Fail"
        );
    }

    // ── wrong key ──────────────────────────────────────────────────────────

    #[test]
    fn wrong_key_fails() {
        let signing_key = SigningKey::from_seed(&KEY_A);
        let other_key = SigningKey::from_seed(&KEY_B);

        let receipt = make_receipt();
        let mut signed = sign_receipt(&signing_key, &receipt);
        // Replace public key with a different, unrelated key.
        signed.public_key = hex::encode(other_key.public_key());

        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        assert!(
            matches!(outcome, Outcome::Fail(_)),
            "wrong public key must not verify"
        );
    }

    // ── hash chain (R20) ─────────────────────────────────────────────────────

    /// Sign a receipt with an explicit `prev_hash` set first — mirrors the host's chaining.
    fn sign_chained(key: &SigningKey, base: &Receipt, prev: Option<String>, step: &str) -> String {
        let mut r = base.clone();
        r.step_id = step.to_string();
        r.prev_hash = prev;
        serde_json::to_string(&sign_receipt(key, &r)).unwrap()
    }

    #[test]
    fn complete_chain_verifies_and_deletion_is_caught() {
        let key = SigningKey::from_seed(&KEY_A);
        let base = make_receipt();
        // A 3-receipt chain: each prev_hash = SHA-256 of the previous LINE.
        let l1 = sign_chained(&key, &base, None, "s1");
        let l2 = sign_chained(&key, &base, Some(sha256_hex(l1.as_bytes())), "s2");
        let l3 = sign_chained(&key, &base, Some(sha256_hex(l2.as_bytes())), "s3");

        // Intact: every signature verifies and the chain is unbroken.
        let intact = format!("{l1}\n{l2}\n{l3}\n");
        assert_eq!(verify_log(&intact), (3, 0, 0));

        // Delete the MIDDLE receipt: the survivors are unaltered (sigs still pass), but l3's
        // prev_hash no longer matches l1 → the deletion is caught as a chain break.
        let with_gap = format!("{l1}\n{l3}\n");
        let (ok, fail, breaks) = verify_log(&with_gap);
        assert_eq!((ok, fail), (2, 0), "remaining receipts still verify");
        assert_eq!(breaks, 1, "whole-receipt deletion must break the chain");

        // Truncate the HEAD: the new first line claims a predecessor that is gone.
        let head_cut = format!("{l2}\n{l3}\n");
        assert_eq!(verify_log(&head_cut).2, 1, "head truncation must break the chain");
    }

    // ── retention epoch-checkpoint (doc 24 §6-P2) ────────────────────────────────────────────────

    #[test]
    fn retention_checkpoint_at_head_is_a_sealed_chain_point_not_a_break() {
        let key = SigningKey::from_seed(&KEY_A);
        // The pruned prefix (two receipts) — present only to compute the prior head hash H.
        let base = make_receipt();
        let old1 = sign_chained(&key, &base, None, "old1");
        let old2 = sign_chained(&key, &base, Some(sha256_hex(old1.as_bytes())), "old2");
        let prior_head = sha256_hex(old2.as_bytes()); // H = hash of the last pruned line

        // The checkpoint seals to H; its params record prior_head_hash = H.
        let cp = Receipt {
            step_id: "checkpoint".into(),
            action_id: "kriya.retention.checkpoint".into(),
            params: json!({
                "policy": "io-30d",
                "prior_head_hash": prior_head,
                "pruned_before_ts_ms": 1_000_u64,
                "pruned_count": 2
            }),
            success: true,
            ts_ms: 2_000,
            actor: None,
            prev_hash: Some(prior_head.clone()),
        };
        let cp_line = serde_json::to_string(&sign_receipt(&key, &cp)).unwrap();

        // A kriya.io.* receipt re-chained onto the checkpoint.
        let io = Receipt {
            step_id: "io1".into(),
            action_id: "kriya.io.egress.mcp.allow".into(),
            params: json!({
                "decision": "allow",
                "dest_host": "api.vendor.com",
                "hash_scheme": "wire-bytes"
            }),
            success: true,
            ts_ms: 3_000,
            actor: None,
            prev_hash: Some(sha256_hex(cp_line.as_bytes())),
        };
        let io_line = serde_json::to_string(&sign_receipt(&key, &io)).unwrap();

        // The SEALED log: checkpoint (head) + the retained kriya.io.* receipt. Both verify; the
        // checkpoint seal is NOT counted as a truncation break.
        let sealed = format!("{cp_line}\n{io_line}\n");
        let (ok, fail, breaks) = verify_log(&sealed);
        assert_eq!((ok, fail), (2, 0), "checkpoint + kriya.io receipt both verify");
        assert_eq!(
            breaks, 0,
            "a retention checkpoint at the head is a sealed chain point, not a chain break"
        );

        // A NON-checkpoint receipt at the head with a dangling prev_hash is still a truncation break.
        let bogus = sign_chained(&key, &base, Some(prior_head), "bogus");
        assert_eq!(
            verify_log(&format!("{bogus}\n")).2,
            1,
            "only a retention checkpoint earns the seal exemption"
        );
    }

    // ── A5 (design docs/design/a5-pq-dual-sig.md §5 verification matrix) ────────────────────

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn dual_signed_receipt_verifies_row2() {
        let key = SigningKey::from_seed(&KEY_A);
        let (_seed, pq_key) = kriya::crypto::PqSigningKey::generate();
        let signed = sign_receipt_dual(&key, &pq_key, &make_receipt());
        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        assert_eq!(outcome, Outcome::Ok, "a valid dual-signed receipt must verify (row 2)");
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn pq_tampered_sig_fails_distinctly_row3() {
        let key = SigningKey::from_seed(&KEY_A);
        let (_seed, pq_key) = kriya::crypto::PqSigningKey::generate();
        let mut signed = sign_receipt_dual(&key, &pq_key, &make_receipt());
        signed.pq_sig = Some("00".repeat(4627));
        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        match outcome {
            Outcome::Fail(reason) => assert!(
                reason.contains("pq_sig") && reason.contains("does not match"),
                "unexpected reason: {reason}"
            ),
            Outcome::Ok => panic!("a tampered PQ signature must not verify"),
        }
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn ed25519_tamper_fails_even_with_valid_pq_row4() {
        let key = SigningKey::from_seed(&KEY_A);
        let (_seed, pq_key) = kriya::crypto::PqSigningKey::generate();
        let signed = sign_receipt_dual(&key, &pq_key, &make_receipt());
        let mut obj: serde_json::Map<String, Value> =
            serde_json::from_str(&serde_json::to_string(&signed).unwrap()).unwrap();
        obj.insert("action_id".to_string(), json!("something-else"));
        let line = serde_json::to_string(&obj).unwrap();
        let (_, _, outcome) = verify_line(&line);
        match outcome {
            Outcome::Fail(reason) => assert!(
                reason.contains("signature does not match receipt"),
                "unexpected reason: {reason}"
            ),
            Outcome::Ok => panic!("an Ed25519 tamper must fail regardless of PQ presence"),
        }
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn incomplete_pq_set_fails_distinctly_row5() {
        let key = SigningKey::from_seed(&KEY_A);
        let (_seed, pq_key) = kriya::crypto::PqSigningKey::generate();
        let mut signed = sign_receipt_dual(&key, &pq_key, &make_receipt());
        signed.pq_sig = None; // strip one of the four required siblings
        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        match outcome {
            Outcome::Fail(reason) => assert!(
                reason.contains("incomplete or inconsistent PQ signature") && reason.contains("pq_sig"),
                "unexpected reason: {reason}"
            ),
            Outcome::Ok => panic!("a partial PQ set must not verify"),
        }
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn mismatched_pq_key_id_fails_distinctly_row5() {
        let key = SigningKey::from_seed(&KEY_A);
        let (_seed, pq_key) = kriya::crypto::PqSigningKey::generate();
        let mut signed = sign_receipt_dual(&key, &pq_key, &make_receipt());
        signed.pq_key_id = Some("0".repeat(16));
        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        match outcome {
            Outcome::Fail(reason) => assert!(
                reason.contains("incomplete or inconsistent PQ signature") && reason.contains("pq_key_id"),
                "unexpected reason: {reason}"
            ),
            Outcome::Ok => panic!("a pq_key_id that doesn't hash-match must not verify"),
        }
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn unknown_pq_alg_fails_distinctly_row6() {
        let key = SigningKey::from_seed(&KEY_A);
        let (_seed, pq_key) = kriya::crypto::PqSigningKey::generate();
        let mut signed = sign_receipt_dual(&key, &pq_key, &make_receipt());
        signed.pq_alg = Some("ML-DSA-44".to_string());
        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        match outcome {
            Outcome::Fail(reason) => assert!(
                reason.contains("unsupported pq_alg"),
                "unexpected reason: {reason}"
            ),
            Outcome::Ok => panic!("an unsupported pq_alg must not verify"),
        }
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn ed25519_only_receipt_still_verifies_unchanged_row1() {
        // No pq_crypto build awareness needed on the SIGNING side — a receipt with no pq_*
        // material at all must verify identically whether or not this binary was built with
        // pq-crypto (frozen-schema / interop — design RT2.1, acceptance #1).
        let key = SigningKey::from_seed(&KEY_A);
        let signed = sign_receipt(&key, &make_receipt());
        let line = serde_json::to_string(&signed).unwrap();
        assert!(!line.contains("pq_"));
        let (_, _, outcome) = verify_line(&line);
        assert_eq!(outcome, Outcome::Ok);
    }

    /// A5 (design D7): cross-implementation parity. `aws-lc-rs` (this crate's production PQ
    /// signer, via `kriya::crypto::PqSigningKey`) signs; RustCrypto's `ml-dsa` (test-only,
    /// unaudited — V4) independently verifies. And vice-versa. Two independent ML-DSA-87
    /// implementations agreeing is the strongest cross-impl assurance available without a JS
    /// build of aws-lc (design D7's rationale for why this lives here, Rust-side, not in TS).
    #[cfg(feature = "pq-crypto")]
    #[test]
    fn cross_impl_parity_aws_lc_signs_rustcrypto_verifies() {
        use ml_dsa::{EncodedVerifyingKey, MlDsa87, Verifier as _, VerifyingKey};

        let (_seed, pq_key) = kriya::crypto::PqSigningKey::generate();
        let pk_bytes = pq_key.public_key();
        let msg = b"kriya A5 cross-impl parity fixture";
        let sig_bytes = pq_key.sign(msg);

        // aws-lc-rs verifies its own signature (sanity).
        assert!(kriya::crypto::pq_verify(&pk_bytes, msg, &sig_bytes));

        // RustCrypto independently re-verifies the SAME signature over the SAME message under
        // the SAME public key.
        let encoded_vk = EncodedVerifyingKey::<MlDsa87>::try_from(pk_bytes.as_slice())
            .expect("aws-lc-rs public key decodes as a valid RustCrypto ML-DSA-87 verifying key");
        let vk = VerifyingKey::<MlDsa87>::decode(&encoded_vk);
        let sig = ml_dsa::Signature::<MlDsa87>::try_from(sig_bytes.as_slice())
            .expect("aws-lc-rs signature decodes as a valid RustCrypto ML-DSA-87 signature");
        assert!(
            vk.verify(msg, &sig).is_ok(),
            "RustCrypto must independently verify an aws-lc-rs-produced ML-DSA-87 signature"
        );
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn cross_impl_parity_rustcrypto_signs_aws_lc_verifies() {
        use ml_dsa::{Generate, Keypair as _, MlDsa87, Signer as _, SigningKey};

        // `Generate` uses the OS RNG under ml-dsa's default `getrandom` feature.
        let sk = SigningKey::<MlDsa87>::generate();
        let msg = b"kriya A5 cross-impl parity fixture, reverse direction";
        let sig = sk.sign(msg);

        let pk_bytes: Vec<u8> = sk.verifying_key().encode().as_slice().to_vec();
        let sig_bytes: Vec<u8> = sig.encode().as_slice().to_vec();

        assert!(
            kriya::crypto::pq_verify(&pk_bytes, msg, &sig_bytes),
            "aws-lc-rs must independently verify a RustCrypto-produced ML-DSA-87 signature"
        );
    }
}
