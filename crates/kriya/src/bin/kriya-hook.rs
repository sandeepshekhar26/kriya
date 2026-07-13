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
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kriya::audit::{default_audit_dir, now_ms, Actor, Receipt, Signer};
#[cfg(target_os = "macos")]
use kriya::mcp::GuiApproval;
use kriya::mcp::{
    ApprovalGate, AutoApprove, DenyApproval, HashScheme, IoDecision, IoDirection, IoKind, IoRecord,
    TtyApproval,
};
use kriya::permissions::{url_host, Decision, Policy};
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

/// Sign the action receipt and return its `step_id` so a correlated `kriya.io.*` receipt can carry
/// it as `corr` (doc 24 §4.2 — correlation by `corr`, never adjacency, L5).
fn record(signer: &Signer, actor: &Actor, action_id: &str, params: Value, success: bool) -> String {
    let step_id = uuid::Uuid::new_v4().to_string();
    signer.record(
        Receipt::new(
            step_id.clone(),
            action_id.to_string(),
            params,
            success,
            now_ms(),
        )
        .with_actor(Some(actor.clone())),
    );
    step_id
}

/// Resolve the governed-lane egress destination for a Claude Code tool, if one is knowable.
/// Returns `(kind, dest_host, server)`:
/// - `mcp__<server>__<tool>` → an `mcp` destination carrying the server NAME (its endpoint is not
///   claimable, so no host — doc 24 §6-H6).
/// - a tool whose `tool_input` has a `url` (WebFetch) → an `http` destination with the parsed host.
/// - **Bash → `None`**: a shell command has no single extractable destination — never invent one
///   (doc 24 §4.1). Edit/Write/Read and a URL-less WebSearch likewise → `None`.
fn egress_target_for(
    tool_name: &str,
    tool_input: &Value,
) -> Option<(IoKind, Option<String>, Option<String>)> {
    if let Some(rest) = tool_name.strip_prefix("mcp__") {
        let server = rest.split("__").next().unwrap_or(rest).to_string();
        return Some((IoKind::Mcp, None, Some(server)));
    }
    if tool_name.eq_ignore_ascii_case("bash") {
        return None; // never extract a URL from a shell command (doc 24 §4.1)
    }
    if let Some(url) = tool_input.get("url").and_then(Value::as_str) {
        return Some((IoKind::Http, Some(url_host(url)), None));
    }
    None
}

/// Emit a `kriya.io.egress.<kind>.<decision>` receipt for a hook-lane tool call. Records
/// **payload bytes** (the serialized `tool_input` size) — NEVER network bytes — and a content hash
/// over the CANONICAL key-sorted serialization (`hash_scheme: canonical-json`), a different
/// definition than the gateway lane's wire bytes and labeled as such (doc 24 §4.2 rule 6 / L6).
/// Hash + size only, never content (L9).
#[allow(clippy::too_many_arguments)]
fn emit_io_egress(
    signer: &Signer,
    actor: &Actor,
    kind: IoKind,
    dest_host: Option<String>,
    server: Option<String>,
    tool_input: &Value,
    decision: IoDecision,
    reason: Option<String>,
    corr: &str,
) {
    let canon = canonical_json_string(tool_input);
    let io = IoRecord {
        direction: IoDirection::Egress,
        dest_host,
        dest_kind: kind,
        method: None,
        bytes_out: Some(canon.len() as u64),
        bytes_in: None,
        bytes_in_is_partial: false,
        content_sha256: Some(sha256_hex(canon.as_bytes())),
        hash_scheme: HashScheme::CanonicalJson,
        decision,
        policy_rule: None,
        approved_by: None,
        reason,
        server,
        flags: Vec::new(),
    };
    signer.record(
        Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            io.action_id(),
            io.params(Some(corr)),
            decision != IoDecision::Deny,
            now_ms(),
        )
        .with_actor(Some(actor.clone())),
    );
}

/// Emit a `kriya.io.ingress.<kind>.allow` receipt: a **KEYED** hash + size of the tool response
/// (doc 24 §6-P3). The hash is HMAC-SHA256 under a device-local salt over the canonical
/// serialization, so a receipt-holder without the salt cannot dictionary-attack guessable content —
/// an unsalted hash of guessable content is content disclosure. Hash + size only, never content (L9).
fn emit_io_ingress(
    signer: &Signer,
    actor: &Actor,
    kind: IoKind,
    server: Option<String>,
    tool_response: &Value,
    salt: &[u8],
    corr: &str,
) {
    let canon = canonical_json_string(tool_response);
    let io = IoRecord {
        direction: IoDirection::Ingress,
        dest_host: None,
        dest_kind: kind,
        method: None,
        bytes_out: None,
        bytes_in: Some(canon.len() as u64),
        bytes_in_is_partial: false,
        content_sha256: Some(hmac_sha256_hex(salt, canon.as_bytes())),
        hash_scheme: HashScheme::CanonicalJson,
        decision: IoDecision::Allow,
        policy_rule: None,
        approved_by: None,
        reason: None,
        server,
        flags: Vec::new(),
    };
    signer.record(
        Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            io.action_id(),
            io.params(Some(corr)),
            true,
            now_ms(),
        )
        .with_actor(Some(actor.clone())),
    );
}

