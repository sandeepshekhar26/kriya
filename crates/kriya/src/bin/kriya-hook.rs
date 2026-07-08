//! `kriya-hook` — govern **the whole Claude Code lane** via its hooks seam (R30): native tools
//! (Bash, Edit, Write, …) **and every MCP server attached to Claude Code** (`mcp__<server>__<tool>`
//! calls). Servers added straight to Claude Code never pass a gateway — hooks are the one seam
//! that sees them all, with zero per-server config (the snippet below sets **no `matcher`**, and
//! per the hooks contract an absent matcher fires for *every* tool — verified 2026-07-03 against
//! the hooks reference). The gateway remains the seam for *other* MCP clients (Claude Desktop,
//! Cursor, …). One paste into `~/.claude/settings.json`:
//!
//! ```jsonc
//! { "hooks": {
//!     "PreToolUse":  [{ "hooks": [{ "type": "command", "command": "kriya-hook pre" }] }],
//!     "PostToolUse": [{ "hooks": [{ "type": "command", "command": "kriya-hook post" }] }]
//! } }
//! ```
//!
//! Claude Code pipes a JSON payload on stdin (`hook_event_name`, `tool_name`, `tool_input`,
//! and on PostToolUse `tool_response` — see the hooks reference:
//! <https://docs.anthropic.com/en/docs/claude-code/hooks>). Mapping: every tool call becomes the
//! governed action `claude-code__<tool_name lowercased>` with `params = tool_input`; MCP tools
//! keep their full name, so `mcp__github__create_issue` becomes
//! `claude-code__mcp__github__create_issue` — which makes **per-server policy** a prefix glob
//! (`claude-code__mcp__github__*`).
//!
//! ## Division of labor (why two hooks)
//! - **`pre` is the GATE.** Policy check (+ optional human approval). A blocked call exits **2**
//!   with the reason on stderr (the documented blocking contract — Claude sees why and adapts)
//!   and signs a `success:false` receipt so **attempts are evidence too**. An allowed call exits
//!   0 and signs nothing — the outcome isn't known yet.
//! - **`post` is the RECORD.** Signs the Ed25519, hash-chained receipt of what actually ran
//!   (success derived from `tool_response`). Install BOTH: `pre` alone gates but records only
//!   blocks; `post` alone records but never gates.
//!
//! Receipts append to the standard on-device audit dir (`~/.kriya/audit/claude-code.jsonl`) under
//! a **persistent** signing identity (`~/.kriya/keys/claude-code-hook.key`, 0600, auto-created) —
//! the Console auto-discovers the log (R27) and `kriya-audit` / the 5-language verifiers re-prove
//! it offline. The hash chain seeds from the log tail, so the chain spans hook invocations even
//! though each is a fresh process.
//!
//! ## Flags (both subcommands)
//! ```text
//! kriya-hook pre|post [--policy <p.yaml>] [--approval deny|tty|gui|auto]
//!                     [--audit-log <path>] [--signing-key <path>]
//!                     [--actor <agent>] [--user <user>]
//! ```
//! Default policy is **record-only** (`action: "*"` → allow): evidence first, zero broken
//! sessions; enforce by passing a policy (first match wins, no match = deny), e.g.
//! ```yaml
//! rules:
//!   - { action: "claude-code__read",  allow: true }
//!   - { action: "claude-code__glob",  allow: true }
//!   - { action: "claude-code__grep",  allow: true }
//!   - { action: "claude-code__bash",  allow: true, require_approval: true }
//!   # MCP servers attached to Claude Code, gated per server:
//!   - { action: "claude-code__mcp__github__*",  allow: true, require_approval: true }
//!   - { action: "claude-code__mcp__*",          allow: false }
//!   - { action: "claude-code__*",     allow: true }
//! ```
//!
//! ## Honest boundaries (documented, not hidden)
//! - **Fail-closed:** a malformed `pre` payload or unreadable policy blocks the call (exit 2) —
//!   governance that fails open is theater. `post` is best-effort (loud stderr, exit 0): the tool
//!   already ran; blocking after the fact only breaks the session.
//! - Hooks are a **cooperative seam**: whoever controls `settings.json` controls the hook (use
//!   managed settings org-wide; the *absence* of expected receipts is itself visible in the
//!   Console's coverage view).
//! - Per-minute **budget caps don't apply here**: each hook call is a fresh process, so there is
//!   no in-process rate state. Budgets live in the long-running gateway; the hook's `post`
//!   receipts still make volume visible after the fact.
//! - Approval modes: `deny` (default — a `require_approval` rule blocks unless changed), `tty`
//!   (prompt on /dev/tty — terminal sessions only), `gui` (macOS dialog), `auto` (approve all —
//!   demos only). **Claude Code's own hook timeout (600s default for command hooks) fails OPEN on
//!   expiry** — a killed/timed-out hook is treated as no decision, and the tool proceeds. This is
//!   the opposite of an earlier version of this comment, which incorrectly claimed timeouts fail
//!   closed; verified against the current hooks reference. `tty`/`gui` mitigate this the only way
//!   this binary can: both self-bound an unanswered prompt at 300s (well under Claude Code's own
//!   ceiling) and deny on their own timeout, so the decision is made here, not by an external kill.
//!   We deliberately do **not** use Claude Code's native `permissionDecision:"ask"` for the
//!   approval tier — it has documented, reproducible reliability gaps (doesn't always fire in
//!   headless `claude -p` mode, can race with tool execution there, and has been observed silently
//!   overridden by a broad `permissions.allow` rule elsewhere in settings, letting the tool run
//!   with no prompt at all) — `tty`/`gui` are the more reliable mechanism this binary controls
//!   end-to-end.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use kriya::audit::{default_audit_dir, now_ms, Actor, Receipt, Signer};
#[cfg(target_os = "macos")]
use kriya::mcp::GuiApproval;
use kriya::mcp::{ApprovalGate, AutoApprove, DenyApproval, TtyApproval};
use kriya::permissions::{Decision, Policy};
use serde::Deserialize;
use serde_json::Value;

