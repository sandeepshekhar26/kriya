//! End-to-end smoke test for `kriya-hook` (doc 22 §11-B0: the founder-reported enforcement bug —
//! a deny policy did not block Claude Code, and the approval tier surfaced no prompt). Builds the
//! real binary, spawns it as a subprocess exactly as Claude Code's `PreToolUse`/`PostToolUse` hook
//! runner would (stdin JSON, read exit code + stdout/stderr), and asserts on the real process
//! boundary — not just in-process function calls (the existing unit tests in kriya-hook.rs never
//! spawn the compiled binary, so this is genuinely new coverage). Mirrors
//! `kriya_hermes_hook_smoke.rs`'s self-contained pattern.
//!
//! The root cause of the B0 bug was never in this binary: `Policy::check`/`Governor::dispatch`
//! gate correctly whenever a policy is actually supplied (proven below). The bug was that the
//! Console never passed `--policy`/`--approval` when installing the hook, so every
//! Console-installed hook silently ran the permissive built-in default. `no_policy_flag_means_
//! silent_allow_this_is_the_historical_bug_shape` below locks in *why* that Console-side fix
//! (kriya-console's `govern.rs`) is necessary, from this repo's side of the seam.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Build (debug) and return the path to the compiled binary. `--features mcp-client` costs
/// nothing today (kriya-hook builds fine under default features) and keeps this file's build
/// invocation consistent with `kriya_hermes_hook_smoke.rs`'s, so a future dependency shared
/// between the two hook binaries doesn't silently split the two test files' build commands.
fn build_binary() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--no-default-features",
            "--features",
            "mcp-client",
            "--bin",
            "kriya-hook",
        ])
        .status()
        .expect("cargo build should run");
    assert!(status.success(), "kriya-hook must build");

    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/debug/kriya-hook");
    assert!(path.is_file(), "expected binary at {}", path.display());
    path
}

struct RunResult {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn run(bin: &PathBuf, mode: &str, extra_args: &[&str], stdin_json: &str) -> RunResult {
    let mut cmd = Command::new(bin);
    cmd.arg(mode).args(extra_args);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn kriya-hook");
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

/// A fresh, uniquely-named temp dir per call — thread id is the per-test uniquifier since
/// `cargo test` runs every test on its own thread within one process (mirrors the Hermes file).
fn sandbox() -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "kriya-hook-smoke-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (
        dir.join("audit.jsonl"),
        dir.join("signing.key"),
        dir.join("policy.yaml"),
    )
}

/// The real Claude Code `PreToolUse` stdin shape (see kriya-hook.rs's own module doc: `tool_name`,
/// `tool_input`, and on PostToolUse `tool_response`).
fn pre_payload(tool_name: &str, tool_input: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"{tool_name}","tool_input":{tool_input},"session_id":"s1"}}"#
    )
}

fn post_payload(tool_name: &str, tool_response: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PostToolUse","tool_name":"{tool_name}","tool_input":{{}},"tool_response":{tool_response},"session_id":"s1"}}"#
    )
}

fn read_receipts(log: &PathBuf) -> Vec<serde_json::Value> {
    match std::fs::read_to_string(log) {
        Ok(text) => text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
        Err(_) => Vec::new(),
    }
}

// --- The failing-repro-first cases (B0) ----------------------------------------------------

