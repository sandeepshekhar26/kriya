//! Credential brokering (doc 24 §11 B13 / EG-B). The agent never holds a real credential — only a
//! placeholder `{{kriya:<alias>}}`. kriya substitutes the real secret at the egress boundary, on the
//! governed lane, from OS Keychain storage — never a plaintext secret in policy, never the
//! substituted value in a receipt, a log, or (via the hook lane's `updatedInput`) the model's own
//! context.
//!
//! ## A new trust posture
//!
//! Every other governed-lane control in this crate is kriya acting as a WITNESS: it observes an
//! action and signs a receipt about it, but never itself becomes a place where sensitive data lives.
//! Credential brokering is different in kind — kriya becomes a CUSTODIAN, briefly holding a real
//! secret in its own process memory so it can inject it. That is a genuinely new attack surface, not
//! a bigger version of an old one. See `docs/THREAT-MODEL-brokering.md` for the full treatment; the
//! short version: custody lives in macOS Keychain (encrypted at rest, gated behind the user's login
//! session), never a file kriya itself writes, and every read is scoped to the ONE alias's OWN
//! destination allowlist — never a blanket "any allowed egress" grant.
//!
//! ## Where substitution happens (and where it deliberately does NOT)
//!
//! Substitution happens in exactly two places, both named explicitly in doc 24 §11 B13: the governed
//! HTTP transport (`mcp::client::HttpTransport`, right before the wire send) and the Claude Code hook
//! lane (`bin/kriya-hook`, via the documented `hookSpecificOutput.updatedInput` mechanism). It does
//! NOT happen by mutating an action's `params` before the governor signs the action receipt or before
//! `ActionExecutor::execute` runs — both of those capture `params` verbatim, so mutating it there
//! would put the real secret into the action receipt, defeating the entire feature. The governor's
//! OWN pre-check (in `mcp::governor::egress_gate`) only ever inspects which ALIASES are named and
//! whether they're allowed for the destination — it never reads a secret value at all. Reading a
//! value happens exactly once per substitution, as late as possible, at the transport that is about
//! to send.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::permissions::host_matches;

/// B13: credential brokering. Maps the agent-visible placeholder `{{kriya:<alias>}}` to a reference
/// into OS Keychain storage — NEVER a plaintext secret in this struct, and the loader resolves the
/// real value at runtime only. Absent by default (`Policy.secrets: None`): brokering never activates
/// unless the operator explicitly configures at least one alias.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SecretsPolicy {
    #[serde(default)]
    pub aliases: Vec<SecretAlias>,
}