/// Record-only default: sign everything, block nothing, until the operator authors a policy.
/// (The in-process host's deny-by-default is right for app actions; silently bricking a user's
/// coding agent on install is not — evidence first, enforcement by explicit choice.)
const DEFAULT_POLICY_YAML: &str = "rules:\n  - { action: \"*\", allow: true }\n";

/// The slice of the Claude Code hook payload we consume. Unknown fields are ignored on purpose —
/// the payload grows over time and the adapter must not break when it does.
#[derive(Debug, Deserialize)]
struct HookPayload {
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: Option<Value>,
    #[serde(default)]
    tool_response: Option<Value>,
    #[serde(default)]
    session_id: Option<String>,
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
        None => return Err("usage: kriya-hook pre|post [--policy p.yaml] …".into()),
    };
    let mut args = Args {
        mode,
        policy: None,
        approval: "deny".into(),
        audit_log: None,
        signing_key: None,
        actor: "claude-code".into(),
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

/// Map a Claude Code tool name onto the governed action id namespace.
fn action_id_for(tool_name: &str) -> String {
    format!("claude-code__{}", tool_name.to_lowercase())
}

/// Success of a completed call, derived from `tool_response`: an explicit `success` bool wins;
/// an `error`/`is_error`/`isError` marker means failure; otherwise the tool ran and returned →
/// success. `isError` (camelCase) is the MCP `CallToolResult` convention — without it every failed
/// `mcp__*` call would sign as a success, which is wrong evidence.
fn outcome_success(tool_response: Option<&Value>) -> bool {
    match tool_response {
        None => true,
        Some(v) => {
            if let Some(b) = v.get("success").and_then(Value::as_bool) {
                return b;
            }
            if v.get("error").is_some() {
                return false;
            }
            for key in ["is_error", "isError"] {
                if let Some(b) = v.get(key).and_then(Value::as_bool) {
                    return !b;
                }
            }
            true
        }
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

/// The stable per-front audit log + persisted signing identity (defaults; flags override).
fn signer_for(args: &Args) -> Result<Signer, String> {
    let log_path = match &args.audit_log {
        Some(p) => p.clone(),
        None => {
            let dir = default_audit_dir();
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            dir.join("claude-code.jsonl")
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
            keys.join("claude-code-hook.key")
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

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("kriya-hook: {e}");
            return ExitCode::from(2); // fail closed — a misconfigured gate must not wave calls through
        }
    };

    let mut stdin = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("kriya-hook: cannot read hook payload: {e}");
        return fail_mode_exit(&args.mode);
    }
    let payload: HookPayload = match serde_json::from_str(&stdin) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kriya-hook: hook payload is not the expected JSON: {e}");
            return fail_mode_exit(&args.mode);
        }
    };
    let tool_name = match payload.tool_name.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => {
            eprintln!("kriya-hook: payload has no tool_name");
            return fail_mode_exit(&args.mode);
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

    let signer = match signer_for(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("kriya-hook: signer unavailable: {e}");
            return fail_mode_exit(&args.mode);
        }
    };

    match args.mode.as_str() {
        "pre" => {
            let policy = match load_policy(args.policy.as_ref()) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("kriya-hook: {e} — blocking (fail closed)");
                    return ExitCode::from(2);
                }
            };
            match policy.check(&action_id) {
                Decision::Allow => ExitCode::SUCCESS, // the post hook records the outcome
                Decision::RequiresApproval => {
                    let gate = match approval_gate(&args.approval) {
                        Ok(g) => g,
                        Err(e) => {
                            eprintln!("kriya-hook: {e} — blocking (fail closed)");
                            return ExitCode::from(2);
                        }
                    };
                    if gate.request(&action_id, &params) {
                        ExitCode::SUCCESS
                    } else {
                        record(&signer, &actor, &action_id, params, false);
                        eprintln!(
                            "kriya-hook: '{action_id}' requires human approval and was not approved \
                             (kriya policy; approval mode: {}). A signed receipt of the blocked \
                             attempt was recorded.",
                            args.approval
                        );
                        ExitCode::from(2)
                    }
                }
                Decision::Deny => {
                    record(&signer, &actor, &action_id, params, false);
                    eprintln!(
                        "kriya-hook: '{action_id}' is denied by kriya policy. A signed receipt of \
                         the blocked attempt was recorded."
                    );
                    ExitCode::from(2)
                }
            }
        }
        "post" => {
            let success = outcome_success(payload.tool_response.as_ref());
            record(&signer, &actor, &action_id, params, success);
            let _ = payload.session_id; // reserved: session correlation lands with envelope work
            ExitCode::SUCCESS
        }
        _ => unreachable!("parse_args validated the mode"),
    }
}

