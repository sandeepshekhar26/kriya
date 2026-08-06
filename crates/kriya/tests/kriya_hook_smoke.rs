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

/// C2 (docs/design/c2-budget-gate-rereview.md F-C1 build note): a budget-gate test needs its state
/// dir to resolve HERMETICALLY (`<audit-dir>/../state`, never colliding with another parallel test
/// or the operator's real `~/.kriya/state`). The plain flat `sandbox()` above puts the log directly
/// at `<dir>/audit.jsonl`, so `<audit-dir>/../state` would resolve OUTSIDE `<dir>` entirely. Nesting
/// the log under an `audit/` subdir (mirroring production's `~/.kriya/audit/claude-code.jsonl`
/// layout) makes `<audit-dir>/../state` land at a per-sandbox `<dir>/state` instead. Returns
/// `(log, key, policy, state_dir)`.
fn sandbox_with_state_dir() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "kriya-hook-smoke-budget-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("audit")).unwrap();
    (
        dir.join("audit").join("claude-code.jsonl"),
        dir.join("signing.key"),
        dir.join("policy.yaml"),
        dir.join("state"),
    )
}

/// Write `<state_dir>/spend-live.json` with ONE session's pre-seal running total — the minimal C1
/// heartbeat fixture a budget-gate smoke test needs (never touches the operator's real state dir).
fn write_live_state(state_dir: &PathBuf, session_id: &str, observed_usd: f64, as_of_ms: u64) {
    std::fs::create_dir_all(state_dir).unwrap();
    let body = serde_json::json!({
        "v": 1,
        "as_of_ms": as_of_ms,
        "pricing_sheet": "kriya-pricing-test",
        "pricing_sheet_hash": "deadbeef",
        "sessions": { session_id: { "observed_usd": observed_usd, "as_of_ms": as_of_ms } },
        "rolling_day": { "observed_usd": observed_usd, "as_of_ms": as_of_ms },
        "user": { "observed_usd": observed_usd, "as_of_ms": as_of_ms },
    });
    std::fs::write(state_dir.join("spend-live.json"), body.to_string()).unwrap();
}

/// The real wall-clock epoch-ms "now" — the hook's own budget consult calls `now_ms()` for real, so
/// a fixture that wants to look FRESH (not stale) must carry an `as_of_ms` close to actual now, not
/// an arbitrary small constant.
fn fresh_as_of_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
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