/// One brokered alias. `keychain_service`/`keychain_account` are a REFERENCE (macOS Keychain
/// generic-password item coordinates), never the secret itself.
#[derive(Debug, Clone, Deserialize)]
pub struct SecretAlias {
    /// The name inside `{{kriya:<name>}}` — matched exactly, case-sensitively.
    pub alias: String,
    pub keychain_service: String,
    pub keychain_account: String,
    /// Host patterns (the same syntax as an egress rule: `*` / `*.domain` / exact) this alias may be
    /// substituted INTO. A placeholder whose destination matches none of these is DENIED, never
    /// substituted — brokering never rides a blanket "any allowed egress" grant; it has its OWN,
    /// independent (and typically narrower) scope from the general egress tier.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

impl SecretsPolicy {
    pub fn find(&self, alias: &str) -> Option<&SecretAlias> {
        self.aliases.iter().find(|a| a.alias == alias)
    }
}

impl SecretAlias {
    pub fn allows_host(&self, host: &str) -> bool {
        self.allowed_hosts
            .iter()
            .any(|pattern| host_matches(pattern, host))
    }
}

const PREFIX: &str = "{{kriya:";
const SUFFIX: &str = "}}";

fn is_alias_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// Find every distinct alias named by a `{{kriya:<alias>}}` placeholder in `s`, in first-seen order,
/// deduplicated. A malformed placeholder (unterminated, empty alias, or a disallowed character inside
/// it) is NOT matched — a literal `{{kriya:` a tool argument happens to contain that isn't really a
/// placeholder must never be treated as one. Pure and side-effect-free: used by the governor's
/// pre-check, which must never touch a secret value, only ask "which aliases are named here."
pub fn find_placeholder_aliases(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find(PREFIX) {
        let after_prefix = &rest[start + PREFIX.len()..];
        let Some(end) = after_prefix.find(SUFFIX) else {
            break; // unterminated — nothing further in the string can close it either.
        };
        let alias = &after_prefix[..end];
        if !alias.is_empty() && alias.chars().all(is_alias_char) && !out.iter().any(|a| a == alias)
        {
            out.push(alias.to_string());
        }
        rest = &after_prefix[end + SUFFIX.len()..];
    }
    out
}

/// Substitute every well-formed `{{kriya:<alias>}}` placeholder in `s`, resolving each alias via
/// `resolve`. Returns `Ok(None)` when `s` contains no placeholders at all (the caller should send the
/// ORIGINAL bytes unchanged — never allocate a needless copy on the hot path). Fails closed on the
/// FIRST alias `resolve` can't supply a value for: a partially-substituted payload (some secrets
/// injected, one missing) must never be sent, so this never returns a partial result — `Err` carries
/// the alias name that failed, for the caller's error message.
pub fn substitute_placeholders(
    s: &str,
    mut resolve: impl FnMut(&str) -> Option<Zeroizing<String>>,
) -> Result<Option<Zeroizing<String>>, String> {
    if find_placeholder_aliases(s).is_empty() {
        return Ok(None);
    }
    let mut out = Zeroizing::new(String::with_capacity(s.len()));
    let mut rest = s;
    loop {
        let Some(start) = rest.find(PREFIX) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_prefix = &rest[start + PREFIX.len()..];
        let Some(end) = after_prefix.find(SUFFIX) else {
            // Unterminated tail — pass it through literally, exactly like `find_placeholder_aliases`
            // treats it as a non-match.
            out.push_str(&rest[start..]);
            break;
        };
        let alias = &after_prefix[..end];
        if !alias.is_empty() && alias.chars().all(is_alias_char) {
            let value = resolve(alias).ok_or_else(|| alias.to_string())?;
            out.push_str(&value);
        } else {
            // Not a well-formed placeholder (empty or a disallowed char) — literal passthrough.
            out.push_str(&rest[start..start + PREFIX.len() + end + SUFFIX.len()]);
        }
        rest = &after_prefix[end + SUFFIX.len()..];
    }
    Ok(Some(out))
}

/// Read one secret from the macOS login Keychain (`kSecClassGenericPassword`). Shells out to the
/// system `/usr/bin/security` CLI rather than linking `Security.framework` directly — `Command`
/// passes `service`/`account` as separate argv entries (no shell is invoked), so this is not
/// susceptible to shell injection even though those strings come from operator-authored policy. Zeros
/// the raw output buffer immediately after extracting the trimmed value, and wraps the result in
/// `Zeroizing` from the moment it exists — best-effort in-process hygiene, not an airtight defense
/// against a sophisticated memory-forensics attacker with kernel access (see the threat model doc).
#[cfg(target_os = "macos")]
pub fn read_keychain_secret(service: &str, account: &str) -> Result<Zeroizing<String>, String> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .map_err(|e| format!("keychain lookup failed to spawn: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "keychain lookup failed for service='{service}' account='{account}' \
             (item not found, access denied, or the keychain is locked)"
        ));
    }
    let mut raw = output.stdout;
    let result = std::str::from_utf8(&raw)
        .map(|s| s.trim_end_matches(['\n', '\r']).to_string())
        .map_err(|_| "keychain value is not valid UTF-8".to_string());
    raw.zeroize();
    result.map(Zeroizing::new)
}

