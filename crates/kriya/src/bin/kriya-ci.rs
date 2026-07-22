//! `kriya-ci` — **the governed CI lane** (S4). A thin wrapper that runs a governed agent step in
//! CI under a **repo-committed policy file**, then **fails the job when that policy blocks a governed
//! action** and **fails closed on kriya's own errors** (missing/invalid policy, unreadable or
//! chain-broken receipt log). The signed receipts it leaves behind are the build's evidence artifact,
//! re-verifiable **offline** by `verify-receipts` (the audit CLI) — the independent second check.
//!
//! ## Not "Policy CI"
//! This is distinct from **I3 "Policy CI — test before apply"** (counterfactual replay of a
//! *candidate* policy to preview its blast radius). `kriya-ci` **enforces the run's OWN policy over
//! the run's OWN receipts** as a gate on a live CI step — no candidate, no counterfactual. Call it
//! "the governed CI lane". (They compose: run I3 in a PR to preview an edit, then the governed CI
//! lane enforces it.)
//!
//! ## Honest ceiling (stated, never hidden)
//! The CI runner is a **cooperative** lane — whoever controls the workflow YAML can edit the step
//! out, and a hostile step could avoid routing its calls through kriya at all. So `kriya-ci` governs
//! and evidences a *cooperative* agent step; it does **not** contain it. Containment is a different
//! product (`kriya-gateway run --`, B14). The tamper-**evidence** comes from the offline
//! `verify-receipts` re-prove, not from trusting the CI runner.
//!
//! ## Semantics (decided, tested — not improvised)
//! - **Headless approvals deny.** A `require_approval` action has no human in CI, so the gate treats
//!   it as **blocked** (honoring the runtime's fail-closed approval posture — a `require_approval`
//!   action reaching a receipt in CI means it would have paused for a human who is not there).
//! - **Fail-closed on kriya errors.** Missing/invalid policy, or an unreadable / hash-chain-broken
//!   receipt log, exits `4` — a governance step that can't prove its own inputs must not report
//!   "clean". Every failure path is tested.
//! - **Two deny signals.** (a) A receipt exists for an action the policy blocks — caught by any lane
//!   that records blocked attempts (the hook does). (b) The governed step itself exited non-zero —
//!   caught on any lane (the agent surfaced the deny as an error). Either fails the job.
//!
//! ## Usage
//! ```text
//! kriya-ci run --policy <policy.yaml> --audit-log <log.jsonl> -- <agent-command...>
//! ```
//! `run` clears `--audit-log`, exports `KRIYA_POLICY` + `KRIYA_AUDIT_LOG` to the child (so the
//! agent's governance lane writes there under the same policy), runs `<agent-command>`, then gates.
//!
//! ## Exit codes (stable — a workflow can branch on them)
//! - `0`  CLEAN — the step ran and no governed action was blocked; receipts written.
//! - `3`  POLICY_DENIED — the policy blocked ≥1 governed action (named in the message).
//! - `4`  KRIYA_ERROR — kriya's own error (policy / log / chain); fail-closed.
//! - `5`  STEP_FAILED — the governed step exited non-zero for a reason that is NOT a recorded policy
//!   denial (a genuine build/agent failure), surfaced distinctly so it isn't misread as a deny.
//! - `2`  USAGE — bad arguments.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use kriya::permissions::{Decision, Policy};
use serde_json::Value;

const EXIT_CLEAN: u8 = 0;
const EXIT_USAGE: u8 = 2;
const EXIT_POLICY_DENIED: u8 = 3;
const EXIT_KRIYA_ERROR: u8 = 4;
const EXIT_STEP_FAILED: u8 = 5;

struct Args {
    policy: PathBuf,
    audit_log: PathBuf,
    command: Vec<String>,
}

fn usage() -> String {
    "usage: kriya-ci run --policy <policy.yaml> --audit-log <log.jsonl> -- <agent-command...>"
        .into()
}