/// Like [`post_payload`], but with an explicit `tool_input` and `session_id` — D1's memory-write
/// receipt fixtures need real `file_path`/`content` shapes and a stable run id to join on.
fn post_payload_full(tool_name: &str, tool_input: &str, tool_response: &str, session_id: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PostToolUse","tool_name":"{tool_name}","tool_input":{tool_input},"tool_response":{tool_response},"session_id":"{session_id}"}}"#
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

    // A4: each `post` invocation also emits one `kriya.crypto.module` self-attestation ahead of
    // the action receipt (docs/design/a4-fips-lane.md D2) — additive and expected, so the action
    // receipts are located by `action_id` rather than raw position.
    let all_receipts = read_receipts(&log);
    assert_eq!(all_receipts.len(), 4, "attest+action per post invocation, twice");
    let receipts: Vec<_> = all_receipts
        .iter()
        .filter(|r| r["action_id"] != "kriya.crypto.module")
        .cloned()
        .collect();
    assert_eq!(receipts.len(), 2, "two action receipts");
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

    // A4: each `post` invocation also emits one `kriya.crypto.module` self-attestation ahead of
    // the action receipt (docs/design/a4-fips-lane.md D2) — additive and expected, so the action
    // receipts are located by `action_id` rather than raw position.
    let all_receipts = read_receipts(&log);
    assert_eq!(all_receipts.len(), 4, "attest+action per post invocation, twice");
    let receipts: Vec<_> = all_receipts
        .iter()
        .filter(|r| r["action_id"] != "kriya.crypto.module")
        .cloned()
        .collect();
    assert_eq!(receipts.len(), 2, "one action receipt per tool call");

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

// --- C2 (doc 27 §4 / docs/design/c2-budget-gate.md) — the endpoint budget gate ------------------
// Real-process rows extending the B0 matrix (never weakening it): budget-breach blocks, approval
// routes, fail-closed on missing/stale state, and — the airtight F-B1 regression — a budget deny
// short-circuits BEFORE the credential-brokering block, so no Keychain secret is ever read/injected
// for an action a budget is about to deny.

fn budget_deny_policy_yaml(threshold_usd: f64) -> String {
    format!(
        "rules:\n  - {{ action: \"claude-code__bash\", allow: true }}\nbudgets:\n  rules:\n    - {{ id: \"session-cap\", scope: session, threshold_usd: {threshold_usd}, action: deny }}\n"
    )
}

#[test]
fn budget_deny_blocks_an_otherwise_allowed_action_and_signs_a_gate_deny_receipt() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, budget_deny_policy_yaml(0.01)).unwrap();
    let as_of = fresh_as_of_ms();
    write_live_state(&state_dir, "s1", 0.02, as_of);

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
        "a deny-tier budget breach must block via exit 2: stderr={:?}",
        r.stderr
    );
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1, "the blocked attempt is itself evidence");
    assert_eq!(receipts[0]["action_id"], "kriya.spend.gate.deny");
    assert_eq!(receipts[0]["success"], false);
    assert_eq!(receipts[0]["params"]["budget_id"], "session-cap");
    assert_eq!(receipts[0]["params"]["threshold_usd"], 0.01);
    assert_eq!(receipts[0]["params"]["observed_usd"], 0.02);
    assert_eq!(receipts[0]["params"]["state_source"], "live-tick");
    assert_eq!(receipts[0]["params"]["state_as_of_ms"], as_of);
    assert_eq!(receipts[0]["params"]["state_stale"], false);

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn budget_deny_is_never_reached_when_the_action_tier_already_allows_and_stays_under_threshold() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, budget_deny_policy_yaml(5.0)).unwrap();
    write_live_state(&state_dir, "s1", 0.02, fresh_as_of_ms());

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

    assert_eq!(r.status.code(), Some(0));
    assert!(read_receipts(&log).is_empty(), "under threshold — no gate receipt");

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn budget_require_approval_denies_when_not_granted_and_allows_when_auto_approved() {
    let bin = build_binary();
    let policy_yaml = "rules:\n  - { action: \"claude-code__bash\", allow: true }\nbudgets:\n  rules:\n    - { id: \"daily-cap\", scope: rolling-day, threshold_usd: 0.01, action: require-approval }\n";

    // Not granted (default --approval deny) -> blocks, signs gate.approval.
    {
        let (log, key, policy, state_dir) = sandbox_with_state_dir();
        std::fs::write(&policy, policy_yaml).unwrap();
        write_live_state(&state_dir, "s1", 1.0, fresh_as_of_ms());
        let r = run(
            &bin,
            "pre",
            &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
            &pre_payload("Bash", r#"{"command":"echo hi"}"#),
        );
        assert_eq!(r.status.code(), Some(2));
        let receipts = read_receipts(&log);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0]["action_id"], "kriya.spend.gate.approval");
        assert_eq!(receipts[0]["success"], false);
        let _ = std::fs::remove_dir_all(&state_dir);
        let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
    }

    // Granted (--approval auto) -> proceeds, signs nothing at pre-stage (mirrors the action tier's
    // own "cleared approval signs nothing yet" convention).
    {
        let (log, key, policy, state_dir) = sandbox_with_state_dir();
        std::fs::write(&policy, policy_yaml).unwrap();
        write_live_state(&state_dir, "s1", 1.0, fresh_as_of_ms());
        let r = run(
            &bin,
            "pre",
            &["--policy", policy.to_str().unwrap(), "--approval", "auto", "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
            &pre_payload("Bash", r#"{"command":"echo hi"}"#),
        );
        assert_eq!(r.status.code(), Some(0));
        assert!(read_receipts(&log).is_empty());
        let _ = std::fs::remove_dir_all(&state_dir);
        let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
    }
}

#[test]
fn budget_warn_never_blocks_but_still_signs_a_success_true_receipt() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__bash\", allow: true }\nbudgets:\n  rules:\n    - { id: \"watch\", scope: user, threshold_usd: 0.01, action: warn }\n",
    )
    .unwrap();
    write_live_state(&state_dir, "s1", 1.0, fresh_as_of_ms());

    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );

    assert_eq!(r.status.code(), Some(0), "warn is observe-only, never blocks");
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["action_id"], "kriya.spend.gate.warn");
    assert_eq!(receipts[0]["success"], true);

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn budget_deny_fails_closed_on_missing_state_never_silently_allows() {
    let bin = build_binary();
    // No spend-live.json written at all — the state dir doesn't even exist.
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, budget_deny_policy_yaml(0.01)).unwrap();

    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );

    assert_eq!(r.status.code(), Some(2), "missing state on a deny rule must fail CLOSED");
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["action_id"], "kriya.spend.gate.deny");
    assert_eq!(receipts[0]["params"]["state_source"], "none");
    assert_eq!(receipts[0]["params"]["state_stale"], true);
    assert_eq!(receipts[0]["params"]["on_missing_state"], "fail-closed");

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn budget_deny_fails_closed_on_stale_state_older_than_max_staleness_secs() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__bash\", allow: true }\nbudgets:\n  max_staleness_secs: 60\n  rules:\n    - { id: \"session-cap\", scope: session, threshold_usd: 0.01, action: deny }\n",
    )
    .unwrap();
    // as_of_ms = 0 -> ~now (well over 60s) later, this state is stale by the time the hook runs.
    write_live_state(&state_dir, "s1", 0.02, 0);

    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );

    assert_eq!(r.status.code(), Some(2), "stale state on a deny rule must fail CLOSED");
    let receipts = read_receipts(&log);
    assert_eq!(receipts[0]["action_id"], "kriya.spend.gate.deny");
    assert_eq!(receipts[0]["params"]["state_stale"], true);

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

/// **THE F-B1 regression (the B0 fix, at the real process boundary).** A `$0.01` `deny` budget PLUS
/// a `secrets:`-configured policy PLUS a `{{kriya:alias}}` placeholder in `tool_input` must exit 2
/// with a signed `kriya.spend.gate.deny` receipt and print NO `updatedInput` on stdout — proving the
/// budget consult fires BEFORE the credential-brokering block's early `return ExitCode::SUCCESS`
/// (`kriya-hook.rs:767`), so no Keychain secret is ever read or injected for an action the budget is
/// about to deny. If the consult were ever moved after the brokering block (the pre-review draft's
/// mistake), this call would instead attempt Keychain substitution — the exact bug this row guards.
#[test]
fn f_b1_budget_deny_short_circuits_before_credential_brokering_no_secret_injected() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__webfetch\", allow: true }\n\
         budgets:\n  rules:\n    - { id: \"session-cap\", scope: session, threshold_usd: 0.01, action: deny }\n\
         secrets:\n  aliases:\n    - alias: \"my-token\"\n      keychain_service: \"kriya-f-b1-test-service\"\n      keychain_account: \"kriya-f-b1-test-account\"\n      allowed_hosts:\n        - \"api.example.com\"\n",
    )
    .unwrap();
    write_live_state(&state_dir, "s1", 0.02, fresh_as_of_ms());

    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload(
            "WebFetch",
            r#"{"url":"https://api.example.com/data","headers":{"Authorization":"Bearer {{kriya:my-token}}"}}"#,
        ),
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "the budget deny must block before brokering ever runs: stderr={:?}",
        r.stderr
    );
    assert!(
        !r.stdout.contains("updatedInput"),
        "no updatedInput may ever be printed for a budget-denied call: stdout={:?}",
        r.stdout
    );
    assert!(
        !r.stdout.contains("{{kriya:"),
        "no placeholder-bearing output either — nothing was ever substituted"
    );
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1, "exactly the gate.deny receipt — no brokering-deny, no action receipt");
    assert_eq!(receipts[0]["action_id"], "kriya.spend.gate.deny");
    assert_eq!(receipts[0]["success"], false);

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

// =============================================================================================
// D1 (doc 27 §4 / docs/design/d1-memory-receipts.md) — memory-write receipts, end-to-end through
// the real compiled `kriya-hook` binary (§5 test-plan fixtures a/b/c, at the process boundary).
// =============================================================================================

