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
    // `serde_json::to_vec` of the unsigned Receipt struct (no preserve_order;
    // struct fields serialize in declaration order: step_id, action_id, params,
    // success, ts_ms).
    let msg = match serde_json::to_vec(&signed.receipt) {
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

    let mut ok_count: u32 = 0;
    let mut fail_count: u32 = 0;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
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
    }

    println!("verified {ok_count}, failed {fail_count}");

    if fail_count > 0 {
        process::exit(1);
    }
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
}