fn parse_args() -> Result<Args, String> {
    let mut argv = std::env::args().skip(1);
    match argv.next().as_deref() {
        Some("run") => {}
        Some(other) => return Err(format!("unknown subcommand '{other}' (expected 'run')")),
        None => return Err(usage()),
    }
    let mut policy: Option<PathBuf> = None;
    let mut audit_log: Option<PathBuf> = None;
    let mut command: Vec<String> = Vec::new();
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--policy" => {
                policy = Some(PathBuf::from(argv.next().ok_or("--policy needs a value")?))
            }
            "--audit-log" => {
                audit_log = Some(PathBuf::from(
                    argv.next().ok_or("--audit-log needs a value")?,
                ))
            }
            "--" => {
                command = argv.by_ref().collect();
                break;
            }
            other => return Err(format!("unknown flag '{other}'")),
        }
    }
    let policy =
        policy.ok_or("--policy is required (the governed CI lane needs a committed policy)")?;
    let audit_log = audit_log.ok_or("--audit-log is required")?;
    if command.is_empty() {
        return Err("no agent command given after `--`".into());
    }
    Ok(Args {
        policy,
        audit_log,
        command,
    })
}

/// Load the policy **fallibly** (unlike `Policy::load_or_default`, which silently falls back to a
/// permissive default — the exact B0-class bug the governed CI lane must never repeat).
fn load_policy_strict(path: &Path) -> Result<Policy, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read policy {}: {e}", path.display()))?;
    serde_yaml::from_str::<Policy>(&text)
        .map_err(|e| format!("policy {} is not valid YAML: {e}", path.display()))
}

/// Lowercase-hex SHA-256 — matches `kriya::audit`'s chain hash (over the exact raw line).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// One receipt as the gate reads it (only what the gate needs).
struct CiReceipt {
    action_id: String,
}

/// Parse + chain-check a receipt log. Returns the receipts on success, or a fail-closed reason
/// (missing/torn line, or a hash-chain break — a deleted/reordered/truncated log). An empty/absent
/// log is a clean empty run (the step governed nothing), not an error.
fn read_and_chain_check(log: &Path) -> Result<Vec<CiReceipt>, String> {
    let content = match std::fs::read_to_string(log) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read receipt log {}: {e}", log.display())),
    };
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut receipts = Vec::with_capacity(lines.len());
    let mut prev: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("receipt log line {} is not valid JSON: {e}", i + 1))?;
        // Chain: each line after the genesis must carry prev_hash = sha256 of the previous line.
        let declared = v
            .get("prev_hash")
            .and_then(Value::as_str)
            .map(str::to_string);
        if declared != prev {
            return Err(format!(
                "hash-chain break at line {} — the receipt log was deleted, reordered, or truncated",
                i + 1
            ));
        }
        prev = Some(sha256_hex(line.as_bytes()));
        let action_id = v
            .get("action_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("receipt log line {} has no action_id", i + 1))?
            .to_string();
        receipts.push(CiReceipt { action_id });
    }
    Ok(receipts)
}

