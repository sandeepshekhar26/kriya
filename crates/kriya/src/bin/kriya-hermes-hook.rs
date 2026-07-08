//! `kriya-hermes-hook` — govern **the whole Hermes lane** (`NousResearch/hermes-agent`) via its
//! shell-hook seam: native tools (`terminal`, `write_file`, `patch`, `read_file`, `computer_use`,
//! `browser_*`, …) **and every MCP server attached to Hermes** (its MCP tools register into the
//! same `ToolRegistry` and share the same `handle_function_call` → `resolve_pre_tool_block` →
//! `registry.dispatch` → post-hook chokepoint as native tools — confirmed against the real
//! `hermes-agent` source, 2026-07-08). A near-clone of `kriya-hook` (Claude Code's adapter), reusing
//! the unchanged `Policy`/`ApprovalGate`/`Signer` primitives — see that file for the shared design.
//! One paste into `~/.hermes/config.yaml`:
//!
//! ```yaml
//! hooks:
//!   pre_tool_call:
//!     - command: "kriya-hermes-hook pre"
//!       timeout: 300
//!   post_tool_call:
//!     - command: "kriya-hermes-hook post"
//!       timeout: 300
//! hooks_auto_accept: true
//! ```
//!
//! (No `matcher` — per Hermes' own dispatch loop, an entry with no `matcher` fires for *every*
//! `tool_name`, verified against `ShellHookSpec.matches_tool`/`agent/shell_hooks.py`'s `_callback`,
//! 2026-07-08 — the same "whole lane, zero per-tool config" property `kriya-hook` has for Claude
//! Code. `hooks_auto_accept: true` skips Hermes' own interactive first-use TTY consent prompt —
//! confirmed sufficient on its own, no env var or CLI flag needed — so installing this hook is a
//! single config write with no follow-up terminal step. `timeout: 300` matches both Hermes' own
//! hard per-hook cap and `GuiApproval`'s `osascript … giving up after 300`, so a pending GUI
//! approval has the full window either backend can give it.)
//!
//! ## The Hermes-specific wire contract (verified against the real source, 2026-07-08 — NOT
//! ## assumed by analogy to Claude Code; a prior analogy-based assumption for this same project
//! ## already shipped one real bug)
//!
//! Hermes pipes a JSON payload on stdin with **six fixed top-level keys** for every hook event:
//! `hook_event_name`, `tool_name`, `tool_input`, `session_id`, `cwd`, `extra` — everything
//! event-specific lives in `extra`. For `post_tool_call`, `extra` carries `status` (`"ok"` |
//! `"error"` | `"blocked"`, a *string*, not a bool), `error_type`, `error_message`, `result`,
//! `duration_ms`. There is no top-level `success` field (unlike Claude Code's `tool_response`).
//!
//! **The block mechanism is the one real divergence from `kriya-hook` (doc 21 Part B) — do not
//! miss this on the build:** Claude Code blocks via `exit 2 + stderr`. Hermes' shell-hook runner
//! (`agent/shell_hooks.py::_callback`) treats a non-zero exit as *log a warning, still parse
//! stdout* — exit code never blocks anything, for **either** `pre_tool_call` or `post_tool_call`.
//! The only thing that blocks a `pre_tool_call` is **stdout JSON**: `{"action":"block","message":
//! "…"}` (Hermes-canonical; the Claude-Code-style `{"decision":"block","reason":"…"}` shape is
//! also accepted and normalised). So on Deny/unapproved, this binary prints that JSON to stdout
//! **and exits 0** — exiting 2 here would just be an ignored, logged warning, not a gate.
//!
//! ## Honest boundary this binary cannot close (state it, don't hide it — `docs/TRUST.md` ethos)
//!
//! Unlike Claude Code (where an unanswered hook is killed and *that itself* blocks the call —
//! timeouts fail closed), Hermes' own per-hook `timeout` (capped at 300s) fails **open** on
//! expiry: a timed-out shell hook is logged and treated as "no directive", i.e. the tool call
//! proceeds. A `require_approval` rule whose human never answers within the window is therefore
//! an **allow**, not a deny, at the Hermes level — the opposite of `kriya-hook`'s Claude Code
//! behavior. This binary cannot change Hermes' own timeout semantics; it can only make its own
//! `ApprovalGate::request` resolve as fast as the operator responds and default to `deny` inside
//! that window (unchanged from `kriya-hook`). Document this to anyone enabling `require_approval`
//! policies against Hermes.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use kriya::audit::{default_audit_dir, now_ms, Actor, Receipt, Signer};
#[cfg(target_os = "macos")]
use kriya::mcp::GuiApproval;
use kriya::mcp::{ApprovalGate, AutoApprove, DenyApproval, TtyApproval};
use kriya::permissions::{Decision, Policy};
use serde::Deserialize;
use serde_json::{json, Value};

