//! Minimal but real permission layer. Every agent action is checked against a policy
//! before the host asks the app to run it. Default is deny-unknown; deletes require
//! human approval (no approval queue exists yet in Phase 0, so they are held/denied).

use serde::Deserialize;

use crate::agent::inference::retry::RetryPolicy;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    RequiresApproval,
    Deny,
}

#[derive(Debug, Clone, Deserialize)]
struct Rule {
    /// Exact action id, a `prefix_*` glob, or `*` for all.
    action: String,
    #[serde(default)]
    allow: bool,
    #[serde(default)]
    require_approval: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Budget {
    /// Max actions the agent may take in any trailing 60-second window. `None` = no cap.
    #[serde(default)]
    pub max_actions_per_minute: Option<u32>,
    /// Max inference/API calls the agent may make in any trailing 60-minute window. `None` = no
    /// cap. Independent of the per-minute action cap: this bounds model *cost* (each agent step is
    /// one backend call, possibly paid/remote), not action bursts against the app.
    #[serde(default)]
    pub max_api_calls_per_hour: Option<u32>,
}

/// Tunes how a *transient* inference-backend error is retried before the host gives up on a
/// step (R10). Optional in policy: when absent the host uses [`RetryPolicy::default`]. Lets an
/// operator dial reliability (e.g. a flaky local model vs. an expensive rate-limited cloud model)
/// without code changes. Has no effect on deterministic/scripted backends — they never error.
#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    /// Retries after the first attempt. `0` = fail-fast (one attempt). Total attempts = this + 1.
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Backoff in milliseconds before the first retry; doubles each retry (capped by `max_backoff_ms`).
    #[serde(default)]
    pub initial_backoff_ms: Option<u64>,
    /// Upper bound on a single backoff wait, in milliseconds.
    #[serde(default)]
    pub max_backoff_ms: Option<u64>,
}

impl RetryConfig {
    /// Fold this config onto [`RetryPolicy::default`], overriding only the fields that are set.
    fn to_policy(&self) -> RetryPolicy {
        let mut p = RetryPolicy::default();
        if let Some(n) = self.max_retries {
            p.max_retries = n;
        }
        if let Some(ms) = self.initial_backoff_ms {
            p.initial_backoff = Duration::from_millis(ms);
        }
        if let Some(ms) = self.max_backoff_ms {
            p.max_backoff = Duration::from_millis(ms);
        }
        p
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Policy {
    rules: Vec<Rule>,
    #[serde(default)]
    budget: Budget,
    /// Optional retry/backoff tuning for transient inference failures (R10).
    #[serde(default)]
    retry: Option<RetryConfig>,
    /// On-device guarantee (R13). When `true`, the in-process host refuses to run with an
    /// inference backend that egresses to a remote service, and signs an attestation that the
    /// run was sealed — the "nothing leaves the device" posture regulated apps need. Default
    /// `false` (off, fully backward compatible).
    #[serde(default)]
    on_device: bool,
}

impl Default for Policy {
    fn default() -> Self {
        // Allow writes/creates/edits; deletes need approval; everything else denied.
        Policy {
            rules: vec![
                Rule { action: "create_*".into(), allow: true, require_approval: false },
                Rule { action: "edit_*".into(), allow: true, require_approval: false },
                Rule { action: "delete_*".into(), allow: true, require_approval: true },
                Rule { action: "*".into(), allow: false, require_approval: false },
            ],
            budget: Budget::default(),
            retry: None,
            on_device: false,
        }
    }
}

impl Policy {
    /// Load from a YAML file, falling back to the safe default if absent/invalid.
    pub fn load_or_default(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_yaml::from_str(&text).unwrap_or_else(|_| Policy::default()),
            Err(_) => Policy::default(),
        }
    }

    /// The configured per-minute action cap, if any.
    pub fn max_actions_per_minute(&self) -> Option<u32> {
        self.budget.max_actions_per_minute
    }

    /// The configured per-hour inference/API-call cap, if any.
    pub fn max_api_calls_per_hour(&self) -> Option<u32> {
        self.budget.max_api_calls_per_hour
    }

    /// The retry/backoff policy for transient inference failures (R10). Uses the sane default
    /// when no `retry:` section is configured.
    pub fn retry_policy(&self) -> RetryPolicy {
        self.retry.as_ref().map(RetryConfig::to_policy).unwrap_or_default()
    }

    /// Whether the on-device guarantee (R13) is in force for this policy.
    pub fn on_device(&self) -> bool {
        self.on_device
    }

    pub fn check(&self, action_id: &str) -> Decision {
        for rule in &self.rules {
            if matches(&rule.action, action_id) {
                if !rule.allow {
                    return Decision::Deny;
                }
                return if rule.require_approval {
                    Decision::RequiresApproval
                } else {
                    Decision::Allow
                };
            }
        }
        Decision::Deny
    }

    /// Lint the policy and return any concerns. Surfaced as warn-level logs at run
    /// start so developers notice obviously dangerous configurations early. Empty
    /// vec = clean policy.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut delete_named_without_approval = Vec::new();

        for (i, rule) in self.rules.iter().enumerate() {
            // Destructive-named patterns that are allowed without human approval.
            if rule.allow && !rule.require_approval && is_destructive_name(&rule.action) {
                delete_named_without_approval.push(format!(
                    "rule #{}: \"{}\" is allowed without human approval — destructive-sounding actions usually want require_approval: true",
                    i + 1,
                    rule.action
                ));
            }

            // Catch-all wildcard that ALLOWS everything is almost always a mistake.
            if rule.action == "*" && rule.allow && !rule.require_approval {
                out.push(format!(
                    "rule #{}: catch-all \"*\" allows every action without approval — this defeats the deny-by-default model",
                    i + 1
                ));
            }
        }

        out.extend(delete_named_without_approval);

        // No catch-all at all → the host already falls through to Deny, but the
        // explicit `- action: "*"` rule documents intent. Recommend it.
        if !self.rules.iter().any(|r| r.action == "*") {
            out.push(
                "policy has no explicit catch-all \"*\" rule — relying on implicit deny. Add an explicit `- action: \"*\"` with `allow: false` so the intent is obvious.".to_string(),
            );
        }

        if self.budget.max_actions_per_minute.is_none() {
            out.push(
                "no budget.max_actions_per_minute is set — an LLM stuck in a loop can hammer your app indefinitely. Recommend a cap (e.g. 60).".to_string(),
            );
        }

        out
    }
}