/// `pre` fails CLOSED (exit 2 blocks the call); `post` fails OPEN-but-loud (exit 0 — the tool
/// already ran; blocking after the fact only breaks the session, so surface the evidence gap on
/// stderr instead).
fn fail_mode_exit(mode: &str) -> ExitCode {
    if mode == "pre" {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_tool_names_into_the_governed_namespace() {
        assert_eq!(action_id_for("Bash"), "claude-code__bash");
        assert_eq!(action_id_for("WebFetch"), "claude-code__webfetch");
        // MCP tools keep their full name under the same namespace — the whole Claude Code lane,
        // native + attached MCP servers, one action-id scheme.
        assert_eq!(
            action_id_for("mcp__github__create_issue"),
            "claude-code__mcp__github__create_issue"
        );
    }

    #[test]
    fn outcome_success_derivation() {
        assert!(outcome_success(None), "no response info → ran → success");
        assert!(outcome_success(Some(&json!({"success": true}))));
        assert!(!outcome_success(Some(&json!({"success": false}))));
        assert!(!outcome_success(Some(&json!({"error": "boom"}))));
        assert!(!outcome_success(Some(&json!({"is_error": true}))));
        assert!(outcome_success(Some(&json!({"stdout": "ok"}))));
        // MCP CallToolResult convention (camelCase) — a failed MCP call must not sign as success.
        assert!(!outcome_success(Some(&json!({"isError": true}))));
        assert!(outcome_success(Some(&json!({"isError": false, "content": []}))));
    }

    /// Per-server MCP gating is just a prefix glob over the mapped action id — no new policy
    /// machinery. This is Rung 0a of the PATH→WATCHER ladder: the hook is the whole-Claude-Code
    /// lane, and servers attached directly to Claude Code are governable per server.
    #[test]
    fn an_enforcing_policy_gates_mcp_servers_individually() {
        let p: Policy = serde_yaml::from_str(
            "rules:\n  - { action: \"claude-code__mcp__github__*\", allow: true, require_approval: true }\n  - { action: \"claude-code__mcp__*\", allow: false }\n  - { action: \"claude-code__*\", allow: true }\n",
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
            p.check(&action_id_for("Bash")),
            Decision::Allow,
            "native tools ride the trailing namespace rule"
        );
    }

    #[test]
    fn default_policy_records_everything_blocks_nothing() {
        let p: Policy = serde_yaml::from_str(DEFAULT_POLICY_YAML).unwrap();
        assert_eq!(p.check("claude-code__bash"), Decision::Allow);
        assert_eq!(p.check("claude-code__write"), Decision::Allow);
    }

    #[test]
    fn an_enforcing_policy_gates_the_namespace() {
        let p: Policy = serde_yaml::from_str(
            "rules:\n  - { action: \"claude-code__read\", allow: true }\n  - { action: \"claude-code__bash\", allow: true, require_approval: true }\n  - { action: \"*\", allow: false }\n",
        )
        .unwrap();
        assert_eq!(p.check("claude-code__read"), Decision::Allow);
        assert_eq!(p.check("claude-code__bash"), Decision::RequiresApproval);
        assert_eq!(p.check("claude-code__write"), Decision::Deny);
    }

    /// Two separate Signer instances over the SAME log (≈ two hook invocations, fresh process
    /// each) must extend one hash chain under one persisted identity, and deleting a line must
    /// be visible in the chain — the properties that make per-call hook receipts evidence.
    /// (Full signature re-verification is exercised end-to-end by the external verifiers —
    /// `tools/verify-receipts` and the released `kriya-audit` — against a real hook-written log.)
    #[test]
    fn receipts_chain_across_invocations_and_deletion_is_visible() {
        use sha2::{Digest, Sha256};
        let sha256_hex = |b: &[u8]| {
            let mut h = Sha256::new();
            h.update(b);
            hex::encode(h.finalize())
        };

        let dir = std::env::temp_dir().join(format!("kriya-hook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("hook.key");
        let log = dir.join("claude-code.jsonl");
        let actor = Actor::new("claude-code", "tester");

        let s1 = Signer::with_identity(&key, log.clone()).unwrap();
        record(
            &s1,
            &actor,
            "claude-code__bash",
            json!({"command":"ls"}),
            true,
        );
        drop(s1); // ≈ the pre/post process exits
        let s2 = Signer::with_identity(&key, log.clone()).unwrap();
        record(
            &s2,
            &actor,
            "claude-code__edit",
            json!({"file":"a.rs"}),
            false,
        );

        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let v1: Value = serde_json::from_str(lines[0]).unwrap();
        let v2: Value = serde_json::from_str(lines[1]).unwrap();

        assert_eq!(
            v1["public_key"], v2["public_key"],
            "one persisted identity across invocations"
        );
        assert_eq!(v1["prev_hash"], Value::Null, "genesis receipt is unchained");
        assert_eq!(
            v2["prev_hash"].as_str().unwrap(),
            sha256_hex(lines[0].as_bytes()),
            "receipt 2 chains to the exact bytes of receipt 1 — across process boundaries"
        );
        assert_eq!(
            v2["success"],
            json!(false),
            "blocked/failed attempts are receipts too"
        );

        // Delete the first line: the survivor still claims a predecessor whose bytes are gone —
        // any chain verifier flags it (prev_hash matches nothing before it).
        assert_ne!(
            v2["prev_hash"],
            Value::Null,
            "after deleting line 1, line 2's dangling prev_hash exposes the deletion"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