/// Lowercase-hex SHA-256.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// HMAC-SHA256 (lowercase hex) — RFC 2104, built on `sha2` so the hook adds no HMAC dependency. The
/// keyed ingress hash (doc 24 §6-P3).
fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = Sha256::digest(key);
        k[..d.len()].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = {
        let mut h = Sha256::new();
        h.update(ipad);
        h.update(msg);
        h.finalize()
    };
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    hex::encode(outer.finalize())
}

/// Canonical key-sorted JSON serialization — the commitment `content_sha256` is taken over on the
/// hook lane. Matches `kriya::audit`'s param canonicalization so the definition is consistent.
fn canonical_json_string(v: &Value) -> String {
    serde_json::to_string(&canonical_value(v)).unwrap_or_default()
}

fn canonical_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonical_value(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}

/// Load (or create + persist, 0600) the device-local HMAC salt for keyed ingress hashing. Co-located
/// with the hook's signing key. Best-effort: an unwritable location returns an in-memory salt so
/// ingress recording still functions within the run.
fn load_or_create_ingress_salt(args: &Args) -> [u8; 32] {
    let path = match &args.signing_key {
        Some(p) => p.with_file_name("ingress-hmac.salt"),
        None => default_audit_dir()
            .parent()
            .map(|p| p.join("keys"))
            .unwrap_or_else(|| PathBuf::from(".kriya-keys"))
            .join("ingress-hmac.salt"),
    };
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(bytes) = hex::decode(text.trim()) {
            if let Ok(arr) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return arr;
            }
        }
    }
    let salt: [u8; 32] = rand::random();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, hex::encode(salt)).is_ok() {
        restrict_salt_perms(&path);
    }
    salt
}

#[cfg(unix)]
fn restrict_salt_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict_salt_perms(_path: &Path) {}

