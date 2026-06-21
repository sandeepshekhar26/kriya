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

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
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

    let verifying_key = match VerifyingKey::from_bytes(&pub_bytes) {
        Ok(k) => k,
        Err(e) => {
            return (
                action_id,
                step_id,
                Outcome::Fail(format!("bad verifying key: {e}")),
            );
        }
    };

    let signature = Signature::from_bytes(&sig_bytes);

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

    // ── verify ─────────────────────────────────────────────────────────────
    match verifying_key.verify(&msg, &signature) {
        Ok(()) => (action_id, step_id, Outcome::Ok),
        Err(e) => (action_id, step_id, Outcome::Fail(format!("bad signature: {e}"))),
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
            match (&signed.receipt.prev_hash, &prev_line_hash) {
                (Some(claimed), Some(actual)) if claimed != actual => {
                    println!("CHAIN-BREAK {action_id} {step_id}  [prev_hash != previous line — a preceding receipt was deleted, reordered, or altered]");
                    chain_breaks += 1;
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
    use ed25519_dalek::Signer as _;
    use ed25519_dalek::SigningKey;
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
        let signature = hex::encode(key.sign(&msg).to_bytes());
        let public_key = hex::encode(key.verifying_key().to_bytes());
        SignedReceipt {
            receipt: receipt.clone(),
            public_key,
            signature,
        }
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
        let key = SigningKey::from_bytes(&KEY_A);
        let receipt = make_receipt();
        let signed = sign_receipt(&key, &receipt);

        let line = serde_json::to_string(&signed).unwrap();
        let (_, _, outcome) = verify_line(&line);
        assert_eq!(outcome, Outcome::Ok, "round-trip signature must verify");
    }

    #[test]
    fn round_trip_with_actor_ok() {
        // An attributed receipt (R8) must re-derive byte-identically and verify.
        let key = SigningKey::from_bytes(&KEY_A);
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
        let key = SigningKey::from_bytes(&KEY_A);
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
        let key = SigningKey::from_bytes(&KEY_A);
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
        let key = SigningKey::from_bytes(&KEY_A);
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
        let key = SigningKey::from_bytes(&KEY_A);
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
        let signing_key = SigningKey::from_bytes(&KEY_A);
        let other_key = SigningKey::from_bytes(&KEY_B);

        let receipt = make_receipt();
        let mut signed = sign_receipt(&signing_key, &receipt);
        // Replace public key with a different, unrelated key.
        signed.public_key = hex::encode(other_key.verifying_key().to_bytes());

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
        let key = SigningKey::from_bytes(&KEY_A);
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
}
