//! F4-wasm (doc 28 §F4) integration tests — determinism, file-round-trip, and tamper fixtures over
//! the real example WASI-p2 components in `examples/f4-wasm-tools/`. Only compiled/run under
//! `--features wasm-exec` (this whole file is a no-op otherwise, via the module-level cfg).
//!
//! These tests build the example guest crates via `cargo build --release --target wasm32-wasip2`
//! the first time they run (fast on a warm target dir) rather than checking in binary `.wasm`
//! fixtures — the guest SOURCE is the fixture, not an opaque binary blob nobody can audit.

#![cfg(feature = "wasm-exec")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use kriya::wasmexec::{build_and_record, verify_bundle, verify_bundle_file, ExecutionInputs};

/// The repo root, resolved from this crate's manifest dir (`crates/kriya`) — two levels up.
fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crates/kriya is two levels under the repo root")
        .to_path_buf()
}

/// `cargo test` runs tests in parallel THREADS by default, and several tests below build the SAME
/// example guest crate. Cargo itself serializes concurrent builds of one target dir via a file
/// lock, but the observed behavior under heavy concurrency here was an intermittent "wasm not
/// found after a reported-successful build" — so this cache makes each example get built exactly
/// ONCE per test-binary run, with every caller after the first just reusing the resolved path.
static BUILD_CACHE: OnceLock<Mutex<std::collections::HashMap<String, PathBuf>>> = OnceLock::new();

/// Build one `examples/f4-wasm-tools/<name>` guest crate to `wasm32-wasip2` release and return the
/// path to the built component. Panics with the full cargo output on a build failure — a broken
/// example fixture should fail loudly, not be silently skipped.
fn build_example(name: &str) -> PathBuf {
    let cache = BUILD_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(path) = guard.get(name) {
        return path.clone();
    }

    let dir = repo_root().join("examples/f4-wasm-tools").join(name);
    let status = Command::new("cargo")
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .status()
        .unwrap_or_else(|e| panic!("spawning cargo build for example {name}: {e}"));
    assert!(status.success(), "building example guest {name} failed");
    let wasm = dir
        .join("target/wasm32-wasip2/release")
        .join(format!("{name}.wasm"));
    assert!(wasm.exists(), "expected {} to exist after build", wasm.display());

    guard.insert(name.to_string(), wasm.clone());
    wasm
}

fn text_transform_wasm() -> PathBuf {
    build_example("text-transform")
}

fn json_filter_wasm() -> PathBuf {
    build_example("json-filter")
}

fn inputs(args: Vec<&str>, stdin: &[u8], seed: u64) -> ExecutionInputs {
    ExecutionInputs {
        args: args.into_iter().map(String::from).collect(),
        env: BTreeMap::new(),
        stdin: stdin.to_vec(),
        seed,
        epoch_ms: 1_700_000_000_000,
        fuel_limit: 200_000_000,
    }
}

/// DETERMINISM FIXTURE: the SAME bundle re-run twice on this machine → byte-identical stdout AND
/// fuel-identical, asserted (not just eyeballed) — the F4-wasm gate's headline requirement.
#[test]
fn same_bundle_rerun_twice_is_byte_and_fuel_identical() {
    let module = text_transform_wasm();
    let bundle = build_and_record(
        &module,
        inputs(vec!["text-transform", "--upper"], b"hello deterministic world", 42),
    )
    .expect("record run 1");

    let report_a = verify_bundle(&bundle, Some(&module));
    let report_b = verify_bundle(&bundle, Some(&module));

    assert!(report_a.ok, "first re-run must verify: {:?}", report_a.fields);
    assert!(report_b.ok, "second re-run must verify: {:?}", report_b.fields);

    let a = report_a.reexecuted.expect("report_a re-executed");
    let b = report_b.reexecuted.expect("report_b re-executed");
    assert_eq!(a.stdout_hash, b.stdout_hash, "stdout must be byte-identical across the two re-runs");
    assert_eq!(a.stderr_hash, b.stderr_hash, "stderr must be byte-identical across the two re-runs");
    assert_eq!(a.fuel_consumed, b.fuel_consumed, "fuel consumed must be identical across the two re-runs");
    // And identical to what the ORIGINAL record run produced.
    assert_eq!(bundle.stdout_hash, a.stdout_hash);
    assert_eq!(bundle.fuel_consumed, a.fuel_consumed);
}