fn is_destructive_name(pattern: &str) -> bool {
    let p = pattern.to_lowercase();
    const KEYWORDS: &[&str] = &["delete", "remove", "destroy", "drop", "purge", "wipe"];
    KEYWORDS.iter().any(|k| p.contains(k))
}

fn matches(pattern: &str, action_id: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return action_id.starts_with(prefix);
    }
    pattern == action_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_decisions() {
        let p = Policy::default();
        assert_eq!(p.check("create_note"), Decision::Allow);
        assert_eq!(p.check("edit_note"), Decision::Allow);
        assert_eq!(p.check("delete_note"), Decision::RequiresApproval);
        assert_eq!(p.check("wire_money"), Decision::Deny);
    }

    fn policy_from(yaml: &str) -> Policy {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn warn_on_wildcard_allow() {
        let p = policy_from(
            r#"
rules:
  - action: "*"
    allow: true
budget:
  max_actions_per_minute: 60
"#,
        );
        let warns = p.warnings();
        assert!(warns.iter().any(|w| w.contains("catch-all") && w.contains("defeats")), "got: {warns:?}");
    }

    #[test]
    fn warn_on_destructive_named_action_without_approval() {
        let p = policy_from(
            r#"
rules:
  - action: "delete_note"
    allow: true
    require_approval: false
  - action: "purge_db"
    allow: true
    require_approval: false
  - action: "*"
    allow: false
budget:
  max_actions_per_minute: 60
"#,
        );
        let warns = p.warnings();
        assert_eq!(warns.iter().filter(|w| w.contains("destructive-sounding")).count(), 2);
    }

    #[test]
    fn warn_on_missing_budget_and_missing_wildcard() {
        let p = policy_from(
            r#"
rules:
  - action: "create_*"
    allow: true
"#,
        );
        let warns = p.warnings();
        assert!(warns.iter().any(|w| w.contains("no explicit catch-all")), "got: {warns:?}");
        assert!(warns.iter().any(|w| w.contains("budget.max_actions_per_minute")), "got: {warns:?}");
    }

    #[test]
    fn on_device_flag_parses_and_defaults_off() {
        // Absent → off (backward compatible).
        let off = policy_from(
            r#"
rules:
  - action: "*"
    allow: false
"#,
        );
        assert!(!off.on_device());
        // Explicitly sealed.
        let on = policy_from(
            r#"
rules:
  - action: "*"
    allow: false
on_device: true
"#,
        );
        assert!(on.on_device());
    }

    #[test]
    fn budget_caps_parse_and_default_to_none() {
        // Absent → no caps (backward compatible).
        let none = policy_from(
            r#"
rules:
  - action: "*"
    allow: false
"#,
        );
        assert_eq!(none.max_actions_per_minute(), None);
        assert_eq!(none.max_api_calls_per_hour(), None);
        // Both caps set, independently.
        let set = policy_from(
            r#"
rules:
  - action: "*"
    allow: false
budget:
  max_actions_per_minute: 60
  max_api_calls_per_hour: 500
"#,
        );
        assert_eq!(set.max_actions_per_minute(), Some(60));
        assert_eq!(set.max_api_calls_per_hour(), Some(500));
    }

    #[test]
    fn retry_config_parses_and_defaults_sanely() {
        // Absent → the host's default retry policy (3 retries, 250ms→5s).
        let none = policy_from(
            r#"
rules:
  - action: "*"
    allow: false
"#,
        );
        let def = none.retry_policy();
        assert_eq!(def.max_retries, 3);
        assert_eq!(def.initial_backoff, Duration::from_millis(250));
        assert_eq!(def.max_backoff, Duration::from_secs(5));

        // A partial override leaves the unspecified fields at their defaults.
        let tuned = policy_from(
            r#"
rules:
  - action: "*"
    allow: false
retry:
  max_retries: 5
  initial_backoff_ms: 100
"#,
        );
        let rp = tuned.retry_policy();
        assert_eq!(rp.max_retries, 5);
        assert_eq!(rp.initial_backoff, Duration::from_millis(100));
        assert_eq!(rp.max_backoff, Duration::from_secs(5), "unspecified field keeps the default");

        // Fail-fast: explicit zero retries.
        let off = policy_from(
            r#"
rules:
  - action: "*"
    allow: false
retry:
  max_retries: 0
"#,
        );
        assert_eq!(off.retry_policy().max_retries, 0);
    }

    #[test]
    fn clean_policy_emits_no_warnings() {
        let p = policy_from(
            r#"
rules:
  - action: "create_*"
    allow: true
  - action: "delete_*"
    allow: true
    require_approval: true
  - action: "*"
    allow: false
budget:
  max_actions_per_minute: 60
"#,
        );
        let warns = p.warnings();
        assert!(warns.is_empty(), "expected clean, got: {warns:?}");
    }
}
