//! F-5 parity fixture (kriya-console doc 31 §3.5 / doc 33 §4-B): a small, DETERMINISTIC chain of
//! `kriya.run.launched` receipts — the launch attestations the governed launcher (`kriya-run`)
//! emits — that the kriya-console suite imports to prove its TS verifier re-derives the runtime's
//! bytes byte-identically and folds them into launch rows (pack chip on the session). Two launches:
//! Claude Code under the Developer pack with the egress lane, and an unattended Cron run under the
//! Planner pack that FORCES the shift flag (the 2/2 shape of the launcher's two agent kinds).
//! Signed with the same FIXED RFC 8032 test-vector key as `pay_fixture.rs`/`gates_fixture.rs` so the
//! committed fixture is stable across runs and machines.
//!
//! Content-free by construction: `{v, agent, pack, lanes, shift}` only — never the agent command's
//! argv or the working directory (which could carry content).

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer};
use serde_json::{json, Value};

// RFC 8032 Ed25519 test-vector seed — a PUBLIC, well-known key: synthetic evidence, never a real
// signing identity (identical to pay_fixture.rs / gates_fixture.rs).
const SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

fn receipt(step_id: &str, params: Value, ts: u128, actor: &Actor) -> Receipt {
    Receipt::new(step_id.into(), "kriya.run.launched".into(), params, true, ts)
        .with_actor(Some(actor.clone()))
}

#[test]
fn generates_and_verifies_the_kriya_run_launched_parity_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-run-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");

    // Launch 1 — Claude Code under the Developer pack, egress lane governed, interactive (no shift).
    let cc = Actor::new("claude-code", "platform-eng");
    signer.record(receipt(
        "run-launch-1",
        json!({
            "v": 1, "agent": "claude-code", "pack": "developer",
            "lanes": ["egress"], "shift": false
        }),
        1_700_000_300_000,
        &cc,
    ));

    // Launch 2 — an unattended Cron run under the Planner pack: shift reporting is FORCED on, and
    // both lanes are governed (egress + OS containment).
    let cron = Actor::new("cron", "platform-eng");
    signer.record(receipt(
        "run-launch-2",
        json!({
            "v": 1, "agent": "cron", "pack": "planner",
            "lanes": ["egress", "contain"], "shift": true
        }),
        1_700_000_400_000,
        &cron,
    ));

    let generated = std::fs::read_to_string(&log).unwrap();

    // Verify: every signature valid + the chain contiguous, both launches present + content-free.
    let lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "fixture line count");
    let mut prev: Option<String> = None;
    for line in &lines {
        let v: Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["action_id"], "kriya.run.launched");
        let pk: [u8; 32] = hex::decode(v["public_key"].as_str().unwrap()).unwrap().try_into().unwrap();
        let sig: [u8; 64] = hex::decode(v["signature"].as_str().unwrap()).unwrap().try_into().unwrap();
        let vk = VerifyingKey::from_bytes(&pk).unwrap();
        assert!(
            vk.verify(&signed_bytes(&v), &Signature::from_bytes(&sig)).is_ok(),
            "fixture line must verify: {}",
            v["step_id"]
        );
        assert_eq!(
            v.get("prev_hash").and_then(Value::as_str).map(str::to_string),
            prev,
            "chain contiguous"
        );
        prev = Some(sha256_hex(line.as_bytes()));
        assert_eq!(v["params"]["v"], 1, "launch receipts carry the schema-version marker");
        assert!(v["params"]["pack"].is_string(), "launch receipts carry the pack id");
        assert!(v["params"]["lanes"].is_array(), "launch receipts carry the lanes array");
        // Never any command content.
        assert!(!line.contains("claude "), "no agent argv in the receipt");
        assert!(!line.contains("/"), "no filesystem path in the receipt");
    }

    // Land the fixture for the console suite (deterministic → no churn after first commit).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/run_launched_ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reproduce audit.rs's signed bytes (mirrors pay_fixture.rs / gates_fixture.rs exactly — the
/// top-level fields in the `Receipt` struct's declaration order, params re-serialized as-is).
fn signed_bytes(v: &Value) -> Vec<u8> {
    let mut s = String::from("{");
    s.push_str(&format!("\"step_id\":{}", v["step_id"]));
    s.push_str(&format!(",\"action_id\":{}", v["action_id"]));
    s.push_str(&format!(",\"params\":{}", serde_json::to_string(&v["params"]).unwrap()));
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

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
