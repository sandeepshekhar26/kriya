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
    /// Optional **egress destination tier** (doc 24 §7.3 / EG-2). When present, governed-lane
    /// egress (the gateway/broker HTTP upstreams, the hook's WebFetch lane) is checked against
    /// operator-authored host patterns → `allow | approval | deny`, with per-destination byte
    /// budgets. Absent by default → egress governance OFF, byte-identical to pre-EG-2 behaviour.
    /// This is a governed-lane control, not a host-level network control: a spawned subprocess
    /// bypasses it (see the module doc + TRUST.md).
    #[serde(default)]
    egress: Option<EgressPolicy>,
    /// Optional **retention design** (doc 24 §6-P2): the max-age classes that a retention pruner
    /// honours before sealing the pruned prefix behind a signed epoch-checkpoint receipt. Absent
    /// by default → receipts are retained indefinitely (the pre-EG-2 behaviour).
    #[serde(default)]
    retention: Option<Retention>,
}

impl Default for Policy {
    fn default() -> Self {
        // Allow writes/creates/edits; deletes need approval; everything else denied.
        Policy {
            rules: vec![
                Rule {
                    action: "create_*".into(),
                    allow: true,
                    require_approval: false,
                },
                Rule {
                    action: "edit_*".into(),
                    allow: true,
                    require_approval: false,
                },
                Rule {
                    action: "delete_*".into(),
                    allow: true,
                    require_approval: true,
                },
                Rule {
                    action: "*".into(),
                    allow: false,
                    require_approval: false,
                },
            ],
            budget: Budget::default(),
            retry: None,
            on_device: false,
            egress: None,
            retention: None,
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
        self.retry
            .as_ref()
            .map(RetryConfig::to_policy)
            .unwrap_or_default()
    }

    /// Whether the on-device guarantee (R13) is in force for this policy.
    pub fn on_device(&self) -> bool {
        self.on_device
    }

    /// The egress destination tier (doc 24 §7.3), if the policy configures one. `None` → egress
    /// governance is off for this policy and every governed call proceeds unchecked (the io ledger
    /// is likewise silent), byte-identical to pre-EG-2.
    pub fn egress(&self) -> Option<&EgressPolicy> {
        self.egress.as_ref()
    }

    /// The retention design (doc 24 §6-P2), if configured.
    pub fn retention(&self) -> Option<&Retention> {
        self.retention.as_ref()
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

/// Read-like action-name prefixes the zero-config gateway policy allows outright. Verb-first
/// naming is the MCP convention (`get_account`, `list_transactions`), so a prefix match captures
/// the realistic cases.
pub const READ_PREFIXES: &[&str] = &[
    "get_",
    "list_",
    "read_",
    "fetch_",
    "search_",
    "query_",
    "show_",
    "describe_",
];

/// Destructive / side-effecting action-name prefixes the zero-config gateway policy gates behind
/// human approval. Spend/transfer verbs are here too — a cleared agent can read freely but must be
/// approved before it moves money, sends, or destroys.
pub const DESTRUCTIVE_PREFIXES: &[&str] = &[
    "delete", "remove", "destroy", "drop", "purge", "wipe", "close", "transfer", "send", "pay",
    "archive",
];

/// The zero-config **default deny-by-default policy for the broker** (W2): the same read-allow /
/// destructive-approve / else-deny posture as [`default_proxy_policy`], but the rules are minted
/// **per upstream namespace** — the broker serves tools as `<upstream>__<tool>`, so the flat
/// `get_*` prefixes would never match and would silently deny everything. For each namespace `ns`
/// this emits `ns__get_*`-style allows, `ns__delete*`-style approval gates, then the final
/// catch-all deny. An upstream not in `namespaces` (impossible via the broker, which builds this
/// from its own upstream list) falls to deny — the safe direction.
pub fn default_broker_policy(namespaces: &[String]) -> Policy {
    let mut rules = Vec::new();
    for ns in namespaces {
        for p in READ_PREFIXES {
            rules.push(Rule {
                action: format!("{ns}__{p}*"),
                allow: true,
                require_approval: false,
            });
        }
        for p in DESTRUCTIVE_PREFIXES {
            rules.push(Rule {
                action: format!("{ns}__{p}*"),
                allow: true,
                require_approval: true,
            });
        }
    }
    rules.push(Rule {
        action: "*".into(),
        allow: false,
        require_approval: false,
    });
    Policy {
        rules,
        // One budget spans the whole broker session — all upstreams together, same cap as proxy.
        budget: Budget {
            max_actions_per_minute: Some(60),
            max_api_calls_per_hour: None,
        },
        retry: None,
        on_device: false,
        egress: None,
        retention: None,
    }
}

/// The zero-config **default deny-by-default policy** the `kriya-gateway proxy` uses when no
/// `--policy` file is given (D-016 / service-architecture §7): read-like names allow, destructive /
/// spend names require human approval, everything else is denied — with a sane per-minute budget so
/// a runaway agent is capped by the proxy. Built from the in-crate [`Rule`]/[`Budget`] structs so it
/// reuses the exact [`Policy::check`] matching the in-process host enforces.
///
/// Matching note: rules are tried in order and use the existing `prefix*` glob, not substring — so
/// `delete_transaction` is gated but a downstream that named the same capability `transaction_delete`
/// would fall through to deny (the safe direction). Operators wanting substring rules pass `--policy`.
pub fn default_proxy_policy() -> Policy {
    let mut rules = Vec::new();
    // Reads first (most permissive, but only for read-shaped names).
    for p in READ_PREFIXES {
        rules.push(Rule {
            action: format!("{p}*"),
            allow: true,
            require_approval: false,
        });
    }
    // Destructive / spend names: allowed only after explicit human approval.
    for p in DESTRUCTIVE_PREFIXES {
        rules.push(Rule {
            action: format!("{p}*"),
            allow: true,
            require_approval: true,
        });
    }
    // Everything else: deny by default (defense in depth — `check` also denies on no match).
    rules.push(Rule {
        action: "*".into(),
        allow: false,
        require_approval: false,
    });

    Policy {
        rules,
        // Cap a runaway agent: the proxy IS the handler from the budget's view, so this spans the
        // whole session's downstream calls.
        budget: Budget {
            max_actions_per_minute: Some(60),
            max_api_calls_per_hour: None,
        },
        retry: None,
        on_device: false,
        egress: None,
        retention: None,
    }
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

// ─── Egress destination tier (doc 24 §7.3 / EG-2) ────────────────────────────────────────────────
//
// Operators author **human-readable host patterns** in the policy YAML (decided in doc 24 §7.3 — do
// not revisit):
//
// ```yaml
// egress:
//   unlisted: deny            # deny-by-default (default: allow — the permissive posture §7.3)
//   fail_closed: true         # "no receipt, no egress" (B3); default false
//   rules:
//     - host: "*.vendor.com"
//       tier: allow
//       budget: { window_secs: 60, max_bytes: 1048576 }
//     - host: "api.partner.com"
//       tier: approval
//     - host: "*"
//       tier: deny
// ```
//
// **Landmine L1 (permissions.rs `matches()` is PREFIX-only):** a host wildcard is a *leading* `*.`
// (a suffix match on the domain), so feeding `*.notion.com` to the action matcher would strip the
// trailing char (there is none to strip) and fall through to an exact compare — silently never
// matching. Egress matching therefore uses [`host_matches`], a dedicated suffix matcher; the
// reversed-host encoding named in doc 24 §7.3/L1 is one valid way to reuse `matches()`, a direct
// suffix matcher is another — either way the L1 silent-fail case is proven impossible by the test
// matrix below. Operators never see or write reversed hosts.

/// One egress destination rule as authored in the policy YAML.
#[derive(Debug, Clone, Deserialize)]
struct EgressRule {
    /// A human-readable host pattern: `*` (any), `*.vendor.com` (the vendor.com domain — its
    /// subdomains and the apex), or an exact host `api.vendor.com`.
    host: String,
    /// What to do with a call to this destination. Default `allow` (listing a host without a tier
    /// means "allow it").
    #[serde(default = "default_tier")]
    tier: EgressTier,
    /// Optional per-destination byte budget (B2 — anti slow-drip exfil). Observed *payload* bytes
    /// (L2), never wire/TLS bytes.
    #[serde(default)]
    budget: Option<ByteBudget>,
}

fn default_tier() -> EgressTier {
    EgressTier::Allow
}

/// The three egress tiers — the same three the action policy already has, applied by destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EgressTier {
    Allow,
    Approval,
    Deny,
}

/// A per-destination byte budget: no more than `max_bytes` of observed outbound payload in any
/// trailing `window_secs` window. Exceeding it denies the call that would breach — a signed
/// `kriya.io.*.deny` receipt, not a silent drop (B2).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ByteBudget {
    pub window_secs: u64,
    pub max_bytes: u64,
}

/// What happens to a host no rule matches. `deny` is deny-by-default (the safe allowlist posture,
/// which also arms the broker's startup allowlist check); `allow` is the permissive default §7.3
/// documents as a "documented deviation" printed in every export; `defer` parks the unlisted call
/// at the approval gate instead of hard-denying it (B4 defer semantics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnlistedPosture {
    #[default]
    Allow,
    Deny,
    Defer,
}

/// The compiled egress tier for a policy. Deserialized from the policy YAML's `egress:` section.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EgressPolicy {
    #[serde(default)]
    rules: Vec<EgressRule>,
    /// The posture for a host no rule matches. Default `allow` (§7.3 — the tier ships OFF/permissive
    /// and every export prints the mode); set `deny` for deny-by-default.
    #[serde(default)]
    unlisted: UnlistedPosture,
    /// Fail-closed receipt-precondition mode (B3): if the `kriya.io.*` receipt cannot be written,
    /// the egress is DENIED. Default `false` (fail-open — the honest documented default).
    #[serde(default)]
    fail_closed: bool,
    /// Whether to record **ingress** digests (a keyed hash + size of tool responses / inbound
    /// content). Its OWN switch, **default OFF even when egress is ON** (doc 24 §6-P3): computing a
    /// hash reads every content byte, which is a processing activity in its own right, and an
    /// unsalted hash of guessable content is content disclosure — so ingress hashing is keyed
    /// (HMAC) and off unless the operator opts in.
    #[serde(default)]
    record_ingress: bool,
}

