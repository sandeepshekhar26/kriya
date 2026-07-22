//! End-to-end smoke test for `kriya-ci` — **the governed CI lane** (S4). Builds the real `kriya-ci`
//! and `kriya-govern` binaries and drives them exactly as a CI job would: `kriya-ci run` wraps a
//! governed agent step (here a tiny `sh` script that pipes a couple of tool calls through
//! `kriya-govern`, so the test needs no Node/model/keys), then gates on the real signed receipts.
//!
//! Proves the contract at the real process boundary (not just the in-bin unit tests):
//!   - an all-allowed step exits 0 CLEAN and leaves verifiable receipts,
//!   - a policy-blocked action fails the job (exit 3 POLICY_DENIED) and the blocked attempt is
//!     recorded evidence,
//!   - a missing policy fails closed (exit 4), never silently permissive.
//! Unix-only: the scripted agent uses `sh` (CI runs on Linux; Windows skips).
#![cfg(unix)]

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// Build (debug) and return the path to a `kriya` binary via the note-app-style invocation.
fn build_bin(name: &str) -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--bin", name])
        .status()
        .expect("cargo build should run");
    assert!(status.success(), "{name} must build");
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push(format!("target/debug/{name}"));
    assert!(path.is_file(), "expected binary at {}", path.display());
    path
}

fn sandbox() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kriya-ci-smoke-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a scripted "agent": pipe `check`+`record` for each `(action, success)` through kriya-govern,
/// which the parent (`kriya-ci`) points at the shared policy + log via env. Records a blocked attempt
/// (success:false) on a deny — the "attempts are evidence" discipline the CI gate reads.
fn write_agent(dir: &PathBuf, govern: &PathBuf, calls: &[(&str, bool)]) -> PathBuf {
    let mut script = String::from("#!/bin/sh\nset -e\n");
    for (action, success) in calls {
        script.push_str(&format!(
            "printf '{{\"op\":\"check\",\"action_id\":\"{action}\",\"params\":{{}}}}\\n{{\"op\":\"record\",\"action_id\":\"{action}\",\"params\":{{}},\"success\":{success}}}\\n' \
             | \"{govern}\" --policy \"$KRIYA_POLICY\" --audit-log \"$KRIYA_AUDIT_LOG\" --actor ci-agent >/dev/null 2>&1\n",
            govern = govern.display(),
        ));
    }
    let path = dir.join("agent.sh");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(script.as_bytes()).unwrap();
    path
}

fn write_policy(dir: &PathBuf, yaml: &str) -> PathBuf {
    let p = dir.join("policy.yaml");
    std::fs::write(&p, yaml).unwrap();
    p
}

/// Run `kriya-ci run` over the agent script; return (exit_code, receipt_lines).
fn run_ci(ci: &PathBuf, policy: &PathBuf, log: &PathBuf, agent: &PathBuf) -> (i32, Vec<String>) {
    let status = Command::new(ci)
        .args(["run", "--policy"])
        .arg(policy)
        .arg("--audit-log")
        .arg(log)
        .args(["--", "sh"])
        .arg(agent)
        .status()
        .expect("spawn kriya-ci");
    let lines = std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    (status.code().unwrap_or(-1), lines)
}

#[test]
fn all_allowed_step_is_clean_exit_zero_with_verifiable_receipts() {
    let ci = build_bin("kriya-ci");
    let govern = build_bin("kriya-govern");
    let dir = sandbox();
    let policy = write_policy(
        &dir,
        "rules:\n  - { action: \"read_file\", allow: true }\n  - { action: \"list_dir\", allow: true }\n  - { action: \"*\", allow: false }\nbudget:\n  max_actions_per_minute: 120\n",
    );
    let agent = write_agent(&dir, &govern, &[("read_file", true), ("list_dir", true)]);
    let log = dir.join("receipts.jsonl");

    let (code, lines) = run_ci(&ci, &policy, &log, &agent);
    assert_eq!(code, 0, "an all-allowed governed step must exit 0 CLEAN");
    assert_eq!(lines.len(), 2, "two allowed actions → two signed receipts");
    // The receipts are a real hash chain (line 2 carries prev_hash) — the artifact verify-receipts re-proves.
    let v2: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    assert!(
        v2["prev_hash"].is_string(),
        "receipts are hash-chained evidence"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_policy_blocked_action_fails_the_job_exit_three() {
    let ci = build_bin("kriya-ci");
    let govern = build_bin("kriya-govern");
    let dir = sandbox();
    // read_file allowed; wire_funds falls through to deny-by-default. The agent attempts both.
    let policy = write_policy(
        &dir,
        "rules:\n  - { action: \"read_file\", allow: true }\n  - { action: \"*\", allow: false }\nbudget:\n  max_actions_per_minute: 120\n",
    );
    let agent = write_agent(&dir, &govern, &[("read_file", true), ("wire_funds", false)]);
    let log = dir.join("receipts.jsonl");

    let (code, lines) = run_ci(&ci, &policy, &log, &agent);
    assert_eq!(
        code, 3,
        "a policy-denied governed action must fail the job (exit 3)"
    );
    // The blocked attempt is recorded evidence (the gate saw a receipt for a denied action).
    assert!(
        lines.iter().any(|l| l.contains("wire_funds")),
        "the blocked attempt must be in the signed evidence"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_policy_fails_closed_exit_four() {
    let ci = build_bin("kriya-ci");
    let dir = sandbox();
    let missing = dir.join("does-not-exist.yaml");
    let log = dir.join("receipts.jsonl");
    // `true` is a trivial agent — but we never reach it: the missing policy must fail closed first.
    let status = Command::new(&ci)
        .args(["run", "--policy"])
        .arg(&missing)
        .arg("--audit-log")
        .arg(&log)
        .args(["--", "true"])
        .status()
        .expect("spawn kriya-ci");
    assert_eq!(
        status.code(),
        Some(4),
        "a missing policy must fail closed (exit 4), never run permissively"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
