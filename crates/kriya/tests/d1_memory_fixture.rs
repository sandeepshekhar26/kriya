//! D1 (doc 27 §4 / `docs/design/d1-memory-receipts.md` §5 test-plan): the TS↔Rust parity fixture
//! for `kriya.memory.*` — a small, DETERMINISTIC chain covering all four confirmed memory-surface
//! classes (§D-1) that the kriya-console suite imports to prove its TS verifier (`src/lib/verify.ts`)
//! re-derives the runtime's signed bytes byte-identically, exactly like `egress_fixture.rs` does for
//! `kriya.io.*`. Signed with a FIXED key (an RFC 8032 test vector, distinct from the other fixtures'
//! seeds) so the committed fixture is stable across runs and machines.
//!
//! The fixture is committed byte-for-byte identical in both repos: this file mirrors
//! `../kriya-console/test/fixtures/d1-memory-receipts-ledger.jsonl`.

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer};
use kriya::corr::Correlation;
use kriya::memwrite::{self, MemoryOp, MemorySurfaceKind, MemoryVerb, VerbBasis};
use serde_json::Value;

// RFC 8032 Ed25519 test-vector seed (the THIRD standard test vector — distinct from
// `egress_fixture.rs`'s and `fips_fixture.rs`'s) — a PUBLIC, well-known key: this fixture is
// synthetic evidence, never a real signing identity.
const SEED_HEX: &str = "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7";