/// The outcome of evaluating one destination against the egress tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressDecision {
    /// Proceed. `rule` is the operator-authored pattern that matched (for the `policy_rule` param),
    /// `None` for the permissive-unlisted case.
    Allow { rule: Option<String> },
    /// Route through the approval gate (an `approval`-tier host, or `defer` on an unlisted host).
    Approval { rule: Option<String> },
    /// Block at the decision point. `reason` is human-readable for the receipt + the agent.
    Deny {
        rule: Option<String>,
        reason: String,
    },
}

impl EgressPolicy {
    /// Decide what to do with a call to `host` (already lowercased/trimmed is fine — the matcher
    /// normalizes). First matching rule wins; an unmatched host falls to the `unlisted` posture.
    pub fn evaluate(&self, host: &str) -> EgressDecision {
        for r in &self.rules {
            if host_matches(&r.host, host) {
                return match r.tier {
                    EgressTier::Allow => EgressDecision::Allow {
                        rule: Some(r.host.clone()),
                    },
                    EgressTier::Approval => EgressDecision::Approval {
                        rule: Some(r.host.clone()),
                    },
                    EgressTier::Deny => EgressDecision::Deny {
                        rule: Some(r.host.clone()),
                        reason: format!("egress to {host} denied by rule '{}'", r.host),
                    },
                };
            }
        }
        match self.unlisted {
            UnlistedPosture::Allow => EgressDecision::Allow { rule: None },
            UnlistedPosture::Deny => EgressDecision::Deny {
                rule: None,
                reason: format!("egress to {host} is not on the allowlist (deny-by-default)"),
            },
            UnlistedPosture::Defer => EgressDecision::Approval { rule: None },
        }
    }