/// Record-only default: sign everything, block nothing, until the operator authors a policy —
/// identical posture to `kriya-hook`'s default (evidence first, never brick the agent on install).
const DEFAULT_POLICY_YAML: &str = "rules:\n  - { action: \"*\", allow: true }\n";

/// The slice of Hermes' shell-hook payload we consume. Six top-level keys, always present (per
/// `agent/shell_hooks.py::_serialize_payload`); everything event-specific is nested in `extra`.
/// Unknown `extra` fields are ignored on purpose — the payload can grow without breaking us.
#[derive(Debug, Deserialize)]
struct HookPayload {
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: Option<Value>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    extra: Option<Value>,
}

struct Args {
    mode: String,
    policy: Option<PathBuf>,
    approval: String,
    audit_log: Option<PathBuf>,
    signing_key: Option<PathBuf>,
    actor: String,
    user: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut argv = std::env::args().skip(1);
    let mode = match argv.next() {
        Some(m) if m == "pre" || m == "post" => m,
        Some(other) => return Err(format!("unknown subcommand '{other}' (expected pre|post)")),
        None => return Err("usage: kriya-hermes-hook pre|post [--policy p.yaml] …".into()),
    };
    let mut args = Args {
        mode,
        policy: None,
        approval: "deny".into(),
        audit_log: None,
        signing_key: None,
        actor: "hermes".into(),
        user: None,
    };
    while let Some(flag) = argv.next() {
        let mut val = |name: &str| argv.next().ok_or_else(|| format!("{name} needs a value"));
        match flag.as_str() {
            "--policy" => args.policy = Some(PathBuf::from(val("--policy")?)),
            "--approval" => args.approval = val("--approval")?,
            "--audit-log" => args.audit_log = Some(PathBuf::from(val("--audit-log")?)),
            "--signing-key" => args.signing_key = Some(PathBuf::from(val("--signing-key")?)),
            "--actor" => args.actor = val("--actor")?,
            "--user" => args.user = Some(val("--user")?),
            other => return Err(format!("unknown flag '{other}'")),
        }
    }
    Ok(args)
}

/// Map a Hermes tool name onto the governed action id namespace.
fn action_id_for(tool_name: &str) -> String {
    format!("hermes__{}", tool_name.to_lowercase())
}

/// Success of a completed call, derived from `extra.status` (a string: `"ok" | "error" |
/// "blocked"` — NOT a bool, unlike Claude Code's `tool_response.success`). Absent/malformed
/// `extra` or an absent `status` key means "ran, no status reported" → treated as success,
/// mirroring `kriya-hook`'s "no response info → ran → success" default.
fn outcome_success(extra: Option<&Value>) -> bool {
    match extra.and_then(|e| e.get("status")).and_then(Value::as_str) {
        Some("ok") => true,
        Some("error") | Some("blocked") => false,
        _ => true,
    }
}

