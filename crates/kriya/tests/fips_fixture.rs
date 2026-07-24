//! A4 FIPS-lane parity fixture (doc 27 A4 / `docs/design/a4-fips-lane.md`, D5 items 2 + 3 + 6).
//!
//! Generates a small, DETERMINISTIC chain — a genesis action receipt, a chained action receipt,
//! and a `kriya.crypto.module` attestation — signed under the **FIPS lane** (`aws-lc-rs` +
//! `fips`), then independently re-verifies every line with the plain `ed25519-dalek` crate (not
//! `kriya::crypto::verify` — a genuinely separate implementation) to prove the design's central
//! claim: Ed25519 is deterministic (RFC 8032), so a FIPS-lane signature is byte-for-byte
//! indistinguishable from a default-lane one and verifies under ANY correct Ed25519
//! implementation, FIPS-aware or not (acceptance #2).
//!
//! Signed with a FIXED key (an RFC 8032 test vector, distinct from `egress_fixture.rs`'s) so the
//! committed fixture is stable across runs and machines. This same file is committed byte-for-byte
//! in the kriya-console repo at `test/fixtures/fips-signed-ledger.jsonl`, where `verify.ts`
//! (TS/WebCrypto) re-verifies it too — proving the third independent implementation also accepts
//! it (design D5 item 2).
#![cfg(feature = "fips-crypto")]

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer, ATTESTATION_CRYPTO_MODULE};
use serde_json::{json, Value};

// RFC 8032 Ed25519 test-vector seed (the SECOND standard test vector — distinct from
// `egress_fixture.rs`'s first one so the two fixtures are visibly independent) — a PUBLIC,
// well-known key: this fixture is synthetic evidence, never a real signing identity.
const SEED_HEX: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4d0b2455";

#[test]
fn generates_and_verifies_the_fips_signed_parity_fixture() {
    let dir = std::env::temp_dir().join(format!("kriya-fips-fixture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key FIPS signer");
    let actor = Actor::new("claude-code", "platform-eng");

    // The module actually initialized in FIPS mode (design D5 item 4 — the build/test gate that
    // STOPS rather than ships an inflated claim if Ed25519 isn't approved under this build).
    let module = kriya::crypto::active_module();
    assert_eq!(module.backend, "aws-lc-rs");
    assert!(
        module.fips_mode_active,
        "aws_lc_rs::try_fips_mode() must report FIPS mode active under the fips-crypto feature \
         — if this fails, cert #5298's Ed25519 approval assumption (V3) does not hold on this \
         build and the FIPS claim must not ship"
    );

    // Genesis action receipt.
    signer.record(
        Receipt::new(
            "act-genesis".into(),
            "claude-code__read_file".into(),
            json!({ "path": "/etc/hosts" }),
            true,
            1_700_100_000_000,
        )
        .with_actor(Some(actor.clone())),
    );
    // A chained action receipt.
    signer.record(
        Receipt::new(
            "act-chained".into(),
            "claude-code__write_file".into(),
            json!({ "path": "/tmp/out.txt", "bytes": 42 }),
            true,
            1_700_100_000_001,
        )
        .with_actor(Some(actor.clone())),
    );
    // The A4 crypto-module self-attestation (design D2 / §4) — additive, new `action_id`. Built
    // by hand with FIXED illustrative content (rather than calling `Signer::attest_crypto_module`,
    // which stamps a random uuid + wall-clock time AND the live host's os/arch/operational_environment
    // — right for a real deployment, wrong for a fixture that must stay byte-identical whether
    // generated on the Linux CI runner or a macOS dev machine) but the exact same `params` SHAPE
    // that method produces; see `attest_crypto_module_shape_matches_the_fixture` below for a
    // direct test of the real method against this build's live facts. The illustrative values are
    // the design's own §4 Linux/FIPS example row (validated-oe, module-drbg).
    let params = json!({
        "backend": "aws-lc-rs",
        "fips_module": "AWS-LC-FIPS 3.x",
        "cmvp_cert": "5298",
        "fips_mode_active": true,
        "operational_environment": "validated-oe",
        "key_provenance": "module-drbg",
        "os": "linux",
        "arch": "x86_64",
        "component": "kriya-gateway",
    });
    let attestation = signer.record(
        Receipt::new(
            "act-crypto-module".into(),
            ATTESTATION_CRYPTO_MODULE.to_string(),
            params,
            true,
            1_700_100_000_002,
        )
        .with_actor(Some(actor.clone())),
    );
    assert_eq!(attestation.receipt.params["backend"], "aws-lc-rs");
    assert_eq!(attestation.receipt.params["fips_mode_active"], true);
    assert_eq!(attestation.receipt.params["cmvp_cert"], "5298");

    let generated = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 3, "genesis + chained + attestation");

    // Independent re-verification with PLAIN ed25519-dalek (not the facade) — the acceptance-#2
    // byte-for-byte proof: a FIPS-lane signature verifies under a totally separate Ed25519
    // implementation with zero special-casing.
    let mut prev: Option<String> = None;
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
        let vk = VerifyingKey::from_bytes(&pk).unwrap();
        assert!(
            vk.verify(&signed_bytes(&v), &Signature::from_bytes(&sig))
                .is_ok(),
            "FIPS-signed line must verify under plain ed25519-dalek: {}",
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
    }

    // Also verify through the facade itself (both lanes, whichever this test binary was built
    // with) — same result, since verification never over- or under-claims across lanes for
    // honest input (design RT2.5).
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
        assert!(kriya::crypto::verify(&pk, &signed_bytes(&v), &sig));
    }

    // Land the fixture (deterministic → no churn after first commit; copy this same file into
    // the kriya-console repo's test/fixtures/fips-signed-ledger.jsonl per the design).
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fips-signed-ledger.jsonl");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, &generated);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Reproduce audit.rs's signed bytes: `Receipt` fields in declaration order, `params`/`actor`
/// key-sorted, optional fields omitted when absent. (Duplicated from `egress_fixture.rs` — kept
/// test-local and independent on purpose, same rationale as that file's copy.)
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

