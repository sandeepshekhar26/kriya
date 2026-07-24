//! A5 PQ dual-signature parity fixtures (doc 27 A5 / `docs/design/a5-pq-dual-sig.md`, D7 / §8 /
//! acceptance #3).
//!
//! Two Rust-generated, committed fixtures (aws-lc has no JS build — design D7):
//! - `fixtures/pq-dual-signed-ledger.jsonl` — a short chain mixing Ed25519-only receipts (interop:
//!   old-shaped receipts still verify unchanged) with per-receipt dual-signed ones (opt-in mode,
//!   design D1/D2).
//! - `fixtures/pq-checkpoint-ledger.jsonl` — a `kriya.crypto.pq_key` attestation, five Ed25519-only
//!   receipts, and a `kriya.crypto.pq_checkpoint` sealing them (the DEFAULT mode, design D1/§4.1).
//!
//! **Why these are NOT regenerated on every `cargo test` run (unlike `fips_fixture.rs`'s analogous
//! trick):** Ed25519 is deterministic (RFC 8032), so the FIPS fixture is byte-identical across
//! regenerations and rewriting it every run is a harmless no-op. ML-DSA-87 signing is
//! **randomized** (FIPS 204 hedged signing, design D4/RT2.4) — the SAME seed always yields the
//! SAME public key, but a fresh signature every call. Regenerating on every test run would churn
//! the committed file's `pq_sig` bytes for no reason. So: the committed fixture is generated ONCE
//! (`regenerate_pq_fixtures`, `#[ignore]`d — run explicitly via
//! `cargo test --features pq-crypto -- --ignored regenerate_pq_fixtures` to refresh it) and the
//! normal, always-run tests below LOAD the committed file from disk and verify it — the actual
//! acceptance-#3 gate.
#![cfg(feature = "pq-crypto")]

use std::path::PathBuf;

use kriya::audit::{Actor, Receipt, Signer, PQ_CHECKPOINT, PQ_KEY};
use serde_json::Value;

fn dual_signed_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "kriya-pq-dual-fixture-{}",
        std::process::id()
    ))
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Regenerate BOTH committed A5 fixtures. `#[ignore]`d — see the module doc for why this isn't
/// run on every `cargo test`. Run explicitly to refresh the fixtures after an intentional design
/// change: `cargo test -p kriya --features pq-crypto -- --ignored regenerate_pq_fixtures`.
#[test]
#[ignore]
fn regenerate_pq_fixtures() {
    std::fs::create_dir_all(fixtures_dir()).unwrap();
    std::fs::write(
        fixtures_dir().join("pq-dual-signed-ledger.jsonl"),
        build_dual_signed_ledger(),
    )
    .unwrap();
    std::fs::write(
        fixtures_dir().join("pq-checkpoint-ledger.jsonl"),
        build_checkpoint_ledger(),
    )
    .unwrap();
}

const ED25519_SEED: [u8; 32] = [
    0x4c, 0xcd, 0x08, 0x9b, 0x28, 0xff, 0x96, 0xda, 0x9d, 0xb6, 0xc3, 0x46, 0xec, 0x11, 0x4e, 0x0f,
    0x5b, 0x8a, 0x31, 0x9f, 0x35, 0xab, 0xa6, 0x24, 0xda, 0x8c, 0xf6, 0xed, 0x4d, 0x0b, 0x24, 0x55,
];
const PQ_SEED_DUAL: [u8; 32] = [0x22; 32];
const PQ_SEED_CHECKPOINT: [u8; 32] = [0x33; 32];