fn memory_receipt<'a>(receipts: &'a [serde_json::Value]) -> &'a serde_json::Value {
    receipts
        .iter()
        .find(|r| r["action_id"].as_str().unwrap_or("").starts_with("kriya.memory."))
        .expect("expected a kriya.memory.* receipt")
}

#[test]
fn claude_md_write_emits_a_kriya_memory_write_receipt_class_claude_md() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();

    let r = run(
        &bin,
        "post",
        &["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &post_payload_full(
            "Write",
            r#"{"file_path":"CLAUDE.md","content":"Standing instructions: always verify before claiming done."}"#,
            r#"{"success":true}"#,
            "sess-d1-a",
        ),
    );
    assert_eq!(r.status.code(), Some(0), "stderr={}", r.stderr);

    let receipts = read_receipts(&log);
    let mem = memory_receipt(&receipts);
    assert_eq!(mem["action_id"], "kriya.memory.write");
    assert_eq!(mem["success"], true);
    let m = &mem["params"]["kriya.memory"];
    assert_eq!(m["class"], "claude-md");
    assert_eq!(m["surface"], "file");
    assert_eq!(m["verb_basis"], "tool-write");
    assert_eq!(m["path"], "CLAUDE.md");
    assert!(m["path_hmac"].as_str().unwrap().len() == 64);
    assert_eq!(mem["params"]["kriya.corr"]["run_id"], "sess-d1-a");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn claude_memory_dir_edit_emits_a_kriya_memory_update_receipt() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();

    let r = run(
        &bin,
        "post",
        &["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &post_payload_full(
            "Edit",
            r#"{"file_path":"/Users/ci/.claude/projects/x/memory/MEMORY.md","old_string":"a","new_string":"b"}"#,
            r#"{"success":true}"#,
            "sess-d1-b",
        ),
    );
    assert_eq!(r.status.code(), Some(0), "stderr={}", r.stderr);

    let receipts = read_receipts(&log);
    let mem = memory_receipt(&receipts);
    assert_eq!(mem["action_id"], "kriya.memory.update");
    let m = &mem["params"]["kriya.memory"];
    assert_eq!(m["class"], "claude-memory-dir");
    assert_eq!(m["verb_basis"], "tool-edit");
    assert_eq!(m["root"], "user-home");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn claude_settings_write_emits_a_kriya_memory_write_receipt_class_claude_settings() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();

    let r = run(
        &bin,
        "post",
        &["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &post_payload_full(
            "Write",
            r#"{"file_path":".claude/settings.json","content":"{\"permissions\":{}}"}"#,
            r#"{"success":true}"#,
            "sess-d1-c",
        ),
    );
    assert_eq!(r.status.code(), Some(0), "stderr={}", r.stderr);

    let receipts = read_receipts(&log);
    let mem = memory_receipt(&receipts);
    assert_eq!(mem["action_id"], "kriya.memory.write");
    let m = &mem["params"]["kriya.memory"];
    assert_eq!(m["class"], "claude-settings");
    assert_eq!(m["root"], "project");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn a_second_write_to_the_same_path_in_the_same_run_upgrades_to_update_prior_write_seen() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();
    let args = ["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()];
    let write_payload = post_payload_full(
        "Write",
        r#"{"file_path":"CLAUDE.md","content":"first"}"#,
        r#"{"success":true}"#,
        "sess-d1-scan",
    );

    let r1 = run(&bin, "post", &args, &write_payload);
    assert_eq!(r1.status.code(), Some(0), "stderr={}", r1.stderr);
    let r2 = run(&bin, "post", &args, &write_payload);
    assert_eq!(r2.status.code(), Some(0), "stderr={}", r2.stderr);

    let receipts = read_receipts(&log);
    let mem_receipts: Vec<_> = receipts
        .iter()
        .filter(|r| r["action_id"].as_str().unwrap_or("").starts_with("kriya.memory."))
        .collect();
    assert_eq!(mem_receipts.len(), 2);
    assert_eq!(mem_receipts[0]["action_id"], "kriya.memory.write");
    assert_eq!(mem_receipts[0]["params"]["kriya.memory"]["verb_basis"], "tool-write");
    assert_eq!(
        mem_receipts[1]["action_id"], "kriya.memory.update",
        "the second write to the SAME path in the SAME run is honestly upgraded to update"
    );
    assert_eq!(mem_receipts[1]["params"]["kriya.memory"]["verb_basis"], "prior-write-seen");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn hash_not_content_the_written_sentinel_never_appears_in_the_memory_receipt() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();
    let sentinel = "TOP-SECRET-END-TO-END-SENTINEL-4a91";

    let r = run(
        &bin,
        "post",
        &["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &post_payload_full(
            "Write",
            &format!(r#"{{"file_path":"CLAUDE.md","content":"{sentinel}"}}"#),
            r#"{"success":true}"#,
            "sess-d1-hash",
        ),
    );
    assert_eq!(r.status.code(), Some(0), "stderr={}", r.stderr);

    let receipts = read_receipts(&log);
    let mem = memory_receipt(&receipts);
    let mem_wire = serde_json::to_string(mem).unwrap();
    assert!(!mem_wire.contains(sentinel), "the sentinel leaked into the memory receipt");
    let expected_hash = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(sentinel.as_bytes()))
    };
    assert_eq!(mem["params"]["kriya.memory"]["content_sha256"], expected_hash);
    assert_eq!(mem["params"]["kriya.memory"]["content_bytes"], sentinel.len());

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