/// The CI verdict — the pure core, unit-tested without spawning a process.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// N governed actions, none blocked.
    Clean(usize),
    /// The policy blocks these governed actions (action_id, why).
    PolicyDenied(Vec<(String, &'static str)>),
    /// The step exited non-zero for a non-policy reason.
    StepFailed(i32),
}

/// Governance metadata the gate must NOT policy-check as an agent action (the `kriya.*` reserved
/// namespaces: attestation, io ledger, coverage, policy-lifecycle). Only real agent actions gate.
fn is_governance_metadata(action_id: &str) -> bool {
    action_id.starts_with("kriya.")
}

/// The gate: enforce the run's OWN policy over the run's OWN receipts, plus the step's exit code.
/// A receipt for an action the policy `Deny`s — or `RequiresApproval`s, which is a block in a
/// headless run with no human — fails the job. Otherwise a non-zero step exit fails it distinctly.
fn evaluate(policy: &Policy, receipts: &[CiReceipt], step_exit: i32) -> Verdict {
    let mut blocked: Vec<(String, &'static str)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in receipts {
        if is_governance_metadata(&r.action_id) {
            continue;
        }
        let why = match policy.check(&r.action_id) {
            Decision::Deny => "denied by policy",
            Decision::RequiresApproval => "requires human approval — none attached in CI",
            Decision::Allow => continue,
        };
        if seen.insert(r.action_id.clone()) {
            blocked.push((r.action_id.clone(), why));
        }
    }
    if !blocked.is_empty() {
        return Verdict::PolicyDenied(blocked);
    }
    if step_exit != 0 {
        return Verdict::StepFailed(step_exit);
    }
    Verdict::Clean(
        receipts
            .iter()
            .filter(|r| !is_governance_metadata(&r.action_id))
            .count(),
    )
}

fn run(args: Args) -> ExitCode {
    // 1. Policy — fail closed if missing/invalid (never silently permissive).
    let policy = match load_policy_strict(&args.policy) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kriya-ci: {e}");
            eprintln!(
                "kriya-ci: refusing to run a governed step without a valid policy (fail-closed)."
            );
            return ExitCode::from(EXIT_KRIYA_ERROR);
        }
    };

    // 2. Fresh receipt log for THIS run (so the gate judges only this step's actions).
    if let Some(parent) = args.audit_log.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Err(e) = std::fs::write(&args.audit_log, b"") {
        eprintln!(
            "kriya-ci: cannot prepare receipt log {}: {e} (fail-closed)",
            args.audit_log.display()
        );
        return ExitCode::from(EXIT_KRIYA_ERROR);
    }

    // 3. Run the governed agent step. It inherits the policy + log via env so its governance lane
    //    (hook / gateway / kriya-govern) writes signed receipts to the same log under the same policy.
    let (program, rest) = args.command.split_first().expect("validated non-empty");
    let status = Command::new(program)
        .args(rest)
        .env("KRIYA_POLICY", &args.policy)
        .env("KRIYA_AUDIT_LOG", &args.audit_log)
        .status();
    let step_exit = match status {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            eprintln!("kriya-ci: failed to launch the agent step '{program}': {e}");
            return ExitCode::from(EXIT_STEP_FAILED);
        }
    };

    // 4. Read + chain-check the receipts — fail closed on kriya's own error.
    let receipts = match read_and_chain_check(&args.audit_log) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("kriya-ci: {e} (fail-closed)");
            return ExitCode::from(EXIT_KRIYA_ERROR);
        }
    };

    // 5. Gate.
    match evaluate(&policy, &receipts, step_exit) {
        Verdict::Clean(n) => {
            eprintln!(
                "kriya-ci: ✓ governed CI lane clean — {n} governed action(s), none blocked. \
                 Receipts at {} (re-verify offline with `verify-receipts`).",
                args.audit_log.display()
            );
            ExitCode::from(EXIT_CLEAN)
        }
        Verdict::PolicyDenied(blocked) => {
            eprintln!(
                "kriya-ci: ✗ POLICY DENIED — the committed policy blocks {} governed action(s):",
                blocked.len()
            );
            for (action, why) in &blocked {
                eprintln!("  - {action}: {why}");
            }
            eprintln!("kriya-ci: failing the job. Adjust the agent step or the policy; the signed receipts at {} are the evidence.", args.audit_log.display());
            ExitCode::from(EXIT_POLICY_DENIED)
        }
        Verdict::StepFailed(code) => {
            eprintln!(
                "kriya-ci: the governed step exited {code} with no recorded policy denial — \
                 a non-governance failure. Failing the job (exit {EXIT_STEP_FAILED})."
            );
            ExitCode::from(EXIT_STEP_FAILED)
        }
    }
}