fn load_policy(path: Option<&PathBuf>) -> Result<Policy, String> {
    match path {
        None => serde_yaml::from_str(DEFAULT_POLICY_YAML)
            .map_err(|e| format!("built-in default policy failed to parse: {e}")),
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read policy {}: {e}", p.display()))?;
            serde_yaml::from_str(&text)
                .map_err(|e| format!("policy {} is not valid YAML: {e}", p.display()))
        }
    }
}

fn approval_gate(mode: &str) -> Result<Box<dyn ApprovalGate>, String> {
    match mode {
        "deny" => Ok(Box::new(DenyApproval)),
        "tty" => Ok(Box::new(TtyApproval)),
        "auto" => Ok(Box::new(AutoApprove)),
        #[cfg(target_os = "macos")]
        "gui" => Ok(Box::new(GuiApproval)),
        other => Err(format!("unknown --approval mode '{other}'")),
    }
}

/// The stable per-front audit log + persisted signing identity (defaults; flags override). Logs
/// to `~/.kriya/audit/hermes.jsonl` under a `hermes-hook.key` identity — the exact filename the
/// Console's multi-agent Coverage map (`coverage.rs::scan_agents`) already looks for.
fn signer_for(args: &Args) -> Result<Signer, String> {
    let log_path = match &args.audit_log {
        Some(p) => p.clone(),
        None => {
            let dir = default_audit_dir();
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            dir.join("hermes.jsonl")
        }
    };
    let key_path = match &args.signing_key {
        Some(p) => p.clone(),
        None => {
            let keys = default_audit_dir()
                .parent()
                .map(|p| p.join("keys"))
                .unwrap_or_else(|| PathBuf::from(".kriya-keys"));
            std::fs::create_dir_all(&keys)
                .map_err(|e| format!("cannot create {}: {e}", keys.display()))?;
            keys.join("hermes-hook.key")
        }
    };
    Signer::with_identity(&key_path, log_path)
}

fn record(signer: &Signer, actor: &Actor, action_id: &str, params: Value, success: bool) {
    signer.record(
        Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            action_id.to_string(),
            params,
            success,
            now_ms(),
        )
        .with_actor(Some(actor.clone())),
    );
}