#[test]
fn generates_and_verifies_the_d1_memory_receipts_parity_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-d1-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");
    let actor = Actor::new("claude-code", "platform-eng");
    let salt = [0x42u8; 32]; // fixed, illustrative device-local salt — deterministic fixture only

    // (a) claude-code__write -> CLAUDE.md ⇒ kriya.memory.write, class claude-md.
    let run_a = Correlation::run("run-fixture-a");
    signer.record(
        Receipt::new(
            "act-a".into(),
            "claude-code__write".into(),
            kriya::corr::attach(
                serde_json::json!({ "file_path": "CLAUDE.md", "content": "Standing instructions." }),
                &run_a,
            ),
            true,
            1_700_200_000_000,
        )
        .with_actor(Some(actor.clone())),
    );
    let surface_a = memwrite::memory_surface_for(
        "Write",
        &serde_json::json!({ "file_path": "CLAUDE.md", "content": "Standing instructions." }),
    )
    .unwrap();
    memwrite::emit_memory_receipt(
        &signer,
        Some(&actor),
        MemoryVerb::Write,
        VerbBasis::ToolWrite,
        &surface_a,
        b"Standing instructions.",
        true,
        &run_a,
        &salt,
    );

    // (b) claude-code__edit -> ~/.claude/projects/x/memory/MEMORY.md ⇒ kriya.memory.update, class
    // claude-memory-dir.
    let run_b = Correlation::run("run-fixture-b");
    let memory_dir_path = "/Users/dev/.claude/projects/x/memory/MEMORY.md";
    signer.record(
        Receipt::new(
            "act-b".into(),
            "claude-code__edit".into(),
            kriya::corr::attach(
                serde_json::json!({ "file_path": memory_dir_path, "old_string": "a", "new_string": "b" }),
                &run_b,
            ),
            true,
            1_700_200_000_001,
        )
        .with_actor(Some(actor.clone())),
    );
    let surface_b = memwrite::memory_surface_for(
        "Edit",
        &serde_json::json!({ "file_path": memory_dir_path, "old_string": "a", "new_string": "b" }),
    )
    .unwrap();
    memwrite::emit_memory_receipt(
        &signer,
        Some(&actor),
        MemoryVerb::Update,
        VerbBasis::ToolEdit,
        &surface_b,
        b"b",
        true,
        &run_b,
        &salt,
    );

    // (c) claude-code__write -> .claude/settings.json ⇒ kriya.memory.write, class claude-settings.
    let run_c = Correlation::run("run-fixture-c");
    signer.record(
        Receipt::new(
            "act-c".into(),
            "claude-code__write".into(),
            kriya::corr::attach(
                serde_json::json!({ "file_path": ".claude/settings.json", "content": "{\"permissions\":{}}" }),
                &run_c,
            ),
            true,
            1_700_200_000_002,
        )
        .with_actor(Some(actor.clone())),
    );
    let surface_c = memwrite::memory_surface_for(
        "Write",
        &serde_json::json!({ "file_path": ".claude/settings.json", "content": "{\"permissions\":{}}" }),
    )
    .unwrap();
    memwrite::emit_memory_receipt(
        &signer,
        Some(&actor),
        MemoryVerb::Write,
        VerbBasis::ToolWrite,
        &surface_c,
        b"{\"permissions\":{}}",
        true,
        &run_c,
        &salt,
    );

    // (d) a registered broker MCP memory tool (`notes__create_entities`, op create, content_field
    // "entity") ⇒ kriya.memory.write, class mcp-registered.
    let run_d = Correlation::run("run-fixture-d");
    signer.record(
        Receipt::new(
            "act-d".into(),
            "notes__create_entities".into(),
            kriya::corr::attach(
                serde_json::json!({ "entity": "Q3 roadmap note", "kind": "note" }),
                &run_d,
            ),
            true,
            1_700_200_000_003,
        )
        .with_actor(Some(actor.clone())),
    );
    let surface_d = memwrite::mcp_surface("notes", "create_entities", MemoryOp::Create);
    let content_d = memwrite::mcp_memory_content(&serde_json::json!({ "entity": "Q3 roadmap note", "kind": "note" }), Some("entity"));
    memwrite::emit_memory_receipt(
        &signer,
        Some(&actor),
        MemoryVerb::Write,
        VerbBasis::ToolWrite,
        &surface_d,
        &content_d,
        true,
        &run_d,
        &salt,
    );

    let generated = std::fs::read_to_string(&log).unwrap();

    // Verify: every signature valid + the chain contiguous, and every kriya.memory.* receipt is
    // hash-only (never the sentinel content) with the expected class/verb/surface shape.
    let lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 8, "4 action receipts + 4 memory receipts");
    let mut prev: Option<String> = None;
    let mut mem_count = 0;
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

        if v["action_id"].as_str().unwrap().starts_with("kriya.memory.") {
            mem_count += 1;
            assert!(v["params"]["kriya.memory"].get("content_sha256").is_some());
            assert!(v["params"]["kriya.memory"].get("path").is_none() || v["params"]["kriya.memory"]["surface"] == "file");
            // Never the written content, only its hash.
            assert!(!line.contains("Standing instructions."));
            assert!(!line.contains("Q3 roadmap note"));
        }
    }
    assert_eq!(mem_count, 4, "one kriya.memory.* receipt per class (a)-(d)");

    // Spot-check the exact params shape the design's §D-3 field table promises, for class (a).
    let rows: Vec<Value> = lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let a = rows
        .iter()
        .find(|v| v["action_id"] == "kriya.memory.write" && v["params"]["kriya.corr"]["run_id"] == "run-fixture-a")
        .unwrap();
    let m = &a["params"]["kriya.memory"];
    assert_eq!(m["class"], "claude-md");
    assert_eq!(m["surface"], MemorySurfaceKind::File.as_str());
    assert_eq!(m["verb_basis"], "tool-write");
    assert_eq!(m["path"], "CLAUDE.md");
    assert_eq!(
        m["content_sha256"],
        {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(b"Standing instructions."))
        }
    );
    assert_eq!(m["content_bytes"], "Standing instructions.".len());

    let d = rows.iter().find(|v| v["action_id"] == "kriya.memory.write" && v["params"]["kriya.corr"]["run_id"] == "run-fixture-d").unwrap();
    let md = &d["params"]["kriya.memory"];
    assert_eq!(md["class"], "mcp-registered");
    assert_eq!(md["surface"], "mcp-tool");
    assert_eq!(md["tool_ref"]["server"], "notes");
    assert_eq!(md["tool_ref"]["tool"], "create_entities");
    assert_eq!(md["tool_ref"]["op"], "create");

    // Land the fixture for the console suite (deterministic → no churn after first commit).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/d1-memory-receipts-ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reproduce `audit.rs`'s signed bytes: `Receipt` fields in declaration order, `params`/`actor`
/// key-sorted, optional fields omitted when absent. Mirrors `egress_fixture.rs`'s identical helper.
fn signed_bytes(v: &Value) -> Vec<u8> {
    let mut s = String::from("{");
    s.push_str(&format!("\"step_id\":{}", v["step_id"]));
    s.push_str(&format!(",\"action_id\":{}", v["action_id"]));
    s.push_str(&format!(",\"params\":{}", serde_json::to_string(&canonical(&v["params"])).unwrap()));
    s.push_str(&format!(",\"success\":{}", v["success"]));
    s.push_str(&format!(",\"ts_ms\":{}", v["ts_ms"]));
    if let Some(a) = v.get("actor") {
        if !a.is_null() {
            s.push_str(&format!(",\"actor\":{}", serde_json::to_string(&canonical(a)).unwrap()));
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
