//! F-4 parity fixture (kriya-console doc 31 §3.6 / doc 33 §4-A): a small, DETERMINISTIC chain of
//! `kriya.pay.{intent,decision,outcome}` receipts — the purchase-receipt chain the payment gate
//! emits — that the kriya-console suite imports to prove its TS verifier re-derives the runtime's
//! bytes byte-identically and folds them into PurchaseChains. Two purchases: one complete +
//! executed (intent→decision→outcome), one HELD with an UNKNOWN amount (intent→decision, no
//! outcome — the honest 2/3 shape). Signed with the same FIXED RFC 8032 test-vector key as
//! `gates_fixture.rs` so the committed fixture is stable across runs and machines.
//!
//! Content-free by construction: amount (minor units) / currency / merchant-host only — NEVER a PAN
//! (custody stays in the EG-B credential broker); `amount_known: false` is carried verbatim rather
//! than a guessed number.

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer};
use serde_json::{json, Value};

// RFC 8032 Ed25519 test-vector seed — a PUBLIC, well-known key: synthetic evidence, never a real
// signing identity (identical to gates_fixture.rs).
const SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

fn receipt(step_id: &str, action_id: &str, params: Value, success: bool, ts: u128, actor: &Actor) -> Receipt {
    Receipt::new(step_id.into(), action_id.into(), params, success, ts).with_actor(Some(actor.clone()))
}

#[test]
fn generates_and_verifies_the_kriya_pay_parity_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-pay-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");
    let actor = Actor::new("claude-code", "platform-eng");

    // Purchase 1 — a complete, approved, executed $42.00 Stripe payment.
    signer.record(receipt(
        "pay-stripe-1",
        "kriya.pay.intent",
        json!({
            "v": 1, "pay_id": "pay-stripe-1", "merchant": "api.stripe.com",
            "amount_minor": 4200, "currency": "usd", "amount_known": true,
            "tool": "mcp__stripe__create_payment_intent",
            "action_id": "claude-code__mcp__stripe__create_payment_intent"
        }),
        true,
        1_700_000_200_000,
        &actor,
    ));
    signer.record(receipt(
        "pay-stripe-1-decision",
        "kriya.pay.decision",
        json!({
            "v": 1, "pay_id": "pay-stripe-1", "decision": "approved",
            "cap_minor": 50000, "day_spent_minor": 12000, "matched_rule": "pay-server",
            "approver": "platform-eng", "corr_step": "pay-stripe-1"
        }),
        true,
        1_700_000_200_001,
        &actor,
    ));
    signer.record(receipt(
        "pay-stripe-1-outcome",
        "kriya.pay.outcome",
        json!({
            "v": 1, "pay_id": "pay-stripe-1", "result": "executed",
            "status": "200", "corr_step": "pay-stripe-1"
        }),
        true,
        1_700_000_200_002,
        &actor,
    ));

    // Purchase 2 — a HELD payment on a non-payment-server checkout tool whose amount could not be
    // parsed: intent (amount_known:false) → decision (held), no outcome (the honest 2/3 shape).
    signer.record(receipt(
        "pay-checkout-2",
        "kriya.pay.intent",
        json!({
            "v": 1, "pay_id": "pay-checkout-2", "merchant": "checkout.acme.example",
            "amount_known": false, "tool": "mcp__acme__checkout_cart",
            "action_id": "claude-code__mcp__acme__checkout_cart"
        }),
        true,
        1_700_000_200_003,
        &actor,
    ));
    signer.record(receipt(
        "pay-checkout-2-decision",
        "kriya.pay.decision",
        json!({
            "v": 1, "pay_id": "pay-checkout-2", "decision": "held",
            "cap_minor": 100000, "matched_rule": "pay-tool", "corr_step": "pay-checkout-2"
        }),
        false,
        1_700_000_200_004,
        &actor,
    ));

    let generated = std::fs::read_to_string(&log).unwrap();

    // Verify: every signature valid + the chain contiguous, both purchases + all three verbs present.
    let lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 5, "fixture line count");
    let mut prev: Option<String> = None;
    let mut verbs = Vec::new();
    for line in &lines {
        let v: Value = serde_json::from_str(line).unwrap();
        let pk: [u8; 32] = hex::decode(v["public_key"].as_str().unwrap()).unwrap().try_into().unwrap();
        let sig: [u8; 64] = hex::decode(v["signature"].as_str().unwrap()).unwrap().try_into().unwrap();
        let key = VerifyingKey::from_bytes(&pk).unwrap();
        assert!(
            key.verify(&signed_bytes(&v), &Signature::from_bytes(&sig)).is_ok(),
            "fixture line must verify: {}",
            v["action_id"]
        );
        assert_eq!(
            v.get("prev_hash").and_then(Value::as_str).map(str::to_string),
            prev,
            "chain contiguous"
        );
        prev = Some(sha256_hex(line.as_bytes()));
        let action = v["action_id"].as_str().unwrap();
        if let Some(rest) = action.strip_prefix("kriya.pay.") {
            verbs.push(rest.to_string());
            assert_eq!(v["params"]["v"], 1, "pay receipts carry the schema-version marker");
            assert!(v["params"]["pay_id"].is_string(), "pay receipts carry the pay_id correlation key");
        }
    }
    verbs.sort();
    assert_eq!(
        verbs,
        vec!["decision", "decision", "intent", "intent", "outcome"],
        "both purchases + all three verbs of the kriya.pay.* vocabulary are present"
    );

    // Land the fixture for the console suite (deterministic → no churn after first commit).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pay_ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reproduce audit.rs's signed bytes (mirrors gates_fixture.rs exactly).
fn signed_bytes(v: &Value) -> Vec<u8> {
    let mut s = String::from("{");
    s.push_str(&format!("\"step_id\":{}", v["step_id"]));
    s.push_str(&format!(",\"action_id\":{}", v["action_id"]));
    s.push_str(&format!(",\"params\":{}", serde_json::to_string(&canonical(&v["params"])).unwrap()));
    s.push_str(&format!(",\"success\":{}", v["success"]));
    s.push_str(&format!(",\"ts_ms\":{}", v["ts_ms"]));
    if let Some(a) = v.get("actor") {
        if !a.is_null() {
            s.push_str(&format!(",\"actor\":{{\"agent\":{},\"user\":{}}}", a["agent"], a["user"]));
        }
    }
    if let Some(p) = v.get("prev_hash") {
        if !p.is_null() {
            s.push_str(&format!(",\"prev_hash\":{p}"));
        }
    }
    s.push('}');
    s.into_bytes()
}

fn canonical(v: &Value) -> Value {
    v.clone()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