    /// The byte budget in force for `host`, plus the pattern that carries it (the budget-counter
    /// key). The first matching rule that declares a `budget:` wins.
    pub fn budget_for(&self, host: &str) -> Option<(String, ByteBudget)> {
        self.rules
            .iter()
            .find(|r| host_matches(&r.host, host) && r.budget.is_some())
            .map(|r| (r.host.clone(), r.budget.expect("is_some checked")))
    }

    /// Whether fail-closed receipt-precondition mode (B3) is on.
    pub fn fail_closed(&self) -> bool {
        self.fail_closed
    }

    /// Whether ingress digest recording is on (its own switch, default OFF — doc 24 §6-P3).
    pub fn record_ingress(&self) -> bool {
        self.record_ingress
    }

    /// A short, export-safe label for the posture — printed in every egress-bearing export so an
    /// assessor's first question ("was the ledger permissive during the window?") is answered
    /// before it is asked (§6-H10).
    pub fn mode_label(&self) -> &'static str {
        match self.unlisted {
            UnlistedPosture::Allow => "allow-unlisted",
            UnlistedPosture::Deny => "deny-by-default",
            UnlistedPosture::Defer => "defer-unlisted",
        }
    }

    /// Whether the tier is deny-by-default (arms the broker startup allowlist check).
    pub fn is_deny_by_default(&self) -> bool {
        self.unlisted == UnlistedPosture::Deny
    }
}

