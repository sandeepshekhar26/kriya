//! B4 (doc 27 §4 / `docs/design/b4-temporal-conditions.md` D7/§3 *Tests*) — the two MIRRORED
//! fixtures that give the B-1 governance-corpus filter a real, cross-repo-verified guard:
//!
//! - **F-A** (`fixtures/temporal/wildcard-excludes-governance-fold.json`) — targets the FOLD
//!   (`session_cond::fold_receipts` here; the Console's `policy_sim.rs` temporal fold on the other
//!   side). `raw_receipts` mixes two `claude-code__bash` events with THREE governance receipts that
//!   all carry the SAME `run_id` (`kriya.spend.gate.warn`, `kriya.memory.write`,
//!   `kriya.artifact.provenance` — the exact receipt classes the original B-1 finding named), and
//!   `expected_events` proves only the two bash events survive. Both folds load this SAME file and
//!   must produce the IDENTICAL `expected_events` — that identity is axiom 3, made testable rather
//!   than asserted. The two bash events also share an equal `ts_ms`, exercising the M-2
//!   chain-position tie-break (the fold never reorders — it only filters — so equal-`ts_ms` events
//!   stay disambiguated by which line came first).
//! - **F-C** (`fixtures/temporal/governance-filter-ids.json`) — targets the PREDICATE
//!   (`is_governance_internal_b4` / `is_governed_kriya_vocabulary`), mirrored byte-for-byte into
//!   `../kriya-console/test/fixtures/temporal/governance-filter-ids.json` (the one-way dep forbids a
//!   shared crate, so the codebase's established answer is a mirrored committed file — the SAME
//!   `d1_memory_fixture.rs` / `d1-memory-receipts-ledger.jsonl` precedent this file follows).
//!
//! Both fixtures are committed byte-for-byte identical in both repos — regenerating either here
//! must be followed by copying the file into the Console's mirror path.

use std::path::PathBuf;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Receipt, Signer};
use kriya::corr::{self, Correlation};
use kriya::permissions::{TemporalDecision, TemporalPolicy};
use kriya::session_cond::{self, SessionEvent};
use serde_json::Value;

// A fixed, PUBLIC, synthetic 32-byte seed — deliberately NOT an RFC 8032 published test vector,
// since this repo's other three signed fixtures (`egress_fixture.rs`, `fips_fixture.rs`,
// `d1_memory_fixture.rs`) have already claimed the three commonly-cited short ones. "01" repeated
// 32 times is obviously synthetic on sight — never mistake it for a real signing identity.
const SEED_HEX: &str = "0101010101010101010101010101010101010101010101010101010101010101";

fn sandbox_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("kriya-temporal-fixture-{tag}-{}", std::process::id()))
}

/// Reproduce `audit.rs`'s signed bytes: `Receipt` fields in declaration order, `params`/`actor`
/// key-sorted, optional fields omitted when absent — the same helper `d1_memory_fixture.rs` /
/// `egress_fixture.rs` each carry their own copy of.
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

fn verify_line(v: &Value) -> bool {
    let Some(pk_hex) = v["public_key"].as_str() else { return false };
    let Some(sig_hex) = v["signature"].as_str() else { return false };
    let Ok(pk_bytes) = hex::decode(pk_hex) else { return false };
    let Ok(sig_bytes) = hex::decode(sig_hex) else { return false };
    let Ok(pk_arr): Result<[u8; 32], _> = pk_bytes.try_into() else { return false };
    let Ok(sig_arr): Result<[u8; 64], _> = sig_bytes.try_into() else { return false };
    let Ok(key) = VerifyingKey::from_bytes(&pk_arr) else { return false };
    key.verify(&signed_bytes(v), &Signature::from_bytes(&sig_arr)).is_ok()
}