/// CROSS-CHECK: the SAME bundle verifies after a file round-trip (write to disk, read back, verify)
/// — not just the in-memory path the test above exercises.
#[test]
fn bundle_verifies_after_a_file_round_trip() {
    let module = text_transform_wasm();
    let bundle = build_and_record(&module, inputs(vec!["text-transform", "--lower"], b"MIXED Case Input", 7))
        .expect("record");

    let dir = std::env::temp_dir().join(format!("kriya-f4wasm-roundtrip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bundle_path = dir.join("bundle.json");
    std::fs::write(&bundle_path, serde_json::to_string_pretty(&bundle).unwrap()).unwrap();

    // Read back from a FRESH path (not the in-memory `bundle`) — this is the actual round-trip.
    let report = verify_bundle_file(&bundle_path, Some(&module));
    assert!(report.ok, "a genuine bundle must verify after a file round-trip: {:?}", report.fields);

    let _ = std::fs::remove_dir_all(&dir);
}

/// TAMPER FIXTURE: one input byte flipped → verify fails with the RIGHT reason (not just "false").
#[test]
fn one_flipped_stdin_byte_fails_verify_with_the_right_reason() {
    let module = text_transform_wasm();
    let mut bundle = build_and_record(&module, inputs(vec!["text-transform", "--upper"], b"original input", 1))
        .expect("record");

    // Flip one byte in the recorded stdin hex WITHOUT updating stdin_hash — the exact "tampered
    // bundle" scenario: someone (or something) edited the stored bytes after the fact.
    let mut raw = hex::decode(&bundle.stdin_hex).unwrap();
    raw[0] ^= 0xFF;
    bundle.stdin_hex = hex::encode(&raw);

    let report = verify_bundle(&bundle, Some(&module));
    assert!(!report.ok, "a bundle with tampered stdin bytes must fail verify");
    let stdin_field = report
        .fields
        .iter()
        .find(|f| f.field == "stdin_hash")
        .expect("stdin_hash field must be checked");
    assert!(!stdin_field.ok, "stdin_hash check must be the one that fails");
    assert!(
        stdin_field.detail.contains("tampered"),
        "failure reason must say the bundle was tampered, got: {}",
        stdin_field.detail
    );
}

/// TAMPER FIXTURE, module variant: the wasm module bytes changed after recording (a swapped/edited
/// binary) → verify fails on `module_sha256` specifically, before even attempting re-execution.
#[test]
fn a_replaced_module_fails_verify_on_module_hash_before_reexecuting() {
    let module = text_transform_wasm();
    let bundle = build_and_record(&module, inputs(vec!["text-transform", "--upper"], b"abc", 3)).expect("record");

    // Point verify at a DIFFERENT module (the json-filter one) — simulates "the file at this path
    // is no longer the file that was recorded."
    let different_module = json_filter_wasm();
    let report = verify_bundle(&bundle, Some(&different_module));
    assert!(!report.ok);
    let module_field = report
        .fields
        .iter()
        .find(|f| f.field == "module_sha256")
        .expect("module_sha256 must be checked");
    assert!(!module_field.ok);
    assert!(report.reexecuted.is_none(), "must fail closed before spending time re-executing a mismatched module");
}

/// A genuinely divergent re-execution (here: simulated by hand-editing the recorded `stdout_hash`
/// to a value the real deterministic re-run will never produce) fails on `stdout_hash` specifically
/// — proves the comparison is real, not a rubber stamp.
#[test]
fn a_bundle_with_a_falsified_stdout_hash_fails_on_stdout_hash() {
    let module = text_transform_wasm();
    let mut bundle = build_and_record(&module, inputs(vec!["text-transform", "--upper"], b"xyz", 5)).expect("record");
    bundle.stdout_hash = "0".repeat(64);

    let report = verify_bundle(&bundle, Some(&module));
    assert!(!report.ok);
    let stdout_field = report.fields.iter().find(|f| f.field == "stdout_hash").unwrap();
    assert!(!stdout_field.ok);
    assert!(stdout_field.detail.contains("MISMATCH"));
}

/// The two example tools actually do their job (not just "run without crashing") — json-filter's
/// real filtering behavior, exercised end-to-end through the deterministic-execution lane, cross-
/// checked against a plain (non-lane) run of the same logic in-process.
#[test]
fn json_filter_example_tool_filters_correctly_under_the_lane() {
    let module = json_filter_wasm();
    let stdin = br#"[{"name":"a","kind":"x"},{"name":"b","kind":"y"},{"name":"c","kind":"x"}]"#;
    let bundle = build_and_record(
        &module,
        inputs(vec!["json-filter", "--field", "kind", "--equals", "x"], stdin, 11),
    )
    .expect("record");
    assert!(bundle.success, "json-filter must succeed on well-formed input");

    let value: serde_json::Value = serde_json::from_slice(stdin).unwrap();
    let expected: Vec<&serde_json::Value> = value
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item.get("kind").and_then(|v| v.as_str()) == Some("x"))
        .collect();
    let expected_json = serde_json::to_string(&expected).unwrap();
    let expected_hash = kriya::wasmexec::determinism::sha256_hex(expected_json.as_bytes());
    assert_eq!(bundle.stdout_hash, expected_hash, "json-filter's actual filtered output must match hand-computed expectations");

    let report = verify_bundle(&bundle, Some(&module));
    assert!(report.ok);
}

/// The size-cap refusal is honest (a refusal, never a silent truncation): an over-cap stdin makes
/// `build_and_record` return `Err` rather than producing an unreplayable bundle.
#[test]
fn oversized_stdin_is_refused_not_truncated() {
    let module = text_transform_wasm();
    let huge = vec![b'a'; kriya::wasmexec::MAX_INPUT_BYTES + 1];
    let result = build_and_record(&module, inputs(vec!["text-transform", "--upper"], &huge, 1));
    assert!(result.is_err(), "over-cap input must be refused");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("cap"), "refusal reason should mention the size cap, got: {err}");
}