/// Match an operator-authored **host pattern** against a concrete host — the internal compile
/// detail that maps human-readable patterns onto matching without the L1 prefix-only trap.
///
/// - `*` → any host
/// - `*.vendor.com` → the vendor.com domain: any subdomain (`a.vendor.com`, `a.b.vendor.com`) and the apex (`vendor.com`)
/// - `api.vendor.com` → exact match only
///
/// Case-insensitive; leading/trailing whitespace ignored. Unlike the action [`matches`] (prefix
/// glob only), a leading `*.` is a genuine suffix match — the exact case L1 warns silently fails
/// under the action matcher.
fn host_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    let host = host.trim().to_ascii_lowercase();
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        // The vendor.com domain: subdomains AND the apex. An empty suffix ("*.") is malformed → no
        // match rather than matching everything.
        return !suffix.is_empty() && (host == suffix || host.ends_with(&format!(".{suffix}")));
    }
    pattern == host
}

/// Extract the host from an upstream/tool URL without pulling in a URL-parsing dependency. Shared
/// by the HTTP transport (the captured `dest_host`), the broker's egress resolver, and the hook's
/// WebFetch lane, so the ledger and the allowlist agree on the destination string. Lives here (an
/// always-compiled module) so the hook — built without `mcp-client` — can reach it too.
/// `https://user@api.vendor.com:443/mcp` → `api.vendor.com`.
pub fn url_host(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    // Strip the port. IPv6 literals (`[::1]:port`) keep their brackets' contents; the common
    // hostname/IPv4 case splits on the first colon.
    if let Some(stripped) = host_port.strip_prefix('[') {
        return stripped.split(']').next().unwrap_or(stripped).to_string();
    }
    host_port
        .split(':')
        .next()
        .unwrap_or(host_port)
        .to_ascii_lowercase()
}