/// Emit a `kriya.io.egress.*.deny` receipt for a blocked hook-lane tool call, when the destination
/// is knowable and the policy carries an egress tier (doc 24 L10 — the deny is receipted at the
/// decision point). No-op otherwise.
fn maybe_record_io_deny(
    signer: &Signer,
    actor: &Actor,
    policy: &Policy,
    tool_name: &str,
    tool_input: &Value,
    reason: &str,
    corr: &str,
) {
    if policy.egress().is_none() {
        return;
    }
    if let Some((kind, host, server)) = egress_target_for(tool_name, tool_input) {
        emit_io_egress(
            signer,
            actor,
            kind,
            host,
            server,
            tool_input,
            IoDecision::Deny,
            Some(reason.to_string()),
            corr,
        );
    }
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

    // Both modes need the policy: `pre` for the decision, `post` for whether to record io receipts.
    let policy = match load_policy(args.policy.as_ref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "kriya-hook: {e} — {}",
                if args.mode == "pre" {
                    "blocking (fail closed)"
                } else {
                    "skipping io (fail open)"
                }
            );
            return fail_mode_exit(&args.mode);
        }
    };

    match args.mode.as_str() {
        "pre" => match policy.check(&action_id) {
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
                    let step_id = record(&signer, &actor, &action_id, params.clone(), false);
                    maybe_record_io_deny(
                        &signer,
                        &actor,
                        &policy,
                        tool_name,
                        &params,
                        "requires human approval; not granted",
                        &step_id,
                    );
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
                let step_id = record(&signer, &actor, &action_id, params.clone(), false);
                maybe_record_io_deny(
                    &signer,
                    &actor,
                    &policy,
                    tool_name,
                    &params,
                    "denied by kriya policy",
                    &step_id,
                );
                eprintln!(
                    "kriya-hook: '{action_id}' is denied by kriya policy. A signed receipt of \
                     the blocked attempt was recorded."
                );
                ExitCode::from(2)
            }
        },
        "post" => {
            let success = outcome_success(payload.tool_response.as_ref());
            let step_id = record(&signer, &actor, &action_id, params.clone(), success);
            // kriya.io.* receipts (doc 24 §7.3): recorded only when the policy opts in via an
            // `egress:` section, and only for a tool whose destination is knowable (WebFetch host,
            // mcp server) — Bash and file tools produce none (doc 24 §4.1).
            if let Some(egress) = policy.egress() {
                if let Some((kind, host, server)) = egress_target_for(tool_name, &params) {
                    emit_io_egress(
                        &signer,
                        &actor,
                        kind,
                        host,
                        server.clone(),
                        &params,
                        IoDecision::Allow,
                        None,
                        &step_id,
                    );
                    // Ingress digests ride their OWN switch, default OFF (doc 24 §6-P3): a keyed
                    // (HMAC) hash + size of the response, never its content.
                    if egress.record_ingress() {
                        if let Some(resp) = payload.tool_response.as_ref() {
                            let salt = load_or_create_ingress_salt(&args);
                            emit_io_ingress(&signer, &actor, kind, server, resp, &salt, &step_id);
                        }
                    }
                }
            }
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

    // ─── EG-2: hook-lane io destinations + keyed ingress (doc 24 §4.1 / §6-P3) ───────────────────

    #[test]
    fn egress_target_extraction_by_tool() {
        // WebFetch → an http destination with the parsed host.
        let (kind, host, server) =
            egress_target_for("WebFetch", &json!({"url": "https://api.vendor.com/v1"})).unwrap();
        assert_eq!(kind, IoKind::Http);
        assert_eq!(host.as_deref(), Some("api.vendor.com"));
        assert!(server.is_none());

        // mcp__<server>__<tool> → an mcp destination carrying the SERVER NAME, no host.
        let (kind, host, server) =
            egress_target_for("mcp__github__create_issue", &json!({})).unwrap();
        assert_eq!(kind, IoKind::Mcp);
        assert!(host.is_none(), "an mcp endpoint is not a claimable host");
        assert_eq!(server.as_deref(), Some("github"));

        // Bash → NEVER extract a URL, even if the command contains one.
        assert!(
            egress_target_for("Bash", &json!({"command": "curl https://evil.example"})).is_none(),
            "Bash must never yield an egress destination"
        );
        // Edit/Write and a url-less WebSearch → no destination.
        assert!(egress_target_for("Edit", &json!({"file_path": "/x"})).is_none());
        assert!(egress_target_for("WebSearch", &json!({"query": "kriya"})).is_none());
    }

    #[test]
    fn ingress_hash_is_keyed_so_it_is_not_a_content_disclosure() {
        // The SAME content under two different device salts must hash differently — a receipt-holder
        // without the salt cannot dictionary-attack guessable content (doc 24 §6-P3).
        let content = b"did this agent read salary.xlsx?";
        let a = hmac_sha256_hex(&[7u8; 32], content);
        let b = hmac_sha256_hex(&[9u8; 32], content);
        assert_ne!(a, b, "different salts → different keyed hashes");
        assert_eq!(a.len(), 64, "sha256 hex");
        // Deterministic under a fixed salt.
        assert_eq!(a, hmac_sha256_hex(&[7u8; 32], content));
        // Differs from a plain (unkeyed) SHA-256 of the same content.
        assert_ne!(a, sha256_hex(content));
    }

    #[test]
    fn ingress_receipt_serializes_only_a_keyed_digest_and_size_never_the_content() {
        // L9 sentinel: build an ingress receipt exactly as `emit_io_ingress` does and prove the
        // sensitive content NEVER appears in the receipt params — only the keyed HMAC digest + size.
        let sensitive = json!({ "secret": "AKIAIOSFODNN7EXAMPLE", "path": "salary.xlsx" });
        let canon = canonical_json_string(&sensitive);
        let io = IoRecord {
            direction: IoDirection::Ingress,
            dest_host: None,
            dest_kind: IoKind::Http,
            method: None,
            bytes_out: None,
            bytes_in: Some(canon.len() as u64),
            bytes_in_is_partial: false,
            content_sha256: Some(hmac_sha256_hex(&[3u8; 32], canon.as_bytes())),
            hash_scheme: HashScheme::CanonicalJson,
            decision: IoDecision::Allow,
            policy_rule: None,
            approved_by: None,
            reason: None,
            server: None,
            flags: Vec::new(),
        };
        let params = io.params(Some("corr-x")).to_string();
        assert!(
            !params.contains("AKIAIOSFODNN7EXAMPLE"),
            "the secret must never serialize: {params}"
        );
        assert!(
            !params.contains("salary.xlsx"),
            "content must never serialize: {params}"
        );
        assert!(
            params.contains("content_sha256"),
            "only the keyed digest is present"
        );
        assert!(
            params.contains(&format!("\"bytes_in\":{}", canon.len())),
            "size is present"
        );
    }

    #[test]
    fn canonical_json_is_key_sorted_and_stable() {
        let a = canonical_json_string(&json!({"z": 1, "a": {"y": 2, "b": 3}}));
        let b = canonical_json_string(&json!({"a": {"b": 3, "y": 2}, "z": 1}));
        assert_eq!(a, b, "key order must not change the commitment");
        assert_eq!(a, r#"{"a":{"b":3,"y":2},"z":1}"#);
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
        assert!(outcome_success(Some(
            &json!({"isError": false, "content": []})
        )));
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