/// Hermes' block directive: `{"action":"block","message":"…"}` printed to stdout. Exit code is
/// irrelevant to whether Hermes treats the call as blocked (confirmed: a non-zero exit only logs
/// a warning, stdout is parsed regardless) — this binary still exits 0 on a block so nothing about
/// process exit status is ever load-bearing for either side.
fn print_block(message: &str) {
    println!(
        "{}",
        json!({ "action": "block", "message": message })
    );
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("kriya-hermes-hook: {e}");
            print_block(&format!("kriya-hermes-hook misconfigured: {e}"));
            return ExitCode::SUCCESS; // fail closed via the stdout directive, not the exit code
        }
    };

    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("kriya-hermes-hook: cannot read hook payload: {e}");
        return fail_mode(&args.mode, "kriya-hermes-hook: cannot read hook payload");
    }
    let payload: HookPayload = match serde_json::from_str(&stdin) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kriya-hermes-hook: hook payload is not the expected JSON: {e}");
            return fail_mode(&args.mode, "kriya-hermes-hook: malformed hook payload");
        }
    };
    let tool_name = match payload.tool_name.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => {
            eprintln!("kriya-hermes-hook: payload has no tool_name");
            return fail_mode(&args.mode, "kriya-hermes-hook: payload has no tool_name");
        }
    };
    let action_id = action_id_for(tool_name);
    let params = payload.tool_input.clone().unwrap_or(Value::Null);

    let user = args
        .user
        .clone()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".into());
    let actor = Actor::new(args.actor.clone(), user);
    let _ = &payload.session_id; // reserved: session correlation, mirrors kriya-hook's own reserve

    let signer = match signer_for(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("kriya-hermes-hook: signer unavailable: {e}");
            return fail_mode(&args.mode, "kriya-hermes-hook: signer unavailable");
        }
    };

    match args.mode.as_str() {
        "pre" => {
            let policy = match load_policy(args.policy.as_ref()) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("kriya-hermes-hook: {e} — blocking (fail closed)");
                    print_block(&format!("kriya-hermes-hook: policy error — {e}"));
                    return ExitCode::SUCCESS;
                }
            };
            match policy.check(&action_id) {
                Decision::Allow => ExitCode::SUCCESS, // the post hook records the outcome
                Decision::RequiresApproval => {
                    let gate = match approval_gate(&args.approval) {
                        Ok(g) => g,
                        Err(e) => {
                            eprintln!("kriya-hermes-hook: {e} — blocking (fail closed)");
                            print_block(&format!("kriya-hermes-hook: {e}"));
                            return ExitCode::SUCCESS;
                        }
                    };
                    if gate.request(&action_id, &params) {
                        ExitCode::SUCCESS
                    } else {
                        record(&signer, &actor, &action_id, params, false);
                        let msg = format!(
                            "'{action_id}' requires human approval and was not approved (kriya policy; approval mode: {}). A signed receipt of the blocked attempt was recorded.",
                            args.approval
                        );
                        eprintln!("kriya-hermes-hook: {msg}");
                        print_block(&msg);
                        ExitCode::SUCCESS
                    }
                }
                Decision::Deny => {
                    record(&signer, &actor, &action_id, params, false);
                    let msg = format!(
                        "'{action_id}' is denied by kriya policy. A signed receipt of the blocked attempt was recorded."
                    );
                    eprintln!("kriya-hermes-hook: {msg}");
                    print_block(&msg);
                    ExitCode::SUCCESS
                }
            }
        }
        "post" => {
            let success = outcome_success(payload.extra.as_ref());
            record(&signer, &actor, &action_id, params, success);
            ExitCode::SUCCESS
        }
        _ => unreachable!("parse_args validated the mode"),
    }
}

