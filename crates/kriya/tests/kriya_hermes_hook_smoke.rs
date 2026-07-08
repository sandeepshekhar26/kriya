//! End-to-end smoke test for `kriya-hermes-hook` (doc 21 Part B requirement: "ship a smoke test
//! that actually blocks a `hermes terminal` call"). Builds the real binary, spawns it as a
//! subprocess exactly as Hermes' `agent/shell_hooks.py::_spawn` would (stdin JSON, read stdout),
//! and asserts on the real process boundary — not just in-process function calls.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Build (debug, `mcp-client` feature) and return the path to the compiled binary. Cached by
/// `cargo test` between calls in the same run since `cargo build` is a no-op when nothing changed.
fn build_binary() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--no-default-features",
            "--features",
            "mcp-client",
            "--bin",
            "kriya-hermes-hook",
        ])
        .status()
        .expect("cargo build should run");
    assert!(status.success(), "kriya-hermes-hook must build");

    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/debug/kriya-hermes-hook");
    assert!(path.is_file(), "expected binary at {}", path.display());
    path
}

/// Run the binary in `mode` ("pre"|"post") with `stdin_json` piped in, plus an isolated
/// `--audit-log`/`--signing-key`/`--policy` so this test never touches the real `~/.kriya`.
struct RunResult {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn run(bin: &PathBuf, mode: &str, extra_args: &[&str], stdin_json: &str) -> RunResult {
    let mut cmd = Command::new(bin);
    cmd.arg(mode).args(extra_args);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn kriya-hermes-hook");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_json.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait for child");
    RunResult {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// A fresh, uniquely-named temp dir per call. `cargo test` runs every test on its own thread
/// within ONE process, so keying only on `process::id()` (as the in-file unit tests do, one test
/// binary each) collides here — thread id is the per-test uniquifier this file needs.
fn sandbox() -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "kriya-hermes-hook-smoke-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (dir.join("audit.jsonl"), dir.join("signing.key"), dir.join("policy.yaml"))
}

/// The real Hermes stdin shape for `pre_tool_call` (per `agent/shell_hooks.py::_serialize_payload`,
/// verified against the live source, 2026-07-08): six fixed top-level keys.
fn pre_payload(tool_name: &str, tool_input: &str) -> String {
    format!(
        r#"{{"hook_event_name":"pre_tool_call","tool_name":"{tool_name}","tool_input":{tool_input},"session_id":"s1","cwd":"/tmp","extra":{{"task_id":"t1","tool_call_id":"c1","turn_id":1,"api_request_id":"r1"}}}}"#
    )
}

fn post_payload(tool_name: &str, status: &str) -> String {
    format!(
        r#"{{"hook_event_name":"post_tool_call","tool_name":"{tool_name}","tool_input":{{}},"session_id":"s1","cwd":"/tmp","extra":{{"status":"{status}","result":"ok","duration_ms":12}}}}"#
    )
}

#[test]
fn pre_hook_actually_blocks_a_terminal_call_end_to_end() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    // A policy that denies terminal outright.
    std::fs::write(&policy, "rules:\n  - { action: \"hermes__terminal\", allow: false }\n").unwrap();

    let r = run(
        &bin,
        "pre",
        &[
            "--policy",
            policy.to_str().unwrap(),
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &pre_payload("terminal", r#"{"command":"rm -rf /"}"#),
    );

    assert!(r.status.success(), "must exit 0 — Hermes ignores exit code, stdout is the gate");
    let v: serde_json::Value = serde_json::from_str(r.stdout.trim())
        .unwrap_or_else(|e| panic!("stdout must be the block JSON, got {:?}: {e}", r.stdout));
    assert_eq!(v["action"], "block", "the Hermes-canonical block directive");
    assert!(v["message"].as_str().unwrap().contains("denied"));
    assert!(!r.stderr.is_empty(), "the reason is also echoed to stderr for a human tailing logs");

    // The blocked attempt is itself a signed receipt.
    let text = std::fs::read_to_string(&log).unwrap();
    let receipt: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(receipt["action_id"], "hermes__terminal");
    assert_eq!(receipt["success"], false);
    assert!(receipt["signature"].is_string());

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn pre_hook_allows_and_signs_nothing_when_policy_permits() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, "rules:\n  - { action: \"*\", allow: true }\n").unwrap();

    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("read_file", r#"{"path":"a.txt"}"#),
    );

    assert!(r.status.success());
    assert!(r.stdout.trim().is_empty(), "an allow prints nothing — no block directive");
    assert!(!log.exists() || std::fs::read_to_string(&log).unwrap().trim().is_empty(), "allow defers signing to post");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn post_hook_signs_the_real_outcome_from_extra_status() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();

    let r_ok = run(
        &bin,
        "post",
        &["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &post_payload("write_file", "ok"),
    );
    assert!(r_ok.status.success());

    let r_err = run(
        &bin,
        "post",
        &["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &post_payload("write_file", "error"),
    );
    assert!(r_err.status.success());

    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    let v1: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let v2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(v1["success"], true, "extra.status=ok → success");
    assert_eq!(v2["success"], false, "extra.status=error → failure");
    // Chained across two fresh processes (mirrors production: one process per hook fire).
    assert!(v2["prev_hash"].is_string());

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
    let _ = policy;
}
