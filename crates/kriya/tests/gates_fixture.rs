//! F-2 parity fixture (kriya-console doc 31 §3.3 / docs/ideas/design/F2-gates.md §5): a small,
//! DETERMINISTIC chain of action + `kriya.gate.<class>.*` receipts that the kriya-console suite
//! imports to prove its TS verifier re-derives the runtime's bytes byte-identically — one receipt
//! per verb of the new vocabulary ({evaluated, held, approved, denied}), with the held/denied
//! receipts corr_step-linked to their blocked attempts' own action receipts, exactly as
//! `kriya-hook` records them. Signed with a FIXED key (an RFC 8032 test vector) so the committed
//! fixture is stable across runs and machines. Mirrors `egress_fixture.rs`.

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer};
use serde_json::{json, Value};

// RFC 8032 Ed25519 test-vector seed — a PUBLIC, well-known key: this fixture is synthetic evidence,
// never a real signing identity.
const SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

fn receipt(step_id: &str, action_id: &str, params: Value, success: bool, ts: u128, actor: &Actor) -> Receipt {
    Receipt::new(step_id.into(), action_id.into(), params, success, ts).with_actor(Some(actor.clone()))
}

#[test]
fn generates_and_verifies_the_kriya_gate_parity_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-gates-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");
    let actor = Actor::new("claude-code", "platform-eng");

    // 1. A held publish: the blocked attempt's action receipt, then the held gate receipt.
    signer.record(receipt(
        "act-npm-publish",
        "claude-code__bash",
        json!({ "command": "npm publish --access public" }),
        false,
        1_700_000_100_000,
        &actor,
    ));
    signer.record(receipt(
        "gate-publish-held",
        "kriya.gate.publish.held",
        json!({
            "v": 1, "class": "publish", "rule_id": "npm-publish", "tier": "approve",
            "matcher_kind": "command", "action_id": "claude-code__bash",
            "corr_step": "act-npm-publish"
        }),
        false,
        1_700_000_100_001,
        &actor,
    ));

    // 2. An approved deploy (the human granted it; the action itself proceeds post-hook).
    signer.record(receipt(
        "gate-deploy-approved",
        "kriya.gate.deploy.approved",
        json!({
            "v": 1, "class": "deploy", "rule_id": "fly-deploy", "tier": "approve",
            "matcher_kind": "command", "action_id": "claude-code__bash"
        }),
        true,
        1_700_000_100_002,
        &actor,
    ));

    // 3. A receipt-tier destructive-git evaluation (recorded, not blocked).
    signer.record(receipt(
        "gate-git-evaluated",
        "kriya.gate.destructive-git.evaluated",
        json!({
            "v": 1, "class": "destructive-git", "rule_id": "git-force-push", "tier": "receipt",
            "matcher_kind": "command", "action_id": "claude-code__bash"
        }),
        true,
        1_700_000_100_003,
        &actor,
    ));

    // 4. A denied self-mod edit (the CurXecute vector): action receipt + denied gate receipt.
    signer.record(receipt(
        "act-hooks-edit",
        "claude-code__edit",
        json!({ "file_path": "/Users/dev/.claude/settings.json" }),
        false,
        1_700_000_100_004,
        &actor,
    ));
    signer.record(receipt(
        "gate-selfmod-denied",
        "kriya.gate.self-mod.denied",
        json!({
            "v": 1, "class": "self-mod", "rule_id": "self-config-path", "tier": "deny",
            "matcher_kind": "path", "action_id": "claude-code__edit",
            "corr_step": "act-hooks-edit"
        }),
        false,
        1_700_000_100_005,
        &actor,
    ));

    // 5. A send held as external — the recipient CLASS is recorded, never an address.
    signer.record(receipt(
        "gate-send-held",
        "kriya.gate.send.held",
        json!({
            "v": 1, "class": "send", "rule_id": "send-tool", "tier": "approve",
            "matcher_kind": "tool", "action_id": "claude-code__mcp__gmail__send_email",
            "recipients_class": "external", "recipients_count": 2
        }),
        false,
        1_700_000_100_006,
        &actor,
    ));

    let generated = std::fs::read_to_string(&log).unwrap();

    // Verify: every signature valid + the chain contiguous, every verb of the vocabulary present.
    let lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 7, "fixture line count");
    let mut prev: Option<String> = None;
    let mut verbs = Vec::new();
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
        if let Some(rest) = action.strip_prefix("kriya.gate.") {
            verbs.push(rest.split('.').nth(1).unwrap().to_string());
            assert_eq!(v["params"]["v"], 1, "gate receipts carry the schema-version marker");
        }
    }
    verbs.sort();
    assert_eq!(
        verbs,
        vec!["approved", "denied", "evaluated", "held", "held"],
        "every verb of the kriya.gate.* vocabulary is present"
    );

    // Land the fixture for the console suite (deterministic → no churn after first commit).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gates_ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reproduce audit.rs's signed bytes: `Receipt` fields in declaration order, `params`/`actor`
/// key-sorted, optional fields omitted when absent. (Mirrors `egress_fixture.rs`.)
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
                ",\"actor\":{{\"agent\":{},\"user\":{}}}",
                a["agent"], a["user"]
            ));
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

/// Key-sorted (BTreeMap) clone of a JSON value — serde_json::Value objects already sort keys when
/// the `preserve_order` feature is off (this crate's configuration), so a parse→serialize round
/// trip is canonical; this helper exists to make that dependency explicit.
fn canonical(v: &Value) -> Value {
    v.clone()
}

/// Lowercase-hex SHA-256 (the R20 chain link).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