#[test]
fn generates_and_verifies_the_wildcard_excludes_governance_fold_fixture() {
    let dir = sandbox_dir("f-a");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("gen.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).expect("fixed-key signer");
    let actor = Actor::new("claude-code", "platform-eng");
    let run_id = "run-fixture-f-a";
    let corr = Correlation::run(run_id);

    // Two claude-code__bash events SHARING one ts_ms (M-2 tie-break exercised by construction:
    // the fold never reorders, so these stay disambiguated by line/chain position, never dropped).
    signer.record(
        Receipt::new(
            "bash-1".into(),
            "claude-code__bash".into(),
            corr::attach(serde_json::json!({"command": "npm test"}), &corr),
            true,
            1_700_300_000_000,
        )
        .with_actor(Some(actor.clone())),
    );
    signer.record(
        Receipt::new(
            "bash-2".into(),
            "claude-code__bash".into(),
            corr::attach(serde_json::json!({"command": "git push origin main"}), &corr),
            false,
            1_700_300_000_000, // SAME ts_ms as bash-1, deliberately
        )
        .with_actor(Some(actor.clone())),
    );
    // The three B4-governance-internal receipts (D7's B-1 case) — all carrying the SAME run_id, so
    // a naive fold that only checked run_id (never the governance filter) would wrongly count them.
    signer.record(
        Receipt::new(
            "spend-warn".into(),
            "kriya.spend.gate.warn".into(),
            corr::attach(serde_json::json!({"budget_id": "session-cap", "scope": "session"}), &corr),
            true,
            1_700_300_000_001,
        )
        .with_actor(Some(actor.clone())),
    );
    signer.record(
        Receipt::new(
            "mem-write".into(),
            "kriya.memory.write".into(),
            corr::attach(serde_json::json!({"kriya.memory": {"class": "claude-md", "surface": "file"}}), &corr),
            true,
            1_700_300_000_002,
        )
        .with_actor(Some(actor.clone())),
    );
    // Synthetic by construction (A-5): kriya.artifact.provenance is emitted on the Console's OWN
    // `kriya-artifact-events.jsonl` chain in production and could never appear on a hook-lane log —
    // placed here in a single-source_log fixture specifically to test the PREDICATE independent of
    // the cross-lane scoping rule (that is `cross-lane-scoping.json`'s job, not this file's).
    signer.record(
        Receipt::new(
            "artifact-prov".into(),
            "kriya.artifact.provenance".into(),
            corr::attach(serde_json::json!({"manifest_hash": "deadbeef"}), &corr),
            true,
            1_700_300_000_003,
        )
        .with_actor(Some(actor.clone())),
    );

    let generated = std::fs::read_to_string(&log).unwrap();
    let raw_lines: Vec<&str> = generated.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(raw_lines.len(), 5, "2 bash + 3 governance receipts");

    let raw_receipts: Vec<Value> = raw_lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    for v in &raw_receipts {
        assert!(verify_line(v), "every F-A raw receipt must actually verify: {}", v["action_id"]);
    }

    let expected_events = session_cond::fold_receipts(&raw_receipts, run_id);
    assert_eq!(expected_events.len(), 2, "only the two bash events survive the B4 governance filter");
    assert_eq!(expected_events[0].action_id, "claude-code__bash");
    assert_eq!(expected_events[0].command.as_deref(), Some("npm test"));
    assert!(expected_events[0].success);
    assert_eq!(expected_events[1].action_id, "claude-code__bash");
    assert_eq!(expected_events[1].command.as_deref(), Some("git push origin main"));
    assert!(!expected_events[1].success);

    let fixture = serde_json::json!({
        "v": 1,
        "run_id": run_id,
        "source_log": "claude-code.jsonl",
        "raw_receipts": raw_receipts,
        "expected_events": expected_events.iter().map(|e| serde_json::json!({
            "action_id": e.action_id,
            "success": e.success,
            "ts_ms": e.ts_ms,
            "command": e.command,
        })).collect::<Vec<_>>(),
    });

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/temporal/wildcard-excludes-governance-fold.json");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, serde_json::to_string_pretty(&fixture).unwrap() + "\n");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Re-load the COMMITTED F-A fixture (not the freshly-generated one above) and re-run the real
/// fold over it — the same "committed file is the source of truth" check `d1_memory_fixture.rs`
/// exercises implicitly via the Console's own suite, made explicit here on the runtime side too.
#[test]
fn the_committed_f_a_fixture_still_folds_to_its_own_expected_events() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/temporal/wildcard-excludes-governance-fold.json");
    let text = std::fs::read_to_string(&path).expect("F-A fixture must be committed");
    let fixture: Value = serde_json::from_str(&text).unwrap();
    let run_id = fixture["run_id"].as_str().unwrap();
    let raw_receipts: Vec<Value> = fixture["raw_receipts"].as_array().unwrap().clone();

    for v in &raw_receipts {
        assert!(verify_line(v), "committed F-A raw receipt must still verify: {}", v["action_id"]);
    }

    let events = session_cond::fold_receipts(&raw_receipts, run_id);
    let expected: Vec<Value> = fixture["expected_events"].as_array().unwrap().clone();
    assert_eq!(events.len(), expected.len());
    for (got, want) in events.iter().zip(expected.iter()) {
        assert_eq!(got.action_id, want["action_id"].as_str().unwrap());
        assert_eq!(got.success, want["success"].as_bool().unwrap());
        assert_eq!(got.ts_ms, want["ts_ms"].as_u64().unwrap());
        assert_eq!(got.command.as_deref(), want["command"].as_str());
    }
}

/// F-C — the CLOSED, mirrored id-set both repos assert their own real predicate over. Every id
/// D7's disposition table names, one row each, so a future name-based guess can be checked against
/// this committed list instead of re-deriving the table from memory.
fn f_c_ids() -> (Vec<&'static str>, Vec<&'static str>) {
    let internal = vec![
        // The shipped base predicate's own four ids (Console `policy_sim.rs::is_governance_internal`).
        // `kriya.io.` / `kriya.policy.` are PREFIX-matched by that predicate, so every real emitted id
        // under each prefix is enumerated individually here — not just one representative example —
        // because F-C's byte-identity check against the Console mirror is a literal RAW-STRING compare
        // (`test/governance-filter.test.ts`: `expect(mine).toBe(theirs)`, on file text, not parsed
        // JSON), so both membership AND array order must match exactly. This file auto-regenerates its
        // own JSON from this vec on every `cargo test` (see `generates_the_governance_filter_ids_fixture`
        // below) — the Console copy is kept as a byte-for-byte copy of whatever this side produces.
        "kriya.attestation.on_device",
        "kriya.coverage.snapshot",
        "kriya.io.run.start",
        "kriya.io.run.exit",
        "kriya.policy.applied",
        "kriya.policy.stale",
        "kriya.policy.sim.result",
        "kriya.policy.cond.deny",
        "kriya.policy.cond.approval",
        "kriya.policy.cond.warn",
        // B4 additions — kriya's OWN bookkeeping about an agent action that already has its own receipt.
        "kriya.spend.gate.warn",
        "kriya.spend.gate.approval",
        "kriya.spend.gate.deny",
        "kriya.spend.session",
        "kriya.spend.rollup",
        "kriya.memory.write",
        "kriya.memory.update",
        "kriya.memory.delete",
        "kriya.crypto.module",
        "kriya.crypto.pq_checkpoint",
        "kriya.crypto.pq_key",
        "kriya.retention.checkpoint",
        "kriya.model.identity",
        "kriya.model.gate",
        "kriya.attest.pipeline",
        "kriya.attest.sandbox",
        "kriya.artifact.provenance",
        "kriya.diode.export",
        "kriya.diode.import",
        "kriya.replay.export",
        "kriya.console.drilldown",
        "kriya.otel.export.enabled",
        "kriya.exec.deterministic",
        // B-2 correction: watcher LIVENESS, not agent activity.
        "kriya.watch.heartbeat",
        "kriya.watch.run.start",
        "kriya.watch.run.exit",
        // The remaining `kriya.io.` PREFIX members — enumerated individually (base predicate coverage
        // is spot-checked separately below via `base_covered`; this full list exists for the Console
        // mirror's byte-identity requirement, since the registry-derived TS test on that side asserts
        // every REGISTERED vocabulary — including every emitted io facet — has a disposition here).
        "kriya.io.egress.mcp.allow",
        "kriya.io.egress.mcp.deny",
        "kriya.io.egress.mcp.approve",
        "kriya.io.egress.http.allow",
        "kriya.io.egress.http.deny",
        "kriya.io.egress.http.approve",
        "kriya.io.egress.model.allow",
        "kriya.io.egress.model.deny",
        "kriya.io.egress.model.approve",
        "kriya.io.egress.file.allow",
        "kriya.io.egress.file.deny",
        "kriya.io.egress.file.approve",
        "kriya.io.ingress.mcp.allow",
        "kriya.io.ingress.mcp.deny",
        "kriya.io.ingress.mcp.approve",
        "kriya.io.ingress.http.allow",
        "kriya.io.ingress.http.deny",
        "kriya.io.ingress.http.approve",
        "kriya.io.ingress.model.allow",
        "kriya.io.ingress.model.deny",
        "kriya.io.ingress.model.approve",
        "kriya.io.ingress.file.allow",
        "kriya.io.ingress.file.deny",
        "kriya.io.ingress.file.approve",
        // B3 (doc 27 §4 / docs/design/b3-drift-sentinel.md D2's cross-item build-order trap): the
        // local drift sentinel's own two vocabularies — excluded from B4's corpus automatically by
        // the SAME namespace default-exclude rule (no predicate change on this side; B3 touches no
        // runtime source at all). Required here too, or THIS generator test would silently
        // regenerate the fixture below without them the next time `cargo test` runs, clobbering B3's
        // fixture edit.
        "kriya.drift.observation",
        "kriya.drift.baseline",
    ];
    let governed = vec![
        "claude-code__bash",
        "claude-code__write",
        "widgets__list",
        // The four EVIDENCE sub-prefixes — real machine activity the agent (or something it
        // spawned) caused; kriya observed it, did not do it.
        "kriya.watch.proc.exec",
        "kriya.watch.file.write",
        "kriya.watch.net.connect",
        "kriya.watch.dns.lookup",
        // Deliberately NOT excluded — the agent's own forwarded model call (D7).
        "kriya.model.serve",
    ];
    (internal, governed)
}

#[test]
fn generates_the_governance_filter_ids_fixture() {
    let (internal, governed) = f_c_ids();
    let fixture = serde_json::json!({ "v": 1, "internal": internal, "governed": governed });
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/temporal/governance-filter-ids.json");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&out, serde_json::to_string_pretty(&fixture).unwrap() + "\n");
}

#[test]
fn f_c_this_repos_own_predicate_matches_every_listed_verdict() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/temporal/governance-filter-ids.json");
    let text = std::fs::read_to_string(&path).expect("F-C fixture must be committed");
    let fixture: Value = serde_json::from_str(&text).unwrap();
    let internal: Vec<String> = fixture["internal"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    let governed: Vec<String> = fixture["governed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();

    for id in &internal {
        assert!(session_cond::is_governance_internal_b4(id), "F-C says {id} should be governance-internal");
    }
    for id in &governed {
        assert!(!session_cond::is_governance_internal_b4(id), "F-C says {id} should be COUNTED (governed agent activity)");
    }

    // The superset property (D7): every id the BASE predicate excludes must also be in `internal`
    // (this runtime's own `is_governance_internal` mirror, not the Console's — but the SHAPE is
    // what's asserted; the Console's own F-C test asserts the identical relation against its real
    // `policy_sim.rs::is_governance_internal`).
    let base_covered = ["kriya.attestation.on_device", "kriya.coverage.snapshot", "kriya.io.egress.http.allow", "kriya.policy.applied"];
    for id in base_covered {
        assert!(session_cond::is_governance_internal(id), "{id} should be covered by the BASE predicate too");
        assert!(internal.iter().any(|s| s == id), "{id} (base-covered) must appear in F-C's internal bucket");
    }
}

/// F-B — the three EVALUATOR twins agree given IDENTICAL pre-folded input (R3): a wildcard
/// `action: "*"` + `predicate: count` rule over 2 folded events must yield `match_count == 2` (here,
/// observably via the DECISION: the rule's `cmp: ">=", n: 2` makes it match). Mirrored byte-for-byte
/// into `../kriya-console/test/fixtures/temporal/wildcard-counts-prefolded.json`; the Console side
/// asserts the SAME fixture against `simulate_temporal` (kriya-verify) and `evaluateTemporal` (TS).
#[test]
fn f_b_wildcard_counts_prefolded_fixture_matches_this_repos_evaluate() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/temporal/wildcard-counts-prefolded.json");
    let text = std::fs::read_to_string(&path).expect("F-B fixture must be committed");
    let fixture: Value = serde_json::from_str(&text).unwrap();

    let policy: TemporalPolicy = serde_json::from_value(fixture["policy"].clone()).unwrap();
    let events: Vec<SessionEvent> = serde_json::from_value(fixture["events"].clone()).unwrap();
    let action_id = fixture["action_id"].as_str().unwrap();
    let command = fixture["command"].as_str();
    let now_ms = fixture["now_ms"].as_u64().unwrap();
    let expected_decision = fixture["expected_decision"].as_str().unwrap();

    let decision = policy.evaluate(&events, action_id, command, now_ms);
    match expected_decision {
        "deny" => assert!(matches!(decision, TemporalDecision::Deny(_)), "expected Deny, got {decision:?}"),
        "approval" => assert!(matches!(decision, TemporalDecision::Approval(_)), "expected Approval, got {decision:?}"),
        "pass" => assert_eq!(decision, TemporalDecision::Pass),
        other => panic!("unknown expected_decision {other}"),
    }
    if let TemporalDecision::Deny(rec) | TemporalDecision::Approval(rec) = decision {
        let expected_match_count = fixture["expected_match_count"].as_u64().unwrap();
        assert_eq!(rec.conditions.len(), 1);
        assert_eq!(rec.conditions[0].match_count, expected_match_count);
    }
}
