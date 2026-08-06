//! O-10 parity fixture (doc 33 §5.8): a small, DETERMINISTIC chain of `kriya.evidence.mcp.start`
//! receipts — the boot receipt the read-only evidence MCP server (`kriya-mcp --evidence`) signs —
//! that the kriya-console suite imports to prove its TS verifier re-derives the runtime's bytes
//! byte-identically and recognizes the vocabulary. Signed with the same FIXED RFC 8032 test-vector
//! key as the other parity fixtures, so the committed file is stable across runs and machines.
//!
//! Content-free by construction: the params carry only `{scope, tools}` — the reader is in evidence,
//! never a query or a result. Verified here with the crate's OWN public verifier
//! (`audit::verify_signed_line` / `audit::verify_chain`) — the single source of truth the console
//! mirrors in TypeScript.

use std::path::PathBuf;

use kriya::audit::{verify_chain, verify_signed_line, Actor, Receipt, Signer, EVIDENCE_MCP_START};
use serde_json::json;

// RFC 8032 Ed25519 test-vector seed — a PUBLIC, well-known key: synthetic evidence, never a real
// signing identity (identical to pay_fixture.rs / gates_fixture.rs).
const SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

#[test]
fn generates_and_verifies_the_evidence_mcp_start_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-evidence-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");
    let actor = Actor::new("claude-code", "platform-eng");
    let tools = json!([
        "receipts_search",
        "receipt_get",
        "chain_verify",
        "session_tree",
        "spend_summary"
    ]);

    // Two boots of the read-only reader — a small chain (each start is content-free: scope + tools).
    signer.record(
        Receipt::new(
            "evidence-start-1".into(),
            EVIDENCE_MCP_START.into(),
            json!({ "scope": "read-only", "tools": tools }),
            true,
            1_700_000_500_000,
        )
        .with_actor(Some(actor.clone())),
    );
    signer.record(
        Receipt::new(
            "evidence-start-2".into(),
            EVIDENCE_MCP_START.into(),
            json!({ "scope": "read-only", "tools": tools }),
            true,
            1_700_000_600_000,
        )
        .with_actor(Some(actor.clone())),
    );

    let generated = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<String> = generated.lines().map(str::to_string).collect();
    assert_eq!(
        lines.iter().filter(|l| !l.trim().is_empty()).count(),
        2,
        "fixture line count"
    );
    for l in &lines {
        if l.trim().is_empty() {
            continue;
        }
        let v = verify_signed_line(l);
        assert!(v.verified, "fixture line must verify: {:?}", v.reason);
        let s = v.signed.unwrap();
        assert_eq!(s.receipt.action_id, EVIDENCE_MCP_START);
        assert_eq!(s.receipt.params["scope"], "read-only");
        // Content-free: exactly scope + tools, nothing else.
        let keys: Vec<&String> = s.receipt.params.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec![&"scope".to_string(), &"tools".to_string()]);
    }
    assert!(verify_chain(&lines).verified, "chain contiguous");

    // Land the fixture for the console suite (deterministic → no churn after first commit).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/evidence_mcp_start_ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}