#[test]
fn a_non_memory_file_write_emits_no_kriya_memory_receipt() {
    let bin = build_binary();
    let (log, key, _policy) = sandbox();

    let r = run(
        &bin,
        "post",
        &["--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &post_payload_full(
            "Write",
            r#"{"file_path":"src/main.rs","content":"fn main() {}"}"#,
            r#"{"success":true}"#,
            "sess-d1-nonmem",
        ),
    );
    assert_eq!(r.status.code(), Some(0), "stderr={}", r.stderr);
    let receipts = read_receipts(&log);
    assert!(
        !receipts.iter().any(|r| r["action_id"].as_str().unwrap_or("").starts_with("kriya.memory.")),
        "an ordinary source file write must never mint kriya.memory.*"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

// --- B4: temporal policy conditions (doc 27 §4 / docs/design/b4-temporal-conditions.md) --------
//
// The canonical demo (design §5 acceptance criterion 1): "deny git push unless npm test succeeded
// earlier this session." Uses `sandbox_with_state_dir()` — B4 reuses C2's `state_dir_for_audit_log`
// seam for its own session-cond cache, so it needs the SAME hermetic-state-dir layout the budget
// tests use (a flat `sandbox()` would resolve `<audit-dir>/../state` OUTSIDE the sandbox, into a
// path shared with other parallel tests).

/// The exact canonical rule from the design doc: action-tier allows Bash outright; the temporal
/// section denies a `git push` unless a `npm test` bash call SUCCEEDED earlier this session.
fn deny_push_without_tests_policy_yaml() -> String {
    "rules:\n  - { action: \"claude-code__bash\", allow: true }\n\
     temporal:\n  \
       rules:\n    \
         - id: \"deny-push-without-tests\"\n      \
           selector: { action: \"claude-code__bash\", command: { contains: \"git push\" } }\n      \
           tier: deny\n      \
           when:\n        \
             - predicate: succeeded\n          \
               selector: { action: \"claude-code__bash\", command: { contains: \"npm test\" } }\n          \
               expect: unsatisfied\n"
        .to_string()
}

#[test]
fn git_push_without_tests_demo_denies_then_allows_after_a_successful_test_this_session() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, deny_push_without_tests_policy_yaml()).unwrap();
    let args: Vec<&str> = vec![
        "--policy",
        policy.to_str().unwrap(),
        "--audit-log",
        log.to_str().unwrap(),
        "--signing-key",
        key.to_str().unwrap(),
    ];

    // 1) No prior test this session -> the push is DENIED, and the deny receipt shows WHY.
    let r1 = run(&bin, "pre", &args, &pre_payload("Bash", r#"{"command":"git push origin main"}"#));
    assert_eq!(r1.status.code(), Some(2), "no prior successful test this session must deny the push: stderr={:?}", r1.stderr);
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1, "the blocked attempt is itself evidence");
    let cond = &receipts[0];
    assert_eq!(cond["action_id"], "kriya.policy.cond.deny");
    assert_eq!(cond["success"], false);
    assert_eq!(cond["params"]["rule_id"], "deny-push-without-tests");
    assert_eq!(cond["params"]["action_id"], "claude-code__bash");
    assert_eq!(cond["params"]["tier"], "deny");
    assert_eq!(cond["params"]["index_source"], "rebuilt", "first-ever consult on this session: a cold-cache full rebuild");
    let conditions = cond["params"]["conditions"].as_array().unwrap();
    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0]["predicate"], "succeeded");
    assert_eq!(conditions[0]["expect"], "unsatisfied");
    assert_eq!(conditions[0]["observed"], false, "no npm test succeeded yet -> succeeded() observed false");
    assert_eq!(conditions[0]["match_count"], 0);
    assert_eq!(conditions[0]["result"], true, "observed(false) == expect(unsatisfied) -> this condition holds -> rule matches");
    // Privacy audit (D4): the agent's own command text must never appear in the receipt.
    let wire = serde_json::to_string(cond).unwrap();
    assert!(!wire.contains("origin main"), "the agent's push target leaked into the receipt: {wire}");

    // 2) Record a SUCCESSFUL `npm test` on the SAME session (the base action receipt post-mode
    //    writes — this is the receipt a real governed test run produces).
    let r_test = run(
        &bin,
        "post",
        &args,
        &post_payload_full("Bash", r#"{"command":"npm test"}"#, r#"{"success":true}"#, "s1"),
    );
    assert_eq!(r_test.status.code(), Some(0), "stderr={}", r_test.stderr);

    // 3) The SAME push now falls through to the base tier's allow — the temporal rule no longer
    //    matches (`succeeded(npm test)` is now true, so `expect: unsatisfied` is UNMET).
    let r2 = run(&bin, "pre", &args, &pre_payload("Bash", r#"{"command":"git push origin main"}"#));
    assert_eq!(r2.status.code(), Some(0), "a governed, successful test this session must clear the push: stderr={:?}", r2.stderr);
    // No NEW cond.deny receipt was appended by this second pre call.
    let receipts_after = read_receipts(&log);
    let cond_denies: Vec<_> = receipts_after.iter().filter(|r| r["action_id"] == "kriya.policy.cond.deny").collect();
    assert_eq!(cond_denies.len(), 1, "only the FIRST push attempt should have been denied");

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn temporal_deny_is_never_reached_when_the_subject_selector_does_not_match() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, deny_push_without_tests_policy_yaml()).unwrap();

    // A bash call that doesn't contain "git push" never even reaches the `when:` evaluation.
    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("Bash", r#"{"command":"ls -la"}"#),
    );
    assert_eq!(r.status.code(), Some(0));
    assert!(read_receipts(&log).is_empty(), "an unrelated command must sign nothing at pre-stage");

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

/// B0 matrix row: fail-closed on an UNREADABLE session log (D2 — genuinely "unavailable", never
/// merely "empty"). Simulated portably: chmod the log write-only (0200) so `session_cond`'s
/// `read_to_string` fails with permission-denied (not NotFound), while the signer's OWN append-only
/// `OpenOptions::new().append(true)` write still succeeds — exactly the asymmetry a real
/// unreadable-but-still-appendable log would have. Empirically verified on this filesystem before
/// writing this test.
#[test]
#[cfg(unix)]
fn temporal_deny_fails_closed_when_the_session_log_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;

    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, deny_push_without_tests_policy_yaml()).unwrap();
    std::fs::write(&log, "").unwrap(); // the log must exist as a FILE before we lock it down
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o200)).unwrap();

    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("Bash", r#"{"command":"git push origin main"}"#),
    );

    // Restore readability before asserting, regardless of outcome, so cleanup never leaks a
    // permission-locked file.
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(r.status.code(), Some(2), "an unreadable session log on a deny rule must fail CLOSED: stderr={:?}", r.stderr);
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1, "the write-only log could still be appended to");
    assert_eq!(receipts[0]["action_id"], "kriya.policy.cond.deny");
    assert_eq!(receipts[0]["params"]["index_source"], "unavailable");
    assert_eq!(receipts[0]["params"]["conditions"].as_array().unwrap().len(), 0, "the fold never ran — no condition trace to show");

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn temporal_tighten_only_a_base_deny_is_never_reopened_by_an_allow_tier_temporal_rule() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    // Action tier DENIES bash outright; an (deliberately misguided) temporal `allow` rule can never
    // re-open that — axiom 5. `tier: allow` never loosens (D3); this proves it end to end.
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__bash\", allow: false }\n\
         temporal:\n  rules:\n    - { id: \"loosen-attempt\", selector: { action: \"claude-code__bash\" }, tier: allow }\n",
    )
    .unwrap();

    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );
    assert_eq!(r.status.code(), Some(2), "the base action-tier deny must stand — a temporal allow rule is never consulted for an already-denied action");
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["action_id"], "claude-code__bash", "the base tier's own deny receipt, not a temporal one");

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn temporal_approval_tier_denies_when_not_granted_and_allows_when_auto_approved() {
    let bin = build_binary();
    let policy_yaml = "rules:\n  - { action: \"claude-code__bash\", allow: true }\n\
         temporal:\n  rules:\n    \
           - id: \"deploy-needs-review\"\n      \
             selector: { action: \"claude-code__bash\", command: { contains: \"deploy\" } }\n      \
             tier: approval\n      \
             when:\n        \
               - predicate: happened\n          \
                 selector: { action: \"claude-code__bash\", command: { contains: \"never-happened-xyz\" } }\n          \
                 expect: unsatisfied\n";

    // Not granted (default --approval deny) -> blocks, signs kriya.policy.cond.approval.
    {
        let (log, key, policy, state_dir) = sandbox_with_state_dir();
        std::fs::write(&policy, policy_yaml).unwrap();
        let r = run(
            &bin,
            "pre",
            &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
            &pre_payload("Bash", r#"{"command":"deploy prod"}"#),
        );
        assert_eq!(r.status.code(), Some(2));
        let receipts = read_receipts(&log);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0]["action_id"], "kriya.policy.cond.approval");
        assert_eq!(receipts[0]["success"], false);
        let _ = std::fs::remove_dir_all(&state_dir);
        let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
    }

    // Granted (--approval auto) -> proceeds, signs nothing at pre-stage.
    {
        let (log, key, policy, state_dir) = sandbox_with_state_dir();
        std::fs::write(&policy, policy_yaml).unwrap();
        let r = run(
            &bin,
            "pre",
            &["--policy", policy.to_str().unwrap(), "--approval", "auto", "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
            &pre_payload("Bash", r#"{"command":"deploy prod"}"#),
        );
        assert_eq!(r.status.code(), Some(0));
        assert!(read_receipts(&log).is_empty());
        let _ = std::fs::remove_dir_all(&state_dir);
        let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
    }
}

#[test]
fn temporal_cond_warn_is_reserved_not_emitted_in_v1() {
    // M-1: there is no hook wiring that could ever emit kriya.policy.cond.warn in v1 — this is a
    // structural/behavioral guard, not just a grep: an `allow`-tier match (the only way a v1
    // TemporalDecision could ever be "observe-only") always resolves to a silent Pass (proven at the
    // unit level in permissions::temporal_tests::a_matched_allow_tier_rule_is_always_a_no_op_pass).
    // Confirmed end-to-end here: with an allow-tier rule matching, and the log fully readable,
    // nothing is ever recorded.
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(
        &policy,
        "rules:\n  - { action: \"claude-code__bash\", allow: true }\ntemporal:\n  rules:\n    - { id: \"observe\", selector: { action: \"claude-code__bash\" }, tier: allow }\n",
    )
    .unwrap();
    let r = run(
        &bin,
        "pre",
        &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );
    assert_eq!(r.status.code(), Some(0));
    assert!(read_receipts(&log).is_empty(), "v1 has no path that emits kriya.policy.cond.warn");

    let _ = std::fs::remove_dir_all(&state_dir);
    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

// ─── F-2 action gates (kriya-console doc 31 §3.3 / the F2-gates design doc (kriya-console)) ───────────────
// The doc-22 B0 discipline applied to the gates dimension: prove at the COMPILED-BINARY boundary
// that a gate deny actually blocks in the Claude Code lane (exit 2 + signed evidence), that the
// receipt tier records-and-proceeds, and that the approve tier holds when no approval is granted.

fn gates_policy_yaml() -> &'static str {
    r#"
rules:
  - { action: "*", allow: true }
gates:
  rules:
    - class: self-mod
      rule_id: self-config-path
      tier: deny
      path_any: ["(?i)(^|/)\\.claude/settings[^/]*$|(^|/)\\.claude/hooks(/|$)|(^|/)\\.?mcp\\.json$"]
    - class: publish
      rule_id: npm-publish
      tier: approve
      command_any: ["\\b(npm|pnpm|yarn)\\s+publish\\b"]
    - class: destructive-git
      rule_id: git-force-push
      tier: receipt
      command_any: ["\\bgit\\s+push\\b[^|;&]*(\\s--force(-with-lease)?\\b|\\s-f\\b)"]
"#
}

/// B0 for gates: a self-mod gate deny BLOCKS the agent editing its own hooks/settings file (the
/// CurXecute vector, doc 30 §5) even though the action tier allows everything — and both the
/// blocked attempt's action receipt and the `kriya.gate.self-mod.denied` receipt are signed, the
/// gate receipt pointing at the action receipt via `corr_step` and carrying no file content.
#[test]
fn gates_self_mod_deny_blocks_hooks_file_edit_and_signs_gate_receipts() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, gates_policy_yaml()).unwrap();

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
        &pre_payload("Edit", r#"{"file_path":"/Users/x/.claude/settings.json","old_string":"a","new_string":"b"}"#),
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "a self-mod gate deny must block via exit 2 (B0): stderr={:?}",
        r.stderr
    );
    assert!(
        r.stderr.contains("self-mod") && r.stderr.contains("denied"),
        "stderr names the gate so Claude sees why: {:?}",
        r.stderr
    );

    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 2, "action receipt + gate receipt");
    assert_eq!(receipts[0]["action_id"], "claude-code__edit");
    assert_eq!(receipts[0]["success"], false);
    assert_eq!(receipts[1]["action_id"], "kriya.gate.self-mod.denied");
    assert_eq!(receipts[1]["success"], false);
    assert_eq!(receipts[1]["params"]["class"], "self-mod");
    assert_eq!(receipts[1]["params"]["rule_id"], "self-config-path");
    assert_eq!(receipts[1]["params"]["tier"], "deny");
    assert_eq!(receipts[1]["params"]["matcher_kind"], "path");
    assert_eq!(receipts[1]["params"]["corr_step"], receipts[0]["step_id"]);
    assert!(
        !receipts[1]["params"].to_string().contains("settings.json"),
        "the gate receipt must not carry the path/content — the action receipt does: {}",
        receipts[1]["params"]
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// The receipt tier records `kriya.gate.<class>.evaluated` and PROCEEDS (exit 0) — governance
/// visibility without a block, exactly what "Receipt-only" promises in the Console UI.
#[test]
fn gates_receipt_tier_signs_evaluated_and_proceeds() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, gates_policy_yaml()).unwrap();

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
        &pre_payload("Bash", r#"{"command":"git push --force origin feat/x"}"#),
    );

    assert_eq!(r.status.code(), Some(0), "receipt tier must not block: stderr={:?}", r.stderr);
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 1, "exactly the evaluated receipt (the action's own receipt lands post-hook)");
    assert_eq!(receipts[0]["action_id"], "kriya.gate.destructive-git.evaluated");
    assert_eq!(receipts[0]["success"], true);
    assert_eq!(receipts[0]["params"]["rule_id"], "git-force-push");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// The approve tier under `--approval deny` (the default, headless posture) holds the action:
/// exit 2 + the blocked attempt's action receipt + `kriya.gate.publish.held` pointing at it.
#[test]
fn gates_approve_tier_holds_npm_publish_when_no_approval_granted() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, gates_policy_yaml()).unwrap();

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
            "--approval",
            "deny",
        ],
        &pre_payload("Bash", r#"{"command":"npm publish --access public"}"#),
    );

    assert_eq!(r.status.code(), Some(2), "unapproved approve-tier gate must block: stderr={:?}", r.stderr);
    assert!(
        r.stderr.contains("publish") && r.stderr.contains("approval"),
        "stderr names the gate + the missing approval: {:?}",
        r.stderr
    );
    let receipts = read_receipts(&log);
    assert_eq!(receipts.len(), 2, "action receipt + held receipt");
    assert_eq!(receipts[0]["action_id"], "claude-code__bash");
    assert_eq!(receipts[0]["success"], false);
    assert_eq!(receipts[1]["action_id"], "kriya.gate.publish.held");
    assert_eq!(receipts[1]["params"]["corr_step"], receipts[0]["step_id"]);

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

// --- F-4 Inc 3: the payment lane — the purchase-receipt chain (kriya-console doc 31 §3.6) --------
//
// When a governed call matches a `payment`-class action gate, kriya-hook emits the
// kriya.pay.{intent,decision,outcome} chain alongside the kriya.gate.payment.* receipt. These
// end-to-end tests spawn the real binary and assert the chain the Console's `purchases.ts` folds:
// a shared `pay_id`, best-effort/honest amount, and the three decision states (executed · denied ·
// held). The `pay_id` is deterministic (session + tool + tool_input) so the post-hook outcome
// chains onto the pre-hook intent without any marker file.