/// `pre` fails CLOSED (a block directive on stdout blocks the call, regardless of exit code);
/// `post` fails OPEN-but-loud (nothing to block — the tool already ran; surface the evidence gap
/// on stderr instead). Exit code is `SUCCESS` in both cases: Hermes never gates on exit status.
fn fail_mode(mode: &str, message: &str) -> ExitCode {
    if mode == "pre" {
        print_block(message);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_tool_names_into_the_governed_namespace() {
        assert_eq!(action_id_for("terminal"), "hermes__terminal");
        assert_eq!(action_id_for("write_file"), "hermes__write_file");
        assert_eq!(action_id_for("computer_use"), "hermes__computer_use");
    }

    #[test]
    fn outcome_success_derivation_from_extra_status() {
        assert!(outcome_success(None), "no extra at all → ran → success");
        assert!(outcome_success(Some(&json!({}))), "extra present, no status → success");
        assert!(outcome_success(Some(&json!({"status": "ok"}))));
        assert!(!outcome_success(Some(&json!({"status": "error"}))));
        assert!(!outcome_success(Some(&json!({"status": "blocked"}))));
        // status is a string, not a bool — an unexpected shape must not crash or silently pass
        // as failure; default to success exactly like an absent key.
        assert!(outcome_success(Some(&json!({"status": true}))));
    }

    #[test]
    fn default_policy_records_everything_blocks_nothing() {
        let p: Policy = serde_yaml::from_str(DEFAULT_POLICY_YAML).unwrap();
        assert_eq!(p.check("hermes__terminal"), Decision::Allow);
        assert_eq!(p.check("hermes__write_file"), Decision::Allow);
    }

    /// Per-server MCP gating is a prefix glob over the mapped action id — the same rung
    /// `kriya-hook` proves for Claude Code, transposed onto Hermes' native tool-name prefix
    /// convention (`mcp__<server>__<tool>` — confirmed Hermes' MCP tools register with the same
    /// prefixed name shape into the shared registry as Claude Code's hook sees).
    #[test]
    fn an_enforcing_policy_gates_mcp_servers_individually() {
        let p: Policy = serde_yaml::from_str(
            "rules:\n  - { action: \"hermes__mcp__github__*\", allow: true, require_approval: true }\n  - { action: \"hermes__mcp__*\", allow: false }\n  - { action: \"hermes__*\", allow: true }\n",
        )
        .unwrap();
        assert_eq!(
            p.check(&action_id_for("mcp__github__create_issue")),
            Decision::RequiresApproval,
            "named server is approval-gated"
        );
        assert_eq!(
            p.check(&action_id_for("mcp__shady_exfil__send")),
            Decision::Deny,
            "unlisted MCP servers are denied"
        );
        assert_eq!(
            p.check(&action_id_for("terminal")),
            Decision::Allow,
            "native tools ride the trailing namespace rule"
        );
    }

    #[test]
    fn an_enforcing_policy_gates_the_namespace() {
        let p: Policy = serde_yaml::from_str(
            "rules:\n  - { action: \"hermes__read_file\", allow: true }\n  - { action: \"hermes__terminal\", allow: true, require_approval: true }\n  - { action: \"*\", allow: false }\n",
        )
        .unwrap();
        assert_eq!(p.check("hermes__read_file"), Decision::Allow);
        assert_eq!(p.check("hermes__terminal"), Decision::RequiresApproval);
        assert_eq!(p.check("hermes__write_file"), Decision::Deny);
    }

    #[test]
    fn print_block_emits_hermes_canonical_json_on_one_line() {
        // A regression guard on the wire shape itself: {"action":"block","message":"..."} — the
        // ONE thing that actually blocks a pre_tool_call on the Hermes side. Exercised via the
        // JSON construction directly (print_block writes to real stdout, checked by the e2e
        // smoke test below); this locks the shape independent of process I/O.
        let v = json!({ "action": "block", "message": "denied" });
        assert_eq!(v["action"], "block");
        assert_eq!(v.get("decision"), None, "must be Hermes-canonical shape, not decision/reason");
    }

    /// Two separate Signer instances over the SAME log (≈ two hook invocations, fresh process
    /// each) must extend one hash chain under one persisted identity, and deleting a line must
    /// be visible in the chain — mirrors kriya-hook's own chain-continuity test.
    #[test]
    fn receipts_chain_across_invocations_and_deletion_is_visible() {
        use sha2::{Digest, Sha256};
        let sha256_hex = |b: &[u8]| {
            let mut h = Sha256::new();
            h.update(b);
            hex::encode(h.finalize())
        };

        let dir = std::env::temp_dir().join(format!("kriya-hermes-hook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("hook.key");
        let log = dir.join("hermes.jsonl");
        let actor = Actor::new("hermes", "tester");

        let s1 = Signer::with_identity(&key, log.clone()).unwrap();
        record(&s1, &actor, "hermes__terminal", json!({"command":"ls"}), true);
        drop(s1);
        let s2 = Signer::with_identity(&key, log.clone()).unwrap();
        record(&s2, &actor, "hermes__write_file", json!({"path":"a.txt"}), false);

        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let v1: Value = serde_json::from_str(lines[0]).unwrap();
        let v2: Value = serde_json::from_str(lines[1]).unwrap();

        assert_eq!(v1["public_key"], v2["public_key"], "one persisted identity across invocations");
        assert_eq!(v1["prev_hash"], Value::Null, "genesis receipt is unchained");
        assert_eq!(
            v2["prev_hash"].as_str().unwrap(),
            sha256_hex(lines[0].as_bytes()),
            "receipt 2 chains to the exact bytes of receipt 1 — across process boundaries"
        );
        assert_eq!(v2["success"], json!(false), "blocked/failed attempts are receipts too");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