#[test]
fn pre_hook_blocks_and_signs_a_receipt_on_deny() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__bash\", allow: false }\n",
    )
    .unwrap();

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
        &pre_payload("Bash", r#"{"command":"rm -rf /tmp/whatever"}"#),
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "deny must block via exit 2 (Claude Code's PreToolUse blocking contract)"
    );
    assert!(
        r.stderr.contains("denied"),
        "the reason must be on stderr so Claude sees why: {:?}",
        r.stderr
    );

    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1, "the blocked attempt is itself evidence");
    assert_eq!(receipts[0]["action_id"], "claude-code__bash");
    assert_eq!(receipts[0]["success"], false);
    assert!(receipts[0]["signature"].is_string());

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// Locks in the ROOT CAUSE at the binary level: a hook invoked exactly as an unfixed Console
/// install would (no `--policy`, no `--approval` — see kriya-console's `govern.rs::hook_group`
/// pre-fix) silently falls back to the permissive built-in default and allows a tool call the
/// operator's intent (expressed only in the Console's Policy view, never wired to this process)
/// was to deny. This is not a bug in this binary — `load_policy`'s record-only fallback is
/// intentional (doc 19: never brick a bare/manual install) — but it is the exact shape of the
/// founder-reported bug, and it is why the Console-side fix must always pass `--policy`.
#[test]
fn no_policy_flag_means_silent_allow_this_is_the_historical_bug_shape() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();

    // No --policy at all — exactly the historical (pre-fix) Console-generated command shape.
    let r = run(
        &bin,
        "pre",
        &[
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &pre_payload("Bash", r#"{"command":"rm -rf /tmp/whatever"}"#),
    );

    assert_eq!(
        r.status.code(),
        Some(0),
        "with no --policy, the built-in default is allow-all — the tool runs"
    );
    assert!(
        read_receipts(&log).is_empty(),
        "an allow at pre-stage signs nothing yet"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// The other half of the founder's report: a `RequiresApproval` decision, with the Console's
/// pre-fix install (no `--approval` flag either), silently falls back to kriya-hook's own
/// hardcoded `deny` default — the tool is blocked, but through ZERO interactive surface: no tty
/// prompt, no GUI dialog, nothing attempted. That is indistinguishable, from the operator's chair,
/// from "the approval tier didn't do anything" — exactly the reported symptom.
#[test]
fn approval_tier_with_no_approval_flag_denies_silently_with_zero_interactive_surface() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__bash\", allow: true, require_approval: true }\n",
    )
    .unwrap();

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
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "no --approval flag -> kriya-hook's own hardcoded 'deny' default"
    );
    assert!(
        r.stderr.contains("approval mode: deny"),
        "confirms the silent fallback, not an explicit operator choice: {:?}",
        r.stderr
    );
    assert_eq!(
        read_receipts(&log).len(),
        1,
        "the silent denial is still receipted"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn approval_tier_with_explicit_auto_mode_allows_and_signs_nothing_at_pre_stage() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__bash\", allow: true, require_approval: true }\n",
    )
    .unwrap();

    let r = run(
        &bin,
        "pre",
        &[
            "--policy",
            policy.to_str().unwrap(),
            "--approval",
            "auto",
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );

    assert_eq!(r.status.code(), Some(0));
    assert!(read_receipts(&log).is_empty());

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
        &[
            "--policy",
            policy.to_str().unwrap(),
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &pre_payload("Read", r#"{"file_path":"a.txt"}"#),
    );

    assert_eq!(r.status.code(), Some(0));
    assert!(r.stdout.trim().is_empty());
    assert!(
        read_receipts(&log).is_empty(),
        "allow defers signing to post"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn post_hook_signs_the_real_outcome() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();

    let r_ok = run(
        &bin,
        "post",
        &[
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &post_payload("Write", r#"{"success":true}"#),
    );
    assert_eq!(r_ok.status.code(), Some(0));

    let r_err = run(
        &bin,
        "post",
        &[
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &post_payload("Write", r#"{"success":false}"#),
    );
    assert_eq!(r_err.status.code(), Some(0));

    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0]["success"], true);
    assert_eq!(receipts[1]["success"], false);
    assert!(
        receipts[1]["prev_hash"].is_string(),
        "chained across two fresh processes"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

// --- S3 run correlation (W0-3): the REAL binary stamps kriya.corr from the pinned payload -----

/// Load a pinned hook-contract fixture (the W0-3 payload shapes).
fn contract_fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/hook-contract");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read fixture {}: {e}", p.display()))
}

/// A subagent-spawning session, driven through the REAL compiled binary: the main agent's `Task`
/// spawn (session_id, no agent_id) and a subagent's `Bash` call (same session_id, distinct
/// agent_id, no parent pointer). Both must carry `kriya.corr.run_id = the shared session`, the
/// subagent receipt must additionally carry `agent_id` and NEVER a fabricated `parent_step_id`, and
/// the tool arguments must survive untouched alongside the reserved key. This is the run-correlation
/// substrate the Console session-tree is built from — proven end-to-end at the process boundary, not
/// just in a unit test. If the pinned contract (session_id / agent_id field names) ever drifts, this
/// breaks loudly instead of silently emitting no correlation.
#[test]
fn post_hook_stamps_run_correlation_from_the_pinned_w03_contract() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();

    // The pinned fixtures ARE the contract — assert the load-bearing fields are present so a drift
    // in the committed fixture can't quietly weaken this test.
    let sub_payload = contract_fixture("posttooluse-subagent.json");
    let main_payload = contract_fixture("posttooluse-main.json");
    let sub_json: serde_json::Value = serde_json::from_str(&sub_payload).unwrap();
    assert_eq!(
        sub_json["session_id"], "sess-w03-A",
        "run scope field pinned"
    );
    assert_eq!(
        sub_json["agent_id"], "subagent-explore-1",
        "subagent field pinned"
    );
    assert!(
        sub_json.get("parent_session_id").is_none() && sub_json.get("parent_tool_use_id").is_none(),
        "the contract carries NO parent pointer — the hook must not invent one"
    );

    let args = [
        "--audit-log",
        log.to_str().unwrap(),
        "--signing-key",
        key.to_str().unwrap(),
    ];
    // Main agent spawns the subagent (Task), then the subagent runs its Bash call.
    assert_eq!(
        run(&bin, "post", &args, &main_payload).status.code(),
        Some(0)
    );
    assert_eq!(
        run(&bin, "post", &args, &sub_payload).status.code(),
        Some(0)
    );

    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 2, "one receipt per tool call");

    // Receipt 0: the main agent's Task spawn — run scoped, no agent_id (absent in payload), no parent.
    let main = &receipts[0];
    assert_eq!(main["action_id"], "claude-code__task");
    assert_eq!(main["params"]["kriya.corr"]["run_id"], "sess-w03-A");
    assert!(
        main["params"]["kriya.corr"].get("agent_id").is_none(),
        "main-agent payload has no agent_id → none is stamped (honest)"
    );
    assert!(main["params"]["kriya.corr"].get("parent_step_id").is_none());
    assert_eq!(
        main["params"]["subagent_type"], "Explore",
        "tool args survive"
    );

    // Receipt 1: the subagent's Bash — same run, distinct agent_id, NO parent_step_id.
    let sub = &receipts[1];
    assert_eq!(sub["action_id"], "claude-code__bash");
    assert_eq!(
        sub["params"]["kriya.corr"]["run_id"], "sess-w03-A",
        "the subagent shares the parent's session as one run"
    );
    assert_eq!(
        sub["params"]["kriya.corr"]["agent_id"],
        "subagent-explore-1"
    );
    assert!(
        sub["params"]["kriya.corr"].get("parent_step_id").is_none(),
        "the hook lane has no parent pointer — it must never fabricate one"
    );
    assert_eq!(sub["params"]["command"], "echo hello-from-subagent");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

// --- Regression matrix: {allow, approval, deny} x {built-in tool, mcp__ tool} ---------------

#[test]
fn regression_matrix_allow_approval_deny_across_builtin_and_mcp_shaped_actions() {
    let bin = build_binary();

    struct Case {
        label: &'static str,
        tool_name: &'static str,
        policy_yaml: &'static str,
        expect_exit: i32,
    }
    let cases = [
        Case { label: "builtin/allow", tool_name: "Bash", policy_yaml: "rules:\n  - { action: \"claude-code__bash\", allow: true }\n", expect_exit: 0 },
        Case { label: "builtin/deny", tool_name: "Bash", policy_yaml: "rules:\n  - { action: \"claude-code__bash\", allow: false }\n", expect_exit: 2 },
        Case { label: "builtin/approval-denied-by-default", tool_name: "Bash", policy_yaml: "rules:\n  - { action: \"claude-code__bash\", allow: true, require_approval: true }\n", expect_exit: 2 },
        Case {
            label: "mcp/allow",
            tool_name: "mcp__github__create_issue",
            policy_yaml: "rules:\n  - { action: \"claude-code__mcp__github__*\", allow: true }\n",
            expect_exit: 0,
        },
        Case {
            label: "mcp/deny",
            tool_name: "mcp__github__create_issue",
            policy_yaml: "rules:\n  - { action: \"claude-code__mcp__github__*\", allow: false }\n",
            expect_exit: 2,
        },
        Case {
            label: "mcp/approval-denied-by-default",
            tool_name: "mcp__github__create_issue",
            policy_yaml: "rules:\n  - { action: \"claude-code__mcp__github__*\", allow: true, require_approval: true }\n",
            expect_exit: 2,
        },
    ];

    for case in cases {
        let (log, key, policy) = sandbox();
        std::fs::write(&policy, case.policy_yaml).unwrap();
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
            &pre_payload(case.tool_name, "{}"),
        );
        assert_eq!(
            r.status.code(),
            Some(case.expect_exit),
            "case {}: exit code mismatch (stderr: {:?})",
            case.label,
            r.stderr
        );
        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }
}

// --- Fail-closed on internal errors (proves the "internal error -> deny" invariant) ---------

#[test]
fn pre_hook_fails_closed_on_missing_policy_file() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();
    let missing = log.parent().unwrap().join("does-not-exist.yaml");

    let r = run(
        &bin,
        "pre",
        &[
            "--policy",
            missing.to_str().unwrap(),
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &pre_payload("Bash", "{}"),
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "an unreadable policy must block, not silently allow"
    );
    assert!(
        r.stderr.contains("cannot read policy") || r.stderr.contains("blocking"),
        "{:?}",
        r.stderr
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn pre_hook_fails_closed_on_malformed_stdin() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, "rules:\n  - { action: \"*\", allow: true }\n").unwrap();

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
        "this is not json",
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "malformed payload must block, not silently allow"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}