fn build_dual_signed_ledger() -> String {
    let dir = dual_signed_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("ed25519.key");
    std::fs::write(&key, hex::encode(ED25519_SEED)).unwrap();
    let pq_seed_path = dir.join("pq-signing.seed");
    std::fs::write(&pq_seed_path, hex::encode(PQ_SEED_DUAL)).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone())
        .expect("fixed-key signer")
        .with_pq(&pq_seed_path, true) // per-receipt dual-sign mode (design D1 opt-in)
        .map_err(|(_, e)| e)
        .expect("fixed PQ seed");
    let actor = Actor::new("claude-code", "platform-eng");

    // Line 1: an Ed25519-ONLY receipt, signed by a separate signer sharing the SAME persisted
    // Ed25519 identity but with no PQ key attached — demonstrates the interop story (old-shaped
    // receipts keep verifying unchanged) within the SAME continuous chain the PQ-enabled signer
    // below continues.
    let bootstrap = Signer::with_identity(&key, log.clone()).expect("same identity, no PQ yet");
    bootstrap.record(
        Receipt::new(
            "pq-fx-1".into(),
            "kriya.test.plain_action".into(),
            serde_json::json!({ "note": "Ed25519-only — interop with pre-A5 verifiers" }),
            true,
            1_700_200_000_000,
        )
        .with_actor(Some(actor.clone())),
    );

    // Lines 2-3: per-receipt dual-signed (opt-in mode) via the PQ-enabled signer, continuing the
    // SAME chain (same log file, same Ed25519 identity).
    signer.record(
        Receipt::new(
            "pq-fx-2".into(),
            "kriya.test.dual_signed_action".into(),
            serde_json::json!({ "note": "per-receipt ML-DSA-87 dual signature" }),
            true,
            1_700_200_000_001,
        )
        .with_actor(Some(actor.clone())),
    );
    signer.record(
        Receipt::new(
            "pq-fx-3".into(),
            "kriya.test.dual_signed_action".into(),
            serde_json::json!({ "note": "second dual-signed receipt in the same chain" }),
            true,
            1_700_200_000_002,
        )
        .with_actor(Some(actor)),
    );

    std::fs::read_to_string(&log).unwrap()
}

fn build_checkpoint_ledger() -> String {
    let dir = std::env::temp_dir().join(format!(
        "kriya-pq-checkpoint-fixture-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("ed25519.key");
    std::fs::write(&key, hex::encode(ED25519_SEED)).unwrap();
    let pq_seed_path = dir.join("pq-signing.seed");
    std::fs::write(&pq_seed_path, hex::encode(PQ_SEED_CHECKPOINT)).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone())
        .expect("fixed-key signer")
        .with_pq(&pq_seed_path, false) // checkpoint-only mode (design D1 DEFAULT)
        .map_err(|(_, e)| e)
        .expect("fixed PQ seed");
    let actor = Actor::new("claude-code", "platform-eng");

    // The PQ key attestation (design §4.2) — first, so a verifier reading top-to-bottom sees the
    // PQ key bound to the pinned Ed25519 identity before any checkpoint references it.
    signer
        .attest_pq_key("kriya-gateway", Some(actor.clone()))
        .expect("PQ key loaded");

    // Five ordinary Ed25519-only receipts (checkpoint-only mode never dual-signs these).
    for i in 0..5 {
        signer.record(
            Receipt::new(
                format!("pq-fx-cp-{i}"),
                "kriya.test.plain_action".into(),
                serde_json::json!({ "i": i }),
                true,
                1_700_200_100_000 + i as u128,
            )
            .with_actor(Some(actor.clone())),
        );
    }

    // The checkpoint — ONE ML-DSA-87 signature sealing the five receipts above (design axiom
    // §1.4). `component` matches the on-device precedent (design §4.1).
    signer
        .pq_checkpoint("kriya-gateway", Some(actor))
        .expect("5 receipts recorded — checkpoint has something to seal");

    std::fs::read_to_string(&log).unwrap()
}

// ── The always-run acceptance gate: LOAD the committed fixtures and verify them ────────────────

#[test]
fn pq_dual_signed_fixture_verifies_ed25519_and_pq_and_chain() {
    let content = load_fixture("pq-dual-signed-ledger.jsonl");
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 3, "1 plain + 2 dual-signed");

    let mut prev: Option<String> = None;
    let mut dual_signed_count = 0;
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
        assert!(
            kriya::crypto::verify(&pk, &signed_bytes(&v), &sig),
            "Ed25519 must verify for every line, dual-signed or not"
        );
        assert_eq!(
            v.get("prev_hash").and_then(Value::as_str).map(str::to_string),
            prev,
            "chain must be contiguous across mixed plain/dual-signed lines"
        );
        prev = Some(sha256_hex(line.as_bytes()));

        if let Some(pq_alg) = v.get("pq_alg").and_then(Value::as_str) {
            dual_signed_count += 1;
            assert_eq!(pq_alg, "ML-DSA-87");
            let pq_pk = hex::decode(v["pq_public_key"].as_str().unwrap()).unwrap();
            let pq_sig = hex::decode(v["pq_sig"].as_str().unwrap()).unwrap();
            assert!(
                kriya::crypto::pq_verify(&pq_pk, &signed_bytes(&v), &pq_sig),
                "ML-DSA-87 signature must verify"
            );
            let expected_key_id = sha256_hex(&pq_pk)[..16].to_string();
            assert_eq!(v["pq_key_id"].as_str().unwrap(), expected_key_id);
        }
    }
    assert_eq!(dual_signed_count, 2, "exactly the two per-receipt dual-signed lines");

    // Line 1 has NO pq_* material at all — byte-for-byte interop with a pre-A5 verifier.
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    assert!(first.get("pq_alg").is_none());
    assert!(!lines[0].contains("pq_"), "line 1 must be byte-identical to a pre-A5 receipt");
}