/// The **retention design** (doc 24 §6-P2): the max-age classes a retention pruner honours before
/// sealing the pruned prefix behind a signed epoch-checkpoint receipt (see
/// [`crate::audit::RETENTION_CHECKPOINT`]). `kriya.io.*` receipts get a **shorter** default class
/// than policy/approval receipts — I/O metadata is the most privacy-sensitive and least
/// evidence-durable class (§4.5). Both fields optional; absent → that class is retained
/// indefinitely.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Retention {
    /// Max age in days for `kriya.io.*` receipts. Shorter than [`Self::default_days`] by design.
    #[serde(default)]
    pub io_days: Option<u32>,
    /// Max age in days for policy/approval/action receipts.
    #[serde(default)]
    pub default_days: Option<u32>,
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
        assert!(
            warns
                .iter()
                .any(|w| w.contains("catch-all") && w.contains("defeats")),
            "got: {warns:?}"
        );
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
        assert_eq!(
            warns
                .iter()
                .filter(|w| w.contains("destructive-sounding"))
                .count(),
            2
        );
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
        assert!(
            warns.iter().any(|w| w.contains("no explicit catch-all")),
            "got: {warns:?}"
        );
        assert!(
            warns
                .iter()
                .any(|w| w.contains("budget.max_actions_per_minute")),
            "got: {warns:?}"
        );
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
        assert_eq!(
            rp.max_backoff,
            Duration::from_secs(5),
            "unspecified field keeps the default"
        );

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
    fn default_proxy_policy_reads_allow_destructive_approve_else_deny() {
        let p = default_proxy_policy();
        // Read-like names allow outright.
        assert_eq!(p.check("get_account"), Decision::Allow);
        assert_eq!(p.check("list_transactions"), Decision::Allow);
        assert_eq!(p.check("search_notes"), Decision::Allow);
        // Destructive / spend names require human approval.
        assert_eq!(p.check("delete_transaction"), Decision::RequiresApproval);
        assert_eq!(p.check("transfer_funds"), Decision::RequiresApproval);
        assert_eq!(p.check("send_payment"), Decision::RequiresApproval);
        assert_eq!(p.check("archive_account"), Decision::RequiresApproval);
        // Anything unrecognized is denied (deny-by-default).
        assert_eq!(p.check("frobnicate"), Decision::Deny);
        assert_eq!(
            p.check("create_note"),
            Decision::Deny,
            "even writes deny unless read-shaped"
        );
        // A budget cap is in force so a runaway agent is bounded.
        assert_eq!(p.max_actions_per_minute(), Some(60));
    }

    #[test]
    fn default_broker_policy_gates_per_upstream_namespace() {
        let p = default_broker_policy(&["github".into(), "linear".into()]);
        // The same read/destructive posture as the proxy default, per namespace.
        assert_eq!(p.check("github__get_issue"), Decision::Allow);
        assert_eq!(p.check("linear__list_projects"), Decision::Allow);
        assert_eq!(p.check("github__delete_repo"), Decision::RequiresApproval);
        assert_eq!(p.check("linear__send_invite"), Decision::RequiresApproval);
        // Non-read, non-destructive names deny — same safe fall-through as the proxy default.
        assert_eq!(p.check("github__create_issue"), Decision::Deny);
        // A namespace the broker didn't declare denies outright.
        assert_eq!(p.check("ghost__get_anything"), Decision::Deny);
        // The flat (un-namespaced) name never matches a broker rule.
        assert_eq!(p.check("get_issue"), Decision::Deny);
        assert_eq!(p.max_actions_per_minute(), Some(60));
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

    // ─── Egress tier (doc 24 §7.3 / EG-2) ────────────────────────────────────────────────────────

    /// **Landmine L1, the mandatory matrix.** A leading-wildcard host pattern (`*.notion.com`) must
    /// match subdomains and the apex — the exact case that silently fails when fed to the PREFIX-only
    /// action matcher. This test both proves `host_matches` is correct AND documents *why* egress
    /// could not simply reuse `matches()`.
    #[test]
    fn l1_host_matcher_handles_leading_wildcards_that_prefix_matching_silently_drops() {
        // The landmine, made explicit: the action matcher strips a TRAILING `*`; a host wildcard is
        // LEADING, so `matches` finds no trailing `*`, falls to exact compare, and never matches.
        assert!(
            !matches("*.notion.com", "api.notion.com"),
            "documents L1: the action prefix matcher silently never matches a leading-wildcard host"
        );

        // The correct suffix matcher: subdomains AND the apex match; look-alikes do NOT.
        assert!(host_matches("*.notion.com", "api.notion.com"));
        assert!(host_matches("*.notion.com", "a.b.notion.com"));
        assert!(host_matches("*.notion.com", "notion.com"), "apex matches");
        assert!(
            !host_matches("*.notion.com", "evilnotion.com"),
            "a look-alike registrable domain must NOT match"
        );
        assert!(
            !host_matches("*.notion.com", "notion.com.evil.com"),
            "a suffix-injection host must NOT match"
        );

        // `*` matches anything; exact matches exactly; matching is case-insensitive.
        assert!(host_matches("*", "anything.example"));
        assert!(host_matches("api.vendor.com", "API.Vendor.COM"));
        assert!(!host_matches("api.vendor.com", "www.vendor.com"));
        // A malformed bare `*.` matches nothing (never an accidental match-all).
        assert!(!host_matches("*.", "vendor.com"));
    }

    fn egress_from(yaml: &str) -> EgressPolicy {
        policy_from(yaml)
            .egress()
            .expect("egress section present")
            .clone()
    }

    #[test]
    fn egress_tiers_allow_approval_deny_by_destination() {
        let e = egress_from(
            r#"
rules:
  - action: "*"
    allow: true
egress:
  unlisted: deny
  rules:
    - host: "*.vendor.com"
      tier: allow
    - host: "api.partner.com"
      tier: approval
    - host: "blocked.example"
      tier: deny
"#,
        );
        assert_eq!(
            e.evaluate("api.vendor.com"),
            EgressDecision::Allow {
                rule: Some("*.vendor.com".into())
            }
        );
        assert_eq!(
            e.evaluate("api.partner.com"),
            EgressDecision::Approval {
                rule: Some("api.partner.com".into())
            }
        );
        assert!(matches!(
            e.evaluate("blocked.example"),
            EgressDecision::Deny { .. }
        ));
        // Unlisted under deny-by-default → Deny (arms the broker startup allowlist check).
        assert!(matches!(
            e.evaluate("random.host"),
            EgressDecision::Deny { .. }
        ));
        assert!(e.is_deny_by_default());
        assert_eq!(e.mode_label(), "deny-by-default");
    }

    #[test]
    fn egress_unlisted_posture_defaults_to_allow_and_supports_defer() {
        // No `unlisted:` → permissive (§7.3: the tier ships OFF/allow, mode printed in the export).
        let permissive = egress_from(
            r#"
rules: [{action: "*", allow: true}]
egress:
  rules:
    - host: "blocked.example"
      tier: deny
"#,
        );
        assert_eq!(
            permissive.evaluate("anything.else"),
            EgressDecision::Allow { rule: None }
        );
        assert_eq!(permissive.mode_label(), "allow-unlisted");

        // `defer` parks an unlisted call at the approval gate instead of hard-denying (B4).
        let defer = egress_from(
            r#"
rules: [{action: "*", allow: true}]
egress:
  unlisted: defer
"#,
        );
        assert_eq!(
            defer.evaluate("new.host"),
            EgressDecision::Approval { rule: None }
        );
        assert_eq!(defer.mode_label(), "defer-unlisted");
    }

    #[test]
    fn egress_byte_budget_is_looked_up_by_matching_pattern() {
        let e = egress_from(
            r#"
rules: [{action: "*", allow: true}]
egress:
  rules:
    - host: "*.vendor.com"
      tier: allow
      budget: { window_secs: 60, max_bytes: 1048576 }
    - host: "nobudget.example"
      tier: allow
"#,
        );
        let (pattern, budget) = e.budget_for("api.vendor.com").expect("budget matched");
        assert_eq!(pattern, "*.vendor.com");
        assert_eq!(budget.window_secs, 60);
        assert_eq!(budget.max_bytes, 1_048_576);
        assert!(e.budget_for("nobudget.example").is_none());
        assert!(e.budget_for("unmatched.host").is_none());
    }

    #[test]
    fn egress_fail_closed_flag_parses_and_defaults_off() {
        let off = egress_from(
            r#"
rules: [{action: "*", allow: true}]
egress:
  rules: []
"#,
        );
        assert!(!off.fail_closed(), "fail-open is the documented default");
        // Ingress recording is its own switch, default OFF even when egress is configured (§6-P3).
        assert!(!off.record_ingress(), "ingress digests are default OFF");
        let on = egress_from(
            r#"
rules: [{action: "*", allow: true}]
egress:
  fail_closed: true
  record_ingress: true
"#,
        );
        assert!(on.fail_closed());
        assert!(on.record_ingress());
    }

    #[test]
    fn egress_absent_by_default_is_backward_compatible() {
        // A policy with no egress section → no egress governance, unchanged behaviour.
        let p = policy_from(r#"rules: [{action: "*", allow: false}]"#);
        assert!(p.egress().is_none());
    }

    #[test]
    fn retention_parses_with_shorter_io_class() {
        // Absent → indefinite retention (pre-EG-2).
        let none = policy_from(r#"rules: [{action: "*", allow: false}]"#);
        assert!(none.retention().is_none());
        // io class is shorter than the default class by design (doc 24 §4.5 / §6-P2).
        let p = policy_from(
            r#"
rules: [{action: "*", allow: false}]
retention:
  io_days: 30
  default_days: 365
"#,
        );
        let r = p.retention().expect("retention configured");
        assert_eq!(r.io_days, Some(30));
        assert_eq!(r.default_days, Some(365));
        assert!(r.io_days.unwrap() < r.default_days.unwrap());
    }
}