/// A gates policy whose ONE payment rule fires at the requested tier for any `stripe`-server MCP
/// tool. Kept self-contained so the pay tests never perturb the shared `gates_policy_yaml()`.
fn pay_gate_policy_yaml(tier: &str) -> String {
    format!(
        "rules:\n  - {{ action: \"*\", allow: true }}\n\
         gates:\n  rules:\n    - class: payment\n      rule_id: pay-server\n      \
         tier: {tier}\n      tool_any:\n        - {{ server: \"stripe\", tool: \".*\" }}\n"
    )
}

/// The pay receipts (kriya.pay.*) from a log, in order.
fn pay_receipts(log: &PathBuf) -> Vec<serde_json::Value> {
    read_receipts(log)
        .into_iter()
        .filter(|r| r["action_id"].as_str().unwrap_or("").starts_with("kriya.pay."))
        .collect()
}

/// A denied payment never runs, so the WHOLE chain closes synchronously in the pre hook:
/// intent → decision(denied) → outcome(denied). Amount is extracted best-effort (Stripe minor
/// units), and the merchant is the processor name only — never a PAN.
#[test]
fn payment_deny_emits_the_full_intent_decision_outcome_chain() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, pay_gate_policy_yaml("deny")).unwrap();
    let args: Vec<&str> = vec![
        "--policy", policy.to_str().unwrap(),
        "--audit-log", log.to_str().unwrap(),
        "--signing-key", key.to_str().unwrap(),
    ];

    let r = run(
        &bin,
        "pre",
        &args,
        &pre_payload(
            "mcp__stripe__create_payment_intent",
            r#"{"amount":4200,"currency":"usd"}"#,
        ),
    );
    assert_eq!(r.status.code(), Some(2), "a denied payment gate blocks: stderr={:?}", r.stderr);

    let pay = pay_receipts(&log);
    assert_eq!(pay.len(), 3, "denied payment closes the full 3-link chain in pre");
    let (intent, decision, outcome) = (&pay[0], &pay[1], &pay[2]);
    assert_eq!(intent["action_id"], "kriya.pay.intent");
    assert_eq!(intent["params"]["amount_minor"], 4200);
    assert_eq!(intent["params"]["currency"], "usd");
    assert_eq!(intent["params"]["amount_known"], true);
    assert_eq!(intent["params"]["merchant"], "stripe", "processor name only — never a PAN");
    assert_eq!(intent["params"]["tool"], "mcp__stripe__create_payment_intent");
    assert_eq!(intent["success"], true, "the intent is a true record of the request");

    assert_eq!(decision["action_id"], "kriya.pay.decision");
    assert_eq!(decision["params"]["decision"], "denied");
    assert_eq!(decision["params"]["matched_rule"], "pay-server");
    assert_eq!(decision["success"], false);

    assert_eq!(outcome["action_id"], "kriya.pay.outcome");
    assert_eq!(outcome["params"]["result"], "denied");
    assert_eq!(outcome["success"], false);

    // The whole chain shares one pay_id (the Console folds by it).
    let pid = intent["params"]["pay_id"].as_str().unwrap();
    assert!(pid.starts_with("pay-"));
    assert_eq!(decision["params"]["pay_id"], pid);
    assert_eq!(outcome["params"]["pay_id"], pid);

    // Privacy: no card-shaped content ever reaches a receipt (there is none in the input, but the
    // whole chain must also never carry the raw amount as anything but the structured minor field).
    let wire = read_receipts(&log).iter().map(|r| r.to_string()).collect::<String>();
    assert!(!wire.contains("card"), "no card content in the chain: {wire}");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// A receipt-tier payment PROCEEDS: pre writes intent + decision(approved); the post hook — driven