/// Direct test of `Signer::attest_crypto_module` (design D2) — the real method the binaries call,
/// as opposed to the hand-built fixture line above. Not part of the committed fixture (random
/// uuid + wall-clock ts_ms would make it non-deterministic).
#[test]
fn attest_crypto_module_shape_matches_the_fixture() {
    let log = std::env::temp_dir().join(format!("kriya-fips-attest-{}.jsonl", uuid_like()));
    let signer = Signer::with_log_path(log);
    let attestation =
        signer.attest_crypto_module("kriya-gateway", Some(Actor::new("claude-code", "ci")));
    assert_eq!(attestation.receipt.action_id, ATTESTATION_CRYPTO_MODULE);
    assert_eq!(attestation.receipt.params["backend"], "aws-lc-rs");
    assert_eq!(attestation.receipt.params["fips_module"], "AWS-LC-FIPS 3.x");
    assert_eq!(attestation.receipt.params["cmvp_cert"], "5298");
    assert_eq!(attestation.receipt.params["fips_mode_active"], true);
    assert_eq!(attestation.receipt.params["component"], "kriya-gateway");
    // A freshly generated (never-persisted-before) key under the fips-crypto lane is minted by
    // the module DRBG, not disclosed as imported (RT2.3).
    assert_eq!(attestation.receipt.params["key_provenance"], "module-drbg");
    let oe = attestation.receipt.params["operational_environment"]
        .as_str()
        .unwrap();
    assert!(
        ["validated-oe", "validated-module-untested-oe", "outside-cmvp-oe"].contains(&oe),
        "unexpected operational_environment: {oe}"
    );
    // It verifies and chains like any other receipt (additivity, design RT2.1).
    let pk: [u8; 32] = hex::decode(&attestation.public_key)
        .unwrap()
        .try_into()
        .unwrap();
    let sig: [u8; 64] = hex::decode(&attestation.signature).unwrap().try_into().unwrap();
    assert!(kriya::crypto::verify(
        &pk,
        &serde_json::to_vec(&attestation.receipt).unwrap(),
        &sig
    ));
}

fn uuid_like() -> String {
    format!("{:?}-{}", std::time::SystemTime::now(), std::process::id())
}