#[cfg(not(target_os = "macos"))]
pub fn read_keychain_secret(_service: &str, _account: &str) -> Result<Zeroizing<String>, String> {
    Err(
        "credential brokering requires macOS Keychain, which isn't available on this platform"
            .to_string(),
    )
}

/// The transport-side enforcement (doc 24 §11 B13): substitute every placeholder in `body` bound for
/// `host`, reading each secret from Keychain fresh (never cached — minimizes how long a real value
/// exists in process memory). This is the ACTUAL enforcement; the governor's own pre-check is a
/// defense-in-depth belt to this suspenders, exactly [`crate::permissions`]'s SSRF guard's dual-layer
/// shape (a governor-level check for clean receipts, a transport-level check that's the real gate).
/// Fails closed on ANY alias that's unconfigured, out of scope for `host`, or whose keychain read
/// fails — never sends a partially-substituted body, and never falls back to sending the literal
/// placeholder text.
pub fn broker_body(
    body: &str,
    secrets: &SecretsPolicy,
    host: &str,
) -> Result<Zeroizing<String>, String> {
    match substitute_placeholders(body, |alias| {
        let entry = secrets.find(alias)?;
        if !entry.allows_host(host) {
            return None;
        }
        let raw = read_keychain_secret(&entry.keychain_service, &entry.keychain_account).ok()?;
        Some(Zeroizing::new(json_escape_inner(&raw)))
    }) {
        Ok(Some(substituted)) => Ok(substituted),
        Ok(None) => Ok(Zeroizing::new(body.to_string())),
        Err(alias) => Err(format!(
            "credential brokering: alias '{alias}' is not configured, not allowed for destination \
             '{host}', or its keychain item could not be read"
        )),
    }
}

/// Escape `s` for splicing into an EXISTING JSON string literal's content — the returned text has NO
/// surrounding quotes (the caller's own JSON text already supplies those). Every brokered
/// substitution goes through this: a placeholder always sits inside a JSON string value (a JSON-RPC
/// body, a hook `tool_input` field), and a secret containing `"` / `\` / control characters spliced
/// in raw would corrupt the JSON or, worse, let the secret's own content break out of its intended
/// string field and inject structure.
pub fn json_escape_inner(s: &str) -> String {
    let quoted = serde_json::to_string(s).unwrap_or_default();
    quoted
        .get(1..quoted.len().saturating_sub(1))
        .unwrap_or_default()
        .to_string()
}

/// The inverse of substitution — a safety net for a lane (the Claude Code hook) where kriya cannot
/// be certain whether an echoed `tool_input` reflects the pre- or post-`updatedInput` form. Redacts
/// every configured alias's REAL value back to its `{{kriya:<alias>}}` placeholder, recursively, over
/// every string in `v`. Regardless of which form `v` turns out to be: if the real value was never
/// present, every replace is a harmless no-op; if it WAS present, it's stripped before anything
/// downstream (a receipt, a hash) ever sees it. Reads every configured alias's value fresh (bounded
/// by the small number of aliases an operator configures) — never persisted, never cached.
pub fn redact_broker_values(v: &Value, secrets: &SecretsPolicy) -> Value {
    redact_broker_values_with(v, secrets, |service, account| {
        read_keychain_secret(service, account).ok()
    })
}

/// [`redact_broker_values`] with an injectable resolver — the pure core, unit-testable without
/// touching the real Keychain. `resolve` is called once per configured alias with
/// `(keychain_service, keychain_account)`.
pub fn redact_broker_values_with(
    v: &Value,
    secrets: &SecretsPolicy,
    mut resolve: impl FnMut(&str, &str) -> Option<Zeroizing<String>>,
) -> Value {
    let live: Vec<(String, Zeroizing<String>)> = secrets
        .aliases
        .iter()
        .filter_map(|a| {
            resolve(&a.keychain_service, &a.keychain_account).map(|val| (a.alias.clone(), val))
        })
        .collect();
    redact_value(v, &live)
}