/// by the real tool_response — closes the chain with outcome(executed) carrying the response status.
/// The post outcome chains onto the pre intent by the SAME deterministic pay_id (no marker file).
#[test]
fn payment_receipt_tier_pre_then_post_emits_executed_chain_with_status() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, pay_gate_policy_yaml("receipt")).unwrap();
    let args: Vec<&str> = vec![
        "--policy", policy.to_str().unwrap(),
        "--audit-log", log.to_str().unwrap(),
        "--signing-key", key.to_str().unwrap(),
    ];
    let tool = "mcp__stripe__create_payment_intent";
    let input = r#"{"amount":4200,"currency":"usd"}"#;

    // pre: receipt tier proceeds (exit 0), opening the chain.
    let r_pre = run(&bin, "pre", &args, &pre_payload(tool, input));
    assert_eq!(r_pre.status.code(), Some(0), "receipt tier must not block: stderr={:?}", r_pre.stderr);

    // post: the real tool_response drives the outcome (executed + status 200).
    let r_post = run(
        &bin,
        "post",
        &args,
        &post_payload_full(tool, input, r#"{"success":true,"status_code":200}"#, "s1"),
    );
    assert_eq!(r_post.status.code(), Some(0), "post is best-effort exit 0: stderr={:?}", r_post.stderr);

    let pay = pay_receipts(&log);
    assert_eq!(pay.len(), 3, "intent + decision (pre) + outcome (post)");
    let intent = pay.iter().find(|r| r["action_id"] == "kriya.pay.intent").unwrap();
    let decision = pay.iter().find(|r| r["action_id"] == "kriya.pay.decision").unwrap();
    let outcome = pay.iter().find(|r| r["action_id"] == "kriya.pay.outcome").unwrap();

    assert_eq!(decision["params"]["decision"], "approved");
    assert_eq!(decision["success"], true);
    assert_eq!(outcome["params"]["result"], "executed");
    assert_eq!(outcome["params"]["status"], "200", "best-effort status from the governed lane");
    assert_eq!(outcome["success"], true);

    // The post outcome chained onto the pre intent — the same deterministic pay_id across processes.
    let pid = intent["params"]["pay_id"].as_str().unwrap();
    assert_eq!(decision["params"]["pay_id"], pid);
    assert_eq!(outcome["params"]["pay_id"], pid, "post re-derived the identical pay_id — no marker file");

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// A held payment (approve tier, no approval granted) is the honest 2/3 shape: intent +
/// decision(held), NO outcome — it neither executed nor failed; it awaits a human. And an
/// un-parseable amount is carried as amount_known:false, never a guessed number.
#[test]
fn payment_held_is_the_two_of_three_chain_with_honest_unknown_amount() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, pay_gate_policy_yaml("approve")).unwrap();
    let args: Vec<&str> = vec![
        "--policy", policy.to_str().unwrap(),
        "--audit-log", log.to_str().unwrap(),
        "--signing-key", key.to_str().unwrap(),
        "--approval", "deny",
    ];

    // A float amount is major-unit-shaped — the hook refuses to guess-scale it ⇒ amount_known:false.
    let r = run(
        &bin,
        "pre",
        &args,
        &pre_payload("mcp__stripe__create_payment_intent", r#"{"amount":42.00}"#),
    );
    assert_eq!(r.status.code(), Some(2), "unapproved approve-tier payment holds: stderr={:?}", r.stderr);

    let pay = pay_receipts(&log);
    assert_eq!(pay.len(), 2, "held payment is intent + decision only — the honest 2/3 shape");
    let intent = &pay[0];
    let decision = &pay[1];
    assert_eq!(intent["action_id"], "kriya.pay.intent");
    assert_eq!(intent["params"]["amount_known"], false, "un-parseable amount is honest, not guessed");
    assert!(intent["params"].get("amount_minor").is_none(), "no number invented");
    assert_eq!(decision["action_id"], "kriya.pay.decision");
    assert_eq!(decision["params"]["decision"], "held");
    assert_eq!(decision["success"], false);
    assert!(
        !pay.iter().any(|r| r["action_id"] == "kriya.pay.outcome"),
        "a held payment has NO outcome receipt"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

// --- F-6 (doc 31 §9 / doc 33 §4-C): the shift-armed fail-closed clamp (gap → tier-drop) -----
// The Console arms a shift (~/.kriya/state/shift.json); while armed, a MISSED heartbeat drops
// policy to the pre-declared fail-closed tier. These prove the runtime enforcement end-to-end
// across the real process boundary: an otherwise-ALLOWED action is blocked (+ a signed
// kriya.shift.clamp receipt) when a beat is missed, and proceeds untouched when the watcher is
// alive. shift_guard.rs unit-tests the pure clamp logic; these test the wired hook.

/// Write an armed `shift.json` into `state_dir`, its window bracketing `now_ms`.
fn write_shift_armed(state_dir: &PathBuf, fail_tier: &str, now_ms: u64) {
    std::fs::create_dir_all(state_dir).unwrap();
    let body = serde_json::json!({
        "armed": true,
        "start_ms": now_ms - 3_600_000,
        "end_ms": now_ms + 3_600_000,
        "fail_tier": fail_tier,
        "cadence_ms": 60_000,
    });
    std::fs::write(state_dir.join("shift.json"), body.to_string()).unwrap();
}

#[test]
fn shift_armed_missed_heartbeat_clamps_an_allowed_action_to_deny() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, "rules:\n  - { action: \"*\", allow: true }\n").unwrap();
    let now = fresh_as_of_ms();
    write_shift_armed(&state_dir, "deny", now);
    // NO heartbeat anywhere in the audit log → a beat is missed by definition (the watcher is silent).

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

    assert_eq!(
        r.status.code(),
        Some(2),
        "armed shift + missed heartbeat must block an otherwise-allowed action (fail-closed deny): stderr={:?}",
        r.stderr
    );
    let receipts = read_receipts(&log);
    let clamp = receipts
        .iter()
        .find(|v| v["action_id"] == "kriya.shift.clamp")
        .expect("a signed kriya.shift.clamp receipt of the blocked attempt");
    assert_eq!(clamp["params"]["reason"], "missed-heartbeat");
    assert_eq!(clamp["params"]["tier"], "deny");
    assert_eq!(clamp["params"]["last_heartbeat_seen"], false);
    assert_eq!(clamp["success"], false);

    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn shift_armed_with_a_fresh_heartbeat_does_not_clamp() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, "rules:\n  - { action: \"*\", allow: true }\n").unwrap();
    let now = fresh_as_of_ms();
    write_shift_armed(&state_dir, "deny", now);
    // A FRESH heartbeat (30s ago, < 2x the 60s cadence) in the audit log → the watcher is alive.
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(
        &log,
        format!(
            r#"{{"step_id":"hb","action_id":"kriya.watch.heartbeat","params":{{}},"success":true,"ts_ms":{}}}"#,
            now - 30_000
        ),
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
        &pre_payload("Read", r#"{"file_path":"a.txt"}"#),
    );

    assert_eq!(
        r.status.code(),
        Some(0),
        "a fresh heartbeat means no missed beat → no clamp: stderr={:?}",
        r.stderr
    );
    let receipts = read_receipts(&log);
    assert!(
        receipts.iter().all(|v| v["action_id"] != "kriya.shift.clamp"),
        "no clamp receipt when the watcher is alive"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

// --- O-3 duration lane (doc 33 §5.5): the pre-marker → post-stamp round trip ---------------------

/// The allow policy every duration test uses (gates/temporal/budget out of scope here).
fn allow_all_policy() -> &'static str {
    "rules:\n  - { action: \"*\", allow: true }\n"
}

#[test]
fn o3_pre_marker_then_post_stamps_kriya_dur_ms() {
    let bin = build_binary();
    let (log, key, policy, _state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, allow_all_policy()).unwrap();
    let flags = [
        "--policy", policy.to_str().unwrap(),
        "--audit-log", log.to_str().unwrap(),
        "--signing-key", key.to_str().unwrap(),
    ];
    let tool_input = r#"{"command":"echo hi"}"#;

    // pre — allowed → writes the start marker (signs nothing itself).
    let r_pre = run(&bin, "pre", &flags, &pre_payload("Bash", tool_input));
    assert_eq!(r_pre.status.code(), Some(0), "allow must not block: {:?}", r_pre.stderr);
    assert!(read_receipts(&log).is_empty(), "pre signs nothing on a cleared allow");

    // post — SAME session/tool/input → reads+deletes the marker and stamps the duration.
    let r_post = run(
        &bin,
        "post",
        &flags,
        &post_payload_full("Bash", tool_input, r#"{"success":true}"#, "s1"),
    );
    assert_eq!(r_post.status.code(), Some(0), "post is best-effort exit 0: {:?}", r_post.stderr);

    let receipts = read_receipts(&log);
    let action = receipts
        .iter()
        .find(|r| r["action_id"] == "claude-code__bash")
        .expect("the action receipt is written");
    assert!(
        action["params"]["kriya.dur.ms"].is_u64(),
        "the action receipt carries a measured duration (u64 ms); params={}",
        action["params"]
    );
    assert_eq!(
        action["params"]["kriya.dur.basis"], "hook-pre-post",
        "the basis names how it was measured"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn o3_post_without_a_pre_marker_omits_the_duration_honest_absence() {
    let bin = build_binary();
    let (log, key, policy, _state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, allow_all_policy()).unwrap();
    let flags = [
        "--policy", policy.to_str().unwrap(),
        "--audit-log", log.to_str().unwrap(),
        "--signing-key", key.to_str().unwrap(),
    ];

    // post with no preceding pre ⇒ no marker ⇒ no duration params (never estimated/zero-filled).
    let r_post = run(
        &bin,
        "post",
        &flags,
        &post_payload_full("Bash", r#"{"command":"echo hi"}"#, r#"{"success":true}"#, "s1"),
    );
    assert_eq!(r_post.status.code(), Some(0));

    let receipts = read_receipts(&log);
    let action = receipts
        .iter()
        .find(|r| r["action_id"] == "claude-code__bash")
        .expect("the action receipt is written");
    assert!(
        action["params"].get("kriya.dur.ms").is_none(),
        "no duration without a pre marker"
    );
    assert!(action["params"].get("kriya.dur.basis").is_none());

    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}

#[test]
fn o3_duration_marker_never_blocks_when_the_state_dir_is_unwritable() {
    let bin = build_binary();
    let (log, key, policy, state_dir) = sandbox_with_state_dir();
    std::fs::write(&policy, allow_all_policy()).unwrap();
    // Make the state dir a FILE so `<state_dir>/durations` can never be created — the marker write
    // (and the sweep) must silently no-op and the gate must still proceed (the session_cond law).
    if let Some(parent) = state_dir.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&state_dir, b"i am a file, not a dir").unwrap();

    let r_pre = run(
        &bin,
        "pre",
        &[
            "--policy", policy.to_str().unwrap(),
            "--audit-log", log.to_str().unwrap(),
            "--signing-key", key.to_str().unwrap(),
        ],
        &pre_payload("Bash", r#"{"command":"echo hi"}"#),
    );
    assert_eq!(
        r_pre.status.code(),
        Some(0),
        "an unwritable marker store must never block the gate: {:?}",
        r_pre.stderr
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap().parent().unwrap());
}