fn main() -> ExitCode {
    match parse_args() {
        Ok(args) => run(args),
        Err(e) => {
            eprintln!("kriya-ci: {e}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy(yaml: &str) -> Policy {
        serde_yaml::from_str(yaml).expect("test policy parses")
    }

    fn receipts(ids: &[&str]) -> Vec<CiReceipt> {
        ids.iter()
            .map(|id| CiReceipt {
                action_id: (*id).to_string(),
            })
            .collect()
    }

    // ── the gate (pure core) ────────────────────────────────────────────────────────────────────

    #[test]
    fn allow_only_run_is_clean_exit_zero() {
        let p = policy("rules:\n  - { action: \"read_file\", allow: true }\n  - { action: \"list_dir\", allow: true }\n");
        assert_eq!(
            evaluate(&p, &receipts(&["read_file", "list_dir", "read_file"]), 0),
            Verdict::Clean(3)
        );
    }

    #[test]
    fn a_denied_action_fails_the_job_with_a_named_message() {
        // deny-by-default: `wire_funds` matches no allow rule.
        let p = policy("rules:\n  - { action: \"read_file\", allow: true }\n");
        match evaluate(&p, &receipts(&["read_file", "wire_funds"]), 0) {
            Verdict::PolicyDenied(blocked) => {
                assert_eq!(blocked.len(), 1);
                assert_eq!(blocked[0].0, "wire_funds");
                assert!(blocked[0].1.contains("denied"));
            }
            other => panic!("expected PolicyDenied, got {other:?}"),
        }
    }

    #[test]
    fn require_approval_is_a_block_in_headless_ci() {
        let p = policy("rules:\n  - { action: \"deploy\", allow: true, require_approval: true }\n");
        match evaluate(&p, &receipts(&["deploy"]), 0) {
            Verdict::PolicyDenied(blocked) => {
                assert_eq!(blocked[0].0, "deploy");
                assert!(blocked[0].1.contains("human approval"));
            }
            other => panic!("expected PolicyDenied for a require_approval action, got {other:?}"),
        }
    }

    #[test]
    fn governance_metadata_receipts_are_never_gated_as_agent_actions() {
        // A deny-by-default policy must NOT flag kriya.* metadata (io ledger / attestation) as denied.
        let p = policy("rules:\n  - { action: \"read_file\", allow: true }\n");
        let recs = receipts(&[
            "read_file",
            "kriya.io.egress.http.allow",
            "kriya.attestation.on_device",
        ]);
        assert_eq!(evaluate(&p, &recs, 0), Verdict::Clean(1)); // only read_file counts
    }

    #[test]
    fn a_nonzero_step_with_no_denial_is_step_failed_not_policy_denied() {
        let p = policy("rules:\n  - { action: \"read_file\", allow: true }\n");
        assert_eq!(
            evaluate(&p, &receipts(&["read_file"]), 42),
            Verdict::StepFailed(42)
        );
    }

    #[test]
    fn a_denial_outranks_a_nonzero_step_exit() {
        // Both a policy block AND a nonzero step: report the policy denial (the actionable cause).
        let p = policy("rules:\n  - { action: \"read_file\", allow: true }\n");
        assert!(matches!(
            evaluate(&p, &receipts(&["read_file", "rm_rf"]), 1),
            Verdict::PolicyDenied(_)
        ));
    }

    // ── policy loading (fail-closed) ────────────────────────────────────────────────────────────

    #[test]
    fn load_policy_strict_errors_on_missing_and_invalid_never_falls_back() {
        let dir = std::env::temp_dir().join(format!("kriya-ci-pol-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            load_policy_strict(&dir.join("nope.yaml")).is_err(),
            "a missing policy must be an error (fail-closed), not a permissive default"
        );
        let bad = dir.join("bad.yaml");
        std::fs::write(&bad, "rules: [ this is : not : valid").unwrap();
        assert!(load_policy_strict(&bad).is_err(), "invalid YAML must error");
        let good = dir.join("good.yaml");
        std::fs::write(&good, "rules:\n  - { action: \"x\", allow: true }\n").unwrap();
        assert!(load_policy_strict(&good).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── chain check (fail-closed) ───────────────────────────────────────────────────────────────

    fn write_log(dir: &Path, lines: &[Value]) -> PathBuf {
        // Build a genuine hash chain: each line's prev_hash = sha256 of the previous raw line.
        let log = dir.join("audit.jsonl");
        let mut out = String::new();
        let mut prev: Option<String> = None;
        for l in lines {
            let mut obj = l.clone();
            if let Some(p) = &prev {
                obj["prev_hash"] = json!(p);
            }
            let line = serde_json::to_string(&obj).unwrap();
            prev = Some(sha256_hex(line.as_bytes()));
            out.push_str(&line);
            out.push('\n');
        }
        std::fs::write(&log, out).unwrap();
        log
    }

    #[test]
    fn absent_log_is_a_clean_empty_run_not_an_error() {
        let missing =
            std::env::temp_dir().join(format!("kriya-ci-absent-{}.jsonl", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_file(&missing);
        assert_eq!(read_and_chain_check(&missing).unwrap().len(), 0);
    }

    #[test]
    fn intact_chain_parses_and_broken_chain_fails_closed() {
        let dir = std::env::temp_dir().join(format!("kriya-ci-chain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = write_log(
            &dir,
            &[
                json!({ "step_id": "s1", "action_id": "read_file", "params": {}, "success": true, "ts_ms": 1 }),
                json!({ "step_id": "s2", "action_id": "list_dir", "params": {}, "success": true, "ts_ms": 2 }),
            ],
        );
        let recs = read_and_chain_check(&log).expect("intact chain parses");
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].action_id, "read_file");

        // Delete the first line → the survivor's prev_hash now dangles → chain break, fail-closed.
        let content = std::fs::read_to_string(&log).unwrap();
        let second_line = content.lines().nth(1).unwrap();
        std::fs::write(&log, format!("{second_line}\n")).unwrap();
        assert!(
            read_and_chain_check(&log).is_err(),
            "a truncated/deleted log must fail closed, not read as clean"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
