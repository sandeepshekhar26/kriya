//! O-3 parity fixture (kriya-console doc 32 §4 O-3 / doc 33 §5.5): a small, DETERMINISTIC run of
//! action receipts — two carrying the additive `kriya.dur.ms` / `kriya.dur.basis` params the hook
//! stamps from its pre→post marker, one WITHOUT them (the honest-absence shape) — that the
//! kriya-console suite imports to prove its TS verifier re-derives the runtime's bytes
//! byte-identically and its session-tree fold reads the duration back. Signed with the same FIXED
//! RFC 8032 test-vector key as `pay_fixture.rs`/`gates_fixture.rs`, so the committed fixture is
//! stable across runs and machines.
//!
//! The duration params are ADDITIVE and OPTIONAL: every receipt here is an ordinary
//! `claude-code__*` action receipt on the frozen envelope — a verifier with no duration awareness
//! accepts all three lines exactly as before. Each carries `kriya.corr.run_id` so the Console groups
//! them into one session tree (the waterfall's input).

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer};
use serde_json::{json, Value};

// RFC 8032 Ed25519 test-vector seed — a PUBLIC, well-known key: synthetic evidence, never a real
// signing identity (identical to pay_fixture.rs).
const SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

fn receipt(step_id: &str, action_id: &str, params: Value, success: bool, ts: u128, actor: &Actor) -> Receipt {
    Receipt::new(step_id.into(), action_id.into(), params, success, ts).with_actor(Some(actor.clone()))
}

#[test]
fn generates_and_verifies_the_kriya_duration_parity_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-dur-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");
    let actor = Actor::new("claude-code", "platform-eng");

    // Action 1 — a fast Read: 120 ms, measured hook-pre-post.
    signer.record(receipt(
        "dur-read-1",
        "claude-code__read",
        json!({
            "file_path": "README.md",
            "kriya.dur.ms": 120,
            "kriya.dur.basis": "hook-pre-post",
            "kriya.corr": { "run_id": "run-dur-1" }
        }),
        true,
        1_700_000_300_000,
        &actor,
    ));
    // Action 2 — a long Bash (a test run): 4300 ms, measured hook-pre-post.
    signer.record(receipt(
        "dur-bash-2",
        "claude-code__bash",
        json!({
            "command": "cargo test",
            "kriya.dur.ms": 4300,
            "kriya.dur.basis": "hook-pre-post",
            "kriya.corr": { "run_id": "run-dur-1" }
        }),
        true,
        1_700_000_300_500,
        &actor,
    ));
    // Action 3 — a WebFetch whose pre marker never resolved: NO duration params (honest absence,
    // never a fabricated 0 ms). The waterfall renders this as an instant marker, not a bar.
    signer.record(receipt(
        "dur-webfetch-3",
        "claude-code__webfetch",
        json!({
            "url": "https://example.com",
            "kriya.corr": { "run_id": "run-dur-1" }
        }),
        true,
        1_700_000_303_000,
        &actor,
    ));

    let generated = std::fs::read_to_string(&log).unwrap();

    // Verify: every signature valid + the chain contiguous; the two duration shapes both present.
    let lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 3, "fixture line count");
    let mut prev: Option<String> = None;
    let mut with_dur = 0;
    let mut without_dur = 0;
    for line in &lines {
        let v: Value = serde_json::from_str(line).unwrap();
        let pk: [u8; 32] = hex::decode(v["public_key"].as_str().unwrap()).unwrap().try_into().unwrap();
        let sig: [u8; 64] = hex::decode(v["signature"].as_str().unwrap()).unwrap().try_into().unwrap();
        let vk = VerifyingKey::from_bytes(&pk).unwrap();
        assert!(
            vk.verify(&signed_bytes(&v), &Signature::from_bytes(&sig)).is_ok(),
            "fixture line must verify: {}",
            v["action_id"]
        );
        assert_eq!(
            v.get("prev_hash").and_then(Value::as_str).map(str::to_string),
            prev,
            "chain contiguous"
        );
        prev = Some(sha256_hex(line.as_bytes()));

        assert_eq!(
            v["params"]["kriya.corr"]["run_id"], "run-dur-1",
            "every action carries the run correlation the Console groups on"
        );
        match v["params"].get("kriya.dur.ms") {
            Some(ms) => {
                assert!(ms.is_u64(), "duration is a u64 millisecond count");
                assert_eq!(v["params"]["kriya.dur.basis"], "hook-pre-post");
                with_dur += 1;
            }
            None => {
                assert!(
                    v["params"].get("kriya.dur.basis").is_none(),
                    "no orphan basis without a duration"
                );
                without_dur += 1;
            }
        }
    }
    assert_eq!(with_dur, 2, "two measured actions");
    assert_eq!(without_dur, 1, "one honest-absence action");

    // Land the fixture for the console suite (deterministic → no churn after first commit).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/duration_ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reproduce audit.rs's signed bytes (mirrors pay_fixture.rs exactly).
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