fn redact_value(v: &Value, live: &[(String, Zeroizing<String>)]) -> Value {
    match v {
        Value::String(s) => {
            let mut out = s.clone();
            for (alias, real) in live {
                if !real.is_empty() && out.contains(real.as_str()) {
                    out = out.replace(real.as_str(), &format!("{{{{kriya:{alias}}}}}"));
                }
            }
            Value::String(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|i| redact_value(i, live)).collect()),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, val) in map {
                out.insert(k.clone(), redact_value(val, live));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// Every `SecretAlias.keychain_service`/`.keychain_account` pair declared in `secrets`, keyed by
/// alias — a convenience for tests and the Console's "which secrets does this policy reference"
/// summary. Never touches Keychain; carries no values.
pub fn alias_references(secrets: &SecretsPolicy) -> HashMap<String, (String, String)> {
    secrets
        .aliases
        .iter()
        .map(|a| {
            (
                a.alias.clone(),
                (a.keychain_service.clone(), a.keychain_account.clone()),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_placeholder_aliases_extracts_distinct_names_in_order() {
        let s = r#"{"a": "{{kriya:github_pat}}", "b": "{{kriya:slack_token}}", "c": "{{kriya:github_pat}}"}"#;
        assert_eq!(
            find_placeholder_aliases(s),
            vec!["github_pat", "slack_token"]
        );
    }

    #[test]
    fn find_placeholder_aliases_ignores_malformed_shapes() {
        assert_eq!(
            find_placeholder_aliases("no placeholder here"),
            Vec::<String>::new()
        );
        assert_eq!(
            find_placeholder_aliases("{{kriya:}}"),
            Vec::<String>::new(),
            "empty alias"
        );
        assert_eq!(
            find_placeholder_aliases("{{kriya:unterminated"),
            Vec::<String>::new(),
            "unterminated placeholder must not match"
        );
        assert_eq!(
            find_placeholder_aliases("{{kriya:has space}}"),
            Vec::<String>::new(),
            "a space is not an allowed alias character"
        );
    }

    #[test]
    fn substitute_placeholders_returns_none_when_nothing_to_substitute() {
        let result = substitute_placeholders("plain text, no placeholders", |_| None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn substitute_placeholders_replaces_every_occurrence() {
        let s = "token={{kriya:github_pat}}&again={{kriya:github_pat}}";
        let result = substitute_placeholders(s, |alias| {
            assert_eq!(alias, "github_pat");
            Some(Zeroizing::new("REAL_SECRET_VALUE".to_string()))
        })
        .unwrap()
        .unwrap();
        assert_eq!(&*result, "token=REAL_SECRET_VALUE&again=REAL_SECRET_VALUE");
    }

    #[test]
    fn substitute_placeholders_fails_closed_never_returns_a_partial_result() {
        let s = "{{kriya:known}} and {{kriya:unknown}}";
        let err = substitute_placeholders(s, |alias| {
            if alias == "known" {
                Some(Zeroizing::new("SECRET".to_string()))
            } else {
                None
            }
        })
        .unwrap_err();
        assert_eq!(err, "unknown");
    }

    #[test]
    fn substitute_placeholders_preserves_malformed_shapes_literally() {
        let s = "prefix {{kriya:}} suffix";
        let result = substitute_placeholders(s, |_| {
            panic!("resolve must not be called for an empty alias")
        });
        // An empty alias isn't a real placeholder — find_placeholder_aliases returns nothing for it,
        // so substitute_placeholders takes the Ok(None) fast path and never calls resolve at all.
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn secret_alias_allows_host_reuses_the_egress_host_matcher() {
        let alias = SecretAlias {
            alias: "github_pat".to_string(),
            keychain_service: "kriya".to_string(),
            keychain_account: "github_pat".to_string(),
            allowed_hosts: vec!["*.github.com".to_string()],
        };
        assert!(alias.allows_host("api.github.com"));
        assert!(alias.allows_host("github.com"));
        assert!(!alias.allows_host("evil.example.com"));
    }

    #[test]
    fn secrets_policy_find_is_case_sensitive_exact_match() {
        let policy = SecretsPolicy {
            aliases: vec![SecretAlias {
                alias: "github_pat".to_string(),
                keychain_service: "kriya".to_string(),
                keychain_account: "github_pat".to_string(),
                allowed_hosts: vec!["api.github.com".to_string()],
            }],
        };
        assert!(policy.find("github_pat").is_some());
        assert!(policy.find("GITHUB_PAT").is_none());
        assert!(policy.find("unknown").is_none());
    }

    #[test]
    fn broker_body_denies_an_alias_not_scoped_to_the_destination_host() {
        let policy = SecretsPolicy {
            aliases: vec![SecretAlias {
                alias: "github_pat".to_string(),
                keychain_service: "kriya".to_string(),
                keychain_account: "github_pat".to_string(),
                allowed_hosts: vec!["api.github.com".to_string()],
            }],
        };
        let body = r#"{"token":"{{kriya:github_pat}}"}"#;
        let err = broker_body(body, &policy, "evil.example.com").unwrap_err();
        assert!(err.contains("github_pat"));
        assert!(err.contains("evil.example.com"));
        // The literal placeholder text must never leak into an error message either.
        assert!(!err.contains("{{kriya:"));
    }

    #[test]
    fn broker_body_denies_an_unconfigured_alias() {
        let policy = SecretsPolicy::default();
        let body = r#"{"token":"{{kriya:nope}}"}"#;
        let err = broker_body(body, &policy, "api.example.com").unwrap_err();
        assert!(err.contains("nope"));
    }

    #[test]
    fn broker_body_passes_through_a_body_with_no_placeholders_unchanged() {
        let policy = SecretsPolicy::default();
        let body = r#"{"q":"list widgets"}"#;
        let result = broker_body(body, &policy, "api.example.com").unwrap();
        assert_eq!(&*result, body);
    }

    #[test]
    fn json_escape_inner_has_no_surrounding_quotes_and_escapes_special_characters() {
        assert_eq!(json_escape_inner("plain"), "plain");
        assert_eq!(json_escape_inner(r#"has"quote"#), r#"has\"quote"#);
        assert_eq!(json_escape_inner("has\\backslash"), "has\\\\backslash");
        assert_eq!(json_escape_inner("line\nbreak"), "line\\nbreak");
    }

    #[test]
    fn substitution_via_json_escape_inner_survives_a_value_with_json_special_characters() {
        // A secret with an embedded quote spliced in RAW would corrupt the JSON (or worse, let the
        // secret's own content break out of its string field). This is the regression test for that
        // — the exact escaping path `broker_body` (and kriya-hook's resolver) both use.
        let secret = r#"va"lue\with/slashes"#;
        let body = r#"{"token":"{{kriya:tricky}}"}"#;
        let result = substitute_placeholders(body, |alias| {
            assert_eq!(alias, "tricky");
            Some(Zeroizing::new(json_escape_inner(secret)))
        })
        .unwrap()
        .unwrap();
        // The result must be VALID JSON, and decode back to the exact original secret.
        let parsed: Value =
            serde_json::from_str(&result).expect("substituted body must be valid JSON");
        assert_eq!(parsed["token"].as_str().unwrap(), secret);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn broker_body_round_trips_a_tricky_secret_through_the_real_keychain() {
        let service = "kriya-test-service";
        let account = "kriya-test-tricky";
        // Embedded quote + backslash — enough to corrupt naive JSON splicing; a literal newline is
        // covered separately by the pure `json_escape_inner` unit test above (argv doesn't carry
        // embedded newlines cleanly, which would conflate a CLI quirk with what this test checks).
        let secret = r#"va"lue\with/slashes"#;

        let _ = std::process::Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .output();
        let add = std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-s",
                service,
                "-a",
                account,
                "-w",
                secret,
                "-U",
            ])
            .output()
            .expect("spawn security add-generic-password");
        assert!(
            add.status.success(),
            "failed to seed the test keychain item: {add:?}"
        );

        let policy = SecretsPolicy {
            aliases: vec![SecretAlias {
                alias: "tricky".to_string(),
                keychain_service: service.to_string(),
                keychain_account: account.to_string(),
                allowed_hosts: vec!["api.example.com".to_string()],
            }],
        };
        let body = r#"{"token":"{{kriya:tricky}}"}"#;
        let result =
            broker_body(body, &policy, "api.example.com").expect("broker_body should clear");
        let parsed: Value =
            serde_json::from_str(&result).expect("substituted body must be valid JSON");
        assert_eq!(parsed["token"].as_str().unwrap(), secret);

        let _ = std::process::Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .output();
    }

    #[test]
    fn redact_broker_values_with_replaces_a_real_value_anywhere_in_the_tree() {
        let secrets = SecretsPolicy {
            aliases: vec![SecretAlias {
                alias: "github_pat".to_string(),
                keychain_service: "kriya".to_string(),
                keychain_account: "github_pat".to_string(),
                allowed_hosts: vec!["api.github.com".to_string()],
            }],
        };
        let payload = serde_json::json!({
            "headers": { "Authorization": "Bearer ghp_realsecretvalue123" },
            "nested": ["ghp_realsecretvalue123", "unrelated"],
        });
        let redacted = redact_broker_values_with(&payload, &secrets, |_service, _account| {
            Some(Zeroizing::new("ghp_realsecretvalue123".to_string()))
        });
        let s = redacted.to_string();
        assert!(
            !s.contains("ghp_realsecretvalue123"),
            "the real value must be gone: {s}"
        );
        assert!(s.contains("{{kriya:github_pat}}"));
        assert_eq!(
            redacted["nested"][1], "unrelated",
            "unrelated strings must survive untouched"
        );
    }

    #[test]
    fn redact_broker_values_with_is_a_no_op_when_the_real_value_is_absent() {
        let secrets = SecretsPolicy {
            aliases: vec![SecretAlias {
                alias: "github_pat".to_string(),
                keychain_service: "kriya".to_string(),
                keychain_account: "github_pat".to_string(),
                allowed_hosts: vec!["api.github.com".to_string()],
            }],
        };
        let payload = serde_json::json!({"q": "list widgets, nothing secret here"});
        let redacted = redact_broker_values_with(&payload, &secrets, |_, _| {
            Some(Zeroizing::new("ghp_realsecretvalue123".to_string()))
        });
        assert_eq!(
            redacted, payload,
            "no match anywhere → byte-identical passthrough"
        );
    }

    /// Real integration test against the ACTUAL macOS login Keychain — not just the pure logic
    /// above. `#[ignore]`d like `mcp::reachin::macos::tests::real_ax_snapshot_of_finder_when_trusted`
    /// (needs real OS state / may prompt); run explicitly with `--ignored`. Creates, reads, and
    /// deletes a throwaway item under a `kriya-test-*` service/account so it never collides with a
    /// real credential — this is the test that actually proves `security find-generic-password -w`'s
    /// output shape (trailing newline, exact stdout bytes) is parsed the way the code above assumes.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn read_keychain_secret_round_trips_against_the_real_macos_keychain() {
        let service = "kriya-test-service";
        let account = "kriya-test-account";
        let value = "test-secret-value-12345";

        let _ = std::process::Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .output(); // best-effort pre-clean; ignore "not found"

        let add = std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-s",
                service,
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
            "failed to seed the test keychain item: {add:?}"
        );

        let read = read_keychain_secret(service, account).expect("read back the seeded item");
        assert_eq!(&*read, value);

        let _ = std::process::Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .output();
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn read_keychain_secret_fails_cleanly_for_a_missing_item() {
        let err =
            read_keychain_secret("kriya-test-definitely-does-not-exist", "nobody").unwrap_err();
        assert!(err.contains("keychain lookup failed"));
    }
}