#[test]
fn pq_checkpoint_fixture_verifies_and_seals_the_chain_head() {
    let content = load_fixture("pq-checkpoint-ledger.jsonl");
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    // attestation + 5 plain receipts + checkpoint = 7.
    assert_eq!(lines.len(), 7);

    let mut prev: Option<String> = None;
    let mut head_before_checkpoint: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        let v: Value = serde_json::from_str(line).unwrap();
        let pk: [u8; 32] = hex::decode(v["public_key"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let sig: [u8; 64] = hex::decode(v["signature"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        assert!(kriya::crypto::verify(&pk, &signed_bytes(&v), &sig));
        assert_eq!(
            v.get("prev_hash").and_then(Value::as_str).map(str::to_string),
            prev
        );

        if i == 5 {
            // The last plain receipt before the checkpoint — its line hash is what the
            // checkpoint (next line) must seal in `params.to_head_hash`.
            head_before_checkpoint = Some(sha256_hex(line.as_bytes()));
        }
        prev = Some(sha256_hex(line.as_bytes()));

        if v["action_id"] == PQ_KEY {
            assert_eq!(v["params"]["pq_alg"], "ML-DSA-87");
            assert!(v["params"]["pq_public_key"].as_str().unwrap().len() == 5184);
        }
        if v["action_id"] == PQ_CHECKPOINT {
            let pq_pk = hex::decode(v["pq_public_key"].as_str().unwrap()).unwrap();
            let pq_sig = hex::decode(v["pq_sig"].as_str().unwrap()).unwrap();
            assert!(
                kriya::crypto::pq_verify(&pq_pk, &signed_bytes(&v), &pq_sig),
                "checkpoint's own ML-DSA-87 signature must verify"
            );
            assert_eq!(
                v["params"]["to_head_hash"].as_str().unwrap(),
                head_before_checkpoint.as_deref().unwrap(),
                "the checkpoint must seal the true chain head (design §4.1 / row 7)"
            );
            assert_eq!(v["params"]["from_seq"], 1);
            assert_eq!(v["params"]["count"], 5);
            // pq_alg-authority rule (design §4.1 revision R4): signed params.pq_alg and the
            // unsigned top-level sibling MUST agree.
            assert_eq!(v["params"]["pq_alg"], v["pq_alg"]);
        }
    }
}

fn load_fixture(name: &str) -> String {
    std::fs::read_to_string(fixtures_dir().join(name))
        .unwrap_or_else(|e| panic!("missing committed fixture {name}: {e} — run `cargo test -p kriya --features pq-crypto -- --ignored regenerate_pq_fixtures`"))
}

/// Reproduce audit.rs's signed bytes (duplicated from `fips_fixture.rs` — kept test-local and
/// independent on purpose, same rationale as that file's copy).
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
