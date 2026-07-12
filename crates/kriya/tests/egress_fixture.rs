//! EG-2 parity fixture: a small, DETERMINISTIC chain of action + `kriya.io.*` receipts that the
//! kriya-console suite (EG-3) imports to prove its TS verifier re-derives the runtime's bytes
//! byte-identically (doc 24 §8 test strategy). Signed with a FIXED key (an RFC 8032 test vector) so
//! the committed fixture is stable across runs and machines. The test regenerates the fixture (it is
//! deterministic — same key + same content → same signatures + chain) and verifies every line's
//! signature and the hash chain, exactly as `kriya-verify` / the released `kriya-audit` do.

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer};
use serde_json::{json, Value};

// RFC 8032 Ed25519 test-vector seed — a PUBLIC, well-known key: this fixture is synthetic evidence,
// never a real signing identity.
const SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

fn io_receipt(action_id: &str, params: Value, success: bool, ts: u128, actor: &Actor) -> Receipt {
    Receipt::new(
        format!("{action_id}-{ts}"),
        action_id.to_string(),
        params,
        success,
        ts,
    )
    .with_actor(Some(actor.clone()))
}

#[test]
fn generates_and_verifies_the_kriya_io_parity_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");
    let actor = Actor::new("claude-code", "platform-eng");

    // A representative governed session across the vocabulary: an allowed broker MCP egress (with a
    // corr-joined action receipt), a decision-point deny (no action receipt), an approval-gated
    // egress, a hook-lane WebFetch egress, and a keyed ingress digest.
    signer.record(
        Receipt::new(
            "act-widgets-list".into(),
            "widgets__list_widgets".into(),
            json!({ "q": "gaskets" }),
            true,
            1_700_000_000_000,
        )
        .with_actor(Some(actor.clone())),
    );
    signer.record(io_receipt(
        "kriya.io.egress.mcp.allow",
        json!({
            "corr": "act-widgets-list", "dest_host": "api.vendor.com", "dest_kind": "mcp",
            "server": "widgets", "method": "tools/call", "bytes_out": 120, "bytes_in": 340,
            "content_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "hash_scheme": "wire-bytes", "policy_rule": "*.vendor.com", "decision": "allow"
        }),
        true,
        1_700_000_000_001,
        &actor,
    ));
    signer.record(io_receipt(
        "kriya.io.egress.mcp.deny",
        json!({
            "dest_host": "unlisted.example", "dest_kind": "mcp", "server": "shadow",
            "hash_scheme": "wire-bytes", "decision": "deny",
            "reason": "egress to unlisted.example is not on the allowlist (deny-by-default)"
        }),
        false,
        1_700_000_000_002,
        &actor,
    ));
    signer.record(io_receipt(
        "kriya.io.egress.mcp.approve",
        json!({
            "corr": "act-partner", "dest_host": "api.partner.com", "dest_kind": "mcp",
            "method": "tools/call", "bytes_out": 88, "bytes_in": 210,
            "content_sha256": "2222222222222222222222222222222222222222222222222222222222222222",
            "hash_scheme": "wire-bytes", "policy_rule": "api.partner.com",
            "approved_by": "platform-eng", "decision": "approve"
        }),
        true,
        1_700_000_000_003,
        &actor,
    ));
    signer.record(io_receipt(
        "kriya.io.egress.http.allow",
        json!({
            "corr": "act-webfetch", "dest_host": "docs.example.org", "dest_kind": "http",
            "bytes_out": 64,
            "content_sha256": "3333333333333333333333333333333333333333333333333333333333333333",
            "hash_scheme": "canonical-json", "decision": "allow"
        }),
        true,
        1_700_000_000_004,
        &actor,
    ));
    signer.record(io_receipt(
        "kriya.io.ingress.http.allow",
        json!({
            "corr": "act-webfetch", "dest_kind": "http", "bytes_in": 4096,
            "content_sha256": "4444444444444444444444444444444444444444444444444444444444444444",
            "hash_scheme": "canonical-json", "decision": "allow"
        }),
        true,
        1_700_000_000_005,
        &actor,
    ));

    let generated = std::fs::read_to_string(&log).unwrap();

    // Verify: every signature valid + the chain contiguous, and every kriya.io.* facet present.
    let lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 6, "fixture line count");
    let mut prev: Option<String> = None;
    let mut io_count = 0;
    for line in &lines {
        let v: Value = serde_json::from_str(line).unwrap();
        let pk: [u8; 32] = hex::decode(v["public_key"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let sig: [u8; 64] = hex::decode(v["signature"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let key = VerifyingKey::from_bytes(&pk).unwrap();
        assert!(
            key.verify(&signed_bytes(&v), &Signature::from_bytes(&sig))
                .is_ok(),
            "fixture line must verify: {}",
            v["action_id"]
        );
        assert_eq!(
            v.get("prev_hash")
                .and_then(Value::as_str)
                .map(str::to_string),
            prev,
            "chain contiguous"
        );
        prev = Some(sha256_hex(line.as_bytes()));
        if v["action_id"].as_str().unwrap().starts_with("kriya.io.") {
            io_count += 1;
            assert!(
                v["params"].get("hash_scheme").is_some(),
                "every kriya.io.* receipt carries hash_scheme"
            );
        }
    }
    assert_eq!(io_count, 5, "five kriya.io.* receipts");

    // Land the fixture for the console suite (deterministic → no churn after first commit).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/egress_ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reproduce audit.rs's signed bytes: `Receipt` fields in declaration order, `params`/`actor`
/// key-sorted, optional fields omitted when absent.
fn signed_bytes(v: &Value) -> Vec<u8> {
    let mut s = String::from("{");
    s.push_str(&format!("\"step_id\":{}", v["step_id"]));
    s.push_str(&format!(",\"action_id\":{}", v["action_id"]));
    s.push_str(&format!(
        ",\"params\":{}",
        serde_json::to_string(&canonical(&v["params"])).unwrap()
    ));
    s.push_str(&format!(",\"success\":{}", v["success"]));
    s.push_str(&format!(",\"ts_ms\":{}", v["ts_ms"]));
    if let Some(a) = v.get("actor") {
        if !a.is_null() {
            s.push_str(&format!(
                ",\"actor\":{}",
                serde_json::to_string(&canonical(a)).unwrap()
            ));
        }
    }
    if let Some(ph) = v.get("prev_hash") {
        if !ph.is_null() {
            s.push_str(&format!(",\"prev_hash\":{ph}"));
        }
    }
    s.push('}');
    s.into_bytes()
}

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonical(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
