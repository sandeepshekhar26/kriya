//! Minimal but real permission layer. Every agent action is checked against a policy
//! before the host asks the app to run it. Default is deny-unknown; deletes require
//! human approval (no approval queue exists yet in Phase 0, so they are held/denied).

use serde::Deserialize;
use serde_json::Value;

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
    /// Optional **detection pack** (doc 24 §11 B5–B12 / EG-P): DNS-exfil heuristics, the SSRF/
    /// rebinding guard, secret+PII scanning, operation rails, canary tokens, the connector
    /// registry, read-only presets, and MCP response trust classes — each independently absent by
    /// default, so opting into the pack as a whole never silently enables a specific detector.
    #[serde(default)]
    detection: Option<DetectionPolicy>,
    /// Optional **credential brokering** (doc 24 §11 B13 / EG-B): alias → OS Keychain reference
    /// mappings, each scoped to its own destination allowlist. Absent by default → no `{{kriya:*}}`
    /// placeholder is ever substituted, byte-identical to pre-EG-B behaviour. A NEW trust posture
    /// when present — kriya briefly holds a real secret in process memory to inject it — see
    /// `crate::secrets`'s module doc and `docs/THREAT-MODEL-brokering.md`.
    #[serde(default)]
    secrets: Option<crate::secrets::SecretsPolicy>,
    /// Optional **A2A (agent-to-agent) governance seam** (doc 24 §11 B18 / EG-F). ROADMAP-DEPTH: no
    /// agent-to-agent transport exists in this codebase yet, so nothing calls
    /// [`Policy::evaluate_a2a_target`] today — this only defines the policy shape + decision
    /// function a future A2A broker lane will call once real inter-agent RPC exists, so that lane
    /// reuses the SAME allowlist engine as the egress tier instead of inventing a second allowlist
    /// DSL. Absent by default → byte-identical to pre-EG-F behaviour.
    #[serde(default)]
    a2a: Option<A2aPolicy>,
    /// Optional **model-identity gate** (F1, doc 28 §F1): `kriya-llm-proxy`'s policy dimension —
    /// approved local-inference model digests per tier + the default action for an unrecognized
    /// model. Absent by default → `kriya-llm-proxy` still runs with [`ModelPolicy::default`]
    /// (`unknown_model: warn`), so an agent-policy.yaml that never authored `model:` behaves
    /// identically whether the key is present-but-empty or absent (same BC discipline `egress`/
    /// `detection`/`secrets` already established).
    #[serde(default)]
    model: Option<ModelPolicy>,
    /// Optional **endpoint budget gate** (C2, doc 27 §4 / `docs/design/c2-budget-gate.md`): USD
    /// spend budgets (session / rolling-day / user scope) the hook `pre` lane enforces
    /// pre-execution against C1's trailing spend state. Absent by default → byte-identical to
    /// pre-C2 behaviour (the same BC discipline `egress`/`detection`/`secrets`/`model` already
    /// established).
    #[serde(default)]
    budgets: Option<BudgetPolicy>,
    /// Optional **deterministic-execution lane routing** (F4-wasm, doc 28 §F4): when a tool has a
    /// registered WASM variant AND `prefer_deterministic_lane` is on, governed calls to it route
    /// through `kriya-run-wasm`'s engine instead of the tool's normal executor — see
    /// [`ExecPolicy`]. Absent by default → byte-identical to pre-F4-wasm behaviour (the same BC
    /// discipline `egress`/`detection`/`secrets`/`model`/`budgets` already established).
    #[serde(default)]
    exec: Option<ExecPolicy>,
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
            detection: None,
            secrets: None,
            a2a: None,
            model: None,
            budgets: None,
            exec: None,
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

    /// The detection pack (doc 24 §11 B5–B12 / EG-P), if configured.
    pub fn detection(&self) -> Option<&DetectionPolicy> {
        self.detection.as_ref()
    }

    /// The credential-brokering policy (doc 24 §11 B13 / EG-B), if configured.
    pub fn secrets(&self) -> Option<&crate::secrets::SecretsPolicy> {
        self.secrets.as_ref()
    }

    /// The A2A governance seam (doc 24 §11 B18 / EG-F), if configured. See [`A2aPolicy`]'s doc
    /// comment for why this is a seam, not a wired enforcement point, today.
    pub fn a2a(&self) -> Option<&A2aPolicy> {
        self.a2a.as_ref()
    }

    /// Decide what to do with an RPC call to `target_agent_id` under the A2A seam — `None` when
    /// `a2a:` isn't configured (no opinion, matches every other detector's "absent = off" idiom).
    /// Convenience wrapper so a future call site doesn't need to unwrap `a2a()` itself.
    pub fn evaluate_a2a_target(&self, target_agent_id: &str) -> Option<EgressDecision> {
        self.a2a.as_ref().map(|a| a.evaluate(target_agent_id))
    }

    /// The model-identity gate (F1, doc 28 §F1), if configured. `None` → `kriya-llm-proxy` runs
    /// with [`ModelPolicy::default`] (`unknown_model: warn`) rather than any stricter posture.
    pub fn model(&self) -> Option<&ModelPolicy> {
        self.model.as_ref()
    }

    /// The endpoint budget gate (C2, doc 27 §4), if configured. `None` → the hook `pre` lane
    /// performs no budget consult at all, byte-identical to pre-C2 behaviour.
    pub fn budgets(&self) -> Option<&BudgetPolicy> {
        self.budgets.as_ref()
    }

    /// The deterministic-execution lane routing policy (F4-wasm, doc 28 §F4), if configured.
    /// `None` → no action is ever routed to the WASM lane, byte-identical to pre-F4-wasm.
    pub fn exec(&self) -> Option<&ExecPolicy> {
        self.exec.as_ref()
    }

    /// The WASI-p2 component path registered for `action_id`'s deterministic-execution variant,
    /// IF `prefer_deterministic_lane` is on AND a variant is registered — `None` in every other
    /// case (no `exec:` section, the lane is off, or this action has no registered variant), which
    /// is exactly "route to it when a WASM variant is registered; receipted either way" (doc 28
    /// §F4 build spec item 4): whichever path a caller takes, the existing action receipt (or, on
    /// the WASM lane, the `kriya.exec.deterministic` receipt alongside it) still gets emitted —
    /// this method only decides WHICH executor runs, never whether a receipt is signed.
    pub fn resolve_wasm_variant(&self, action_id: &str) -> Option<&str> {
        let exec = self.exec.as_ref()?;
        if !exec.prefer_deterministic_lane {
            return None;
        }
        exec.wasm_variants.get(action_id).map(String::as_str)
    }

    pub fn check(&self, action_id: &str) -> Decision {
        // B11 (doc 24 §11): a read-only-preset connector's known-mutating tools are denied
        // BEFORE the explicit rules are even consulted — a hard override the operator's own
        // (possibly broad) allow rules can never widen back open. This is the "rides the existing
        // per-action tier" preset: it denies exactly where an explicit rule would, just pre-empted.
        if self
            .detection
            .as_ref()
            .is_some_and(|d| d.read_only_denies(action_id))
        {
            return Decision::Deny;
        }
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
        detection: None,
        secrets: None,
        a2a: None,
        model: None,
        budgets: None,
        exec: None,
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
        detection: None,
        secrets: None,
        a2a: None,
        model: None,
        budgets: None,
        exec: None,
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

// ─── A2A (agent-to-agent) governance seam (doc 24 §11 B18 / EG-F) ─────────────────────────────────
//
// ROADMAP-DEPTH: thin until real agent-to-agent traffic exists. There is no A2A transport in this
// codebase today (the broker in `kriya-gateway` aggregates MCP upstreams, not other agents; the
// `agent/` module drives one app, not a peer agent) — so nothing calls `A2aPolicy::evaluate` yet.
// This exists so that WHEN an A2A lane is built, it reuses the exact allowlist/tier engine
// `EgressPolicy` already has (rules -> tier, unlisted posture) instead of inventing a second
// allowlist DSL, and so it emits the SAME `kriya.io.*` receipt vocabulary (no new action_id shape)
// once it exists — "apply the same allowlist/receipt path" per doc 24 §11's build item.

/// The A2A destination allowlist, keyed by target agent id instead of network host. `#[serde(flatten)]`
/// so `a2a:` in the policy YAML has the IDENTICAL shape as `egress:` (`rules: [{host, tier, budget?}]`,
/// `unlisted`, `fail_closed`, `record_ingress`) — `host` is read as an agent-id pattern here, reusing
/// `EgressPolicy`'s exact matcher (`*`, `*.suffix`, exact) rather than a bespoke agent-id grammar.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct A2aPolicy {
    #[serde(flatten)]
    allowlist: EgressPolicy,
}

impl A2aPolicy {
    /// Decide what to do with an RPC call to `target_agent_id` — identical decision shape to
    /// [`EgressPolicy::evaluate`], just evaluated over an agent id instead of a host.
    pub fn evaluate(&self, target_agent_id: &str) -> EgressDecision {
        self.allowlist.evaluate(target_agent_id)
    }
}

// ─── Model-identity gate (F1, doc 28 §F1 / FRONTIER-EXECUTE-PROMPT.md §2 · §3 F1) ─────────────────
//
// `kriya-llm-proxy`'s policy dimension: gate a served completion on the RESOLVED digest of the
// model that served it (never the model's self-reported name alone — a name is not an identity).
// Deliberately NOT `EgressPolicy` reused wholesale, because the default posture differs on purpose:
// egress is deny-by-default-capable: an operator-authored destination allowlist; this is
// **observation-first** (doc 24 §11.5 — "enforcement verbs only where enforcement is real"): an
// unrecognized LOCAL model never blocks by default, it only WARNS — via an explicit, signed
// `kriya.model.gate` warn receipt, not silence — until the operator opts into `require-approval` or
// `deny`. The per-digest `approved` allowlist DOES reuse [`EgressTier`] (the same allow/approval/deny
// space an operator already knows from `egress:`), so authoring a specific digest's tier is the
// familiar idiom; only the *default-for-everything-else* posture (`unknown_model`) is a new type.

/// One approved model-identity rule: a resolved digest → tier. Mirrors an [`EgressRule`]'s shape
/// (`host` → `digest`), reusing [`EgressTier`]'s allow/approval/deny space.
#[derive(Debug, Clone, Deserialize)]
pub struct ApprovedModelRule {
    /// The resolved model digest (sha256 hex, no `sha256:` prefix) this rule matches exactly.
    pub digest: String,
    #[serde(default = "default_model_tier")]
    pub tier: EgressTier,
    /// Cosmetic-only human label (e.g. `"llama3.1:8b (verified 2026-07-24)"`) — never read by
    /// [`ModelPolicy::evaluate`], carried only for the Console's display.
    #[serde(default)]
    pub label: Option<String>,
}

fn default_model_tier() -> EgressTier {
    EgressTier::Allow
}

/// What happens to a served completion whose model digest matches no [`ApprovedModelRule`] —
/// covering BOTH a resolved-but-unlisted digest and a digest the `llm-proxy` feature's manifest
/// resolver couldn't resolve at all. Default `warn`: the fail-OPEN posture the F1 build spec
/// requires — an unrecognized model is disclosed, never silently blocked, unless the operator
/// explicitly opts into stricter enforcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnknownModelAction {
    #[default]
    Warn,
    RequireApproval,
    Deny,
}

/// The compiled model-identity policy for `kriya-llm-proxy`. Deserialized from the SAME
/// `agent-policy.yaml` every other governed binary loads, under an optional `model:` key —
/// `#[serde(default)]`'d on [`Policy`] so an agent-policy.yaml that never authored one round-trips
/// unchanged (the same BC discipline `egress`/`detection`/`secrets` already established).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelPolicy {
    #[serde(default)]
    pub approved: Vec<ApprovedModelRule>,
    #[serde(default)]
    pub unknown_model: UnknownModelAction,
}

/// The outcome of gating one served completion by model identity. Distinct from [`EgressDecision`]
/// because an unmatched digest WARNS by default rather than denies — see the module note above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelGateDecision {
    /// Proceed, no further comment. `rule` is the approved digest that matched (`None` only when
    /// [`ModelPolicy::evaluate`] is called directly with an empty policy — practically unreachable
    /// since an empty policy's `unknown_model` default is `Warn`, never `Allow`).
    Allow { rule: Option<String> },
    /// Proceed, but disclose: the observation-first default for an unrecognized model.
    Warn { reason: String },
    /// No synchronous approval channel exists in the v1 proxy (same honest limitation as
    /// `mcp::contain`'s CONNECT tunnel) — refused, never silently allowed nor hung.
    Approval { reason: String },
    /// Block. `rule` is the specific approved-list digest that matched a `deny` tier (`None` for
    /// the `unknown_model: deny` default path).
    Deny {
        rule: Option<String>,
        reason: String,
    },
}

impl ModelGateDecision {
    /// The `decision` facet carried into the `kriya.model.gate` receipt — one of the four closed
    /// values (never a fifth, same discipline as [`IoDecision::facet`](crate::mcp::executor::IoDecision)).
    pub fn facet(&self) -> &'static str {
        match self {
            ModelGateDecision::Allow { .. } => "allow",
            ModelGateDecision::Warn { .. } => "warn",
            ModelGateDecision::Approval { .. } => "approval_required",
            ModelGateDecision::Deny { .. } => "deny",
        }
    }

    /// The approved-list digest that matched, if any (for the receipt's `matched_rule`).
    pub fn matched_rule(&self) -> Option<&str> {
        match self {
            ModelGateDecision::Allow { rule } | ModelGateDecision::Deny { rule, .. } => {
                rule.as_deref()
            }
            ModelGateDecision::Warn { .. } | ModelGateDecision::Approval { .. } => None,
        }
    }

    /// The human-readable reason (for the receipt's `reason` and the proxy's error body on refusal).
    pub fn reason(&self) -> String {
        match self {
            ModelGateDecision::Allow { rule: Some(r) } => {
                format!("model digest {r} is on the approved allowlist")
            }
            ModelGateDecision::Allow { rule: None } => "no model policy configured".to_string(),
            ModelGateDecision::Warn { reason }
            | ModelGateDecision::Approval { reason }
            | ModelGateDecision::Deny { reason, .. } => reason.clone(),
        }
    }

    /// Whether this decision blocks the request. Only `Approval` (no sync approval channel in v1 —
    /// refused honestly rather than hung or silently allowed) and `Deny` block; `Allow`/`Warn` never
    /// do — the fail-OPEN default the F1 spec requires.
    pub fn blocks(&self) -> bool {
        matches!(
            self,
            ModelGateDecision::Approval { .. } | ModelGateDecision::Deny { .. }
        )
    }
}

impl ModelPolicy {
    /// Decide what to do with a completion served by the model whose resolved digest is `digest`
    /// (`None` when [`crate::llm::manifest`] couldn't resolve one — treated exactly like a
    /// resolved-but-unlisted digest: the `unknown_model` default applies).
    pub fn evaluate(&self, digest: Option<&str>) -> ModelGateDecision {
        if let Some(d) = digest {
            if let Some(rule) = self.approved.iter().find(|r| r.digest == d) {
                return match rule.tier {
                    EgressTier::Allow => ModelGateDecision::Allow {
                        rule: Some(rule.digest.clone()),
                    },
                    EgressTier::Approval => ModelGateDecision::Approval {
                        reason: format!("model digest {d} is approval-tier by policy"),
                    },
                    EgressTier::Deny => ModelGateDecision::Deny {
                        rule: Some(rule.digest.clone()),
                        reason: format!("model digest {d} is denied by policy"),
                    },
                };
            }
        }
        match self.unknown_model {
            UnknownModelAction::Warn => ModelGateDecision::Warn {
                reason: "model digest is not on the approved allowlist (observation-only default \
                         — set unknown_model: require-approval or deny to enforce)"
                    .to_string(),
            },
            UnknownModelAction::RequireApproval => ModelGateDecision::Approval {
                reason: "unapproved model requires approval by policy".to_string(),
            },
            UnknownModelAction::Deny => ModelGateDecision::Deny {
                rule: None,
                reason: "unapproved model denied by policy".to_string(),
            },
        }
    }
}

// ─── F4-wasm (doc 28 §F4): deterministic-execution lane routing ──────────────────────────────
//
// Deliberately the SIMPLEST policy shape in this file: no tiers, no allow/approval/deny space —
// this is a ROUTING decision (which executor runs a cleared action), not a NEW gate. The action
// still goes through `Policy::check` exactly as before; a registered WASM variant only changes
// HOW an already-allowed action executes, and either path signs a receipt (the normal action
// receipt, or that PLUS `kriya.exec.deterministic` on the WASM lane) — see
// `Policy::resolve_wasm_variant`.

/// The deterministic-execution lane routing policy (F4-wasm). Deserialized from the SAME
/// `agent-policy.yaml` every other governed binary loads, under an optional `exec:` key.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExecPolicy {
    /// The tier switch (doc 28 §F4 build spec item 4: "a tier option 'prefer deterministic
    /// lane'"). `false` (default) → `wasm_variants` is inert even if populated, so authoring the
    /// registry ahead of flipping this on is safe (no surprise routing change from an
    /// otherwise-unrelated edit).
    #[serde(default)]
    pub prefer_deterministic_lane: bool,
    /// `action_id` → path to a WASI-p2 component implementing the SAME action, built for
    /// `kriya-run-wasm`. An action with no entry here never routes to the WASM lane regardless of
    /// `prefer_deterministic_lane`.
    #[serde(default)]
    pub wasm_variants: std::collections::BTreeMap<String, String>,
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
pub(crate) fn host_matches(pattern: &str, host: &str) -> bool {
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

// ─── Detection pack (doc 24 §11 B5–B12 / EG-P) ───────────────────────────────────────────────────
//
// Every sub-detector is independently `Option`-gated and absent by default, so opting a policy into
// `detection:` at all never silently turns on a specific check — each one is a deliberate, separate
// operator choice (never auto-block silently by default). All detectors run on GOVERNED LANES only
// (the same seams the egress tier already gates); a spawned subprocess bypasses them exactly as it
// bypasses the egress tier itself — this is not a host boundary. Detection findings are additive
// receipt params on the SAME `kriya.io.*` vocabulary (never a new action_id shape): an "alert" is a
// call that still executes but whose io receipt carries an extra flag field; a "deny" is a real
// decision-point block with `decision: "deny"` and a `reason` naming the detector, mirroring the
// egress tier's own L10 discipline.

fn default_true() -> bool {
    true
}

/// What a detector does on a match: proceed but flag it (never blocks a legitimate call on its
/// own), or block outright. Default `Alert` — the house rule for every heuristic that can
/// false-positive (doc 24 §11's "never auto-block silently by default").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertOrDeny {
    #[default]
    Alert,
    Deny,
}

/// What a content-match detector (secret/PII) does: keep the call fidelity intact but strip the
/// matched value from what's hashed/recorded (default — safe, non-breaking), or block outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RedactOrDeny {
    #[default]
    Redact,
    Deny,
}

/// The detection pack. Every field is independently optional; only configured detectors run.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DetectionPolicy {
    /// B5: DNS-exfil / anomalous-destination / subdomain-entropy heuristic.
    #[serde(default)]
    pub dns_exfil: Option<DnsExfilPolicy>,
    /// B6: SSRF / private-IP / cloud-metadata / DNS-rebinding guard.
    #[serde(default)]
    pub ssrf_guard: Option<SsrfGuardPolicy>,
    /// B7: secret + PII scan/redact on outbound governed bodies.
    #[serde(default)]
    pub secret_pii: Option<SecretPiiPolicy>,
    /// B8: operation rails — allow/deny specific outbound API operations, not just hosts.
    #[serde(default)]
    pub operation_rails: Vec<OperationRail>,
    /// B9: canary tokens — an exact-match string whose appearance in an outbound body is always an
    /// immediate deny, never policy-tunable (that is the entire point of a canary).
    #[serde(default)]
    pub canary_tokens: Vec<String>,
    /// B10: the connector registry — new/drifted MCP tools default disabled-until-approved.
    #[serde(default)]
    pub connector_registry: Option<ConnectorRegistryPolicy>,
    /// B11: per-connector/per-tool read-only presets — host/namespace patterns (egress
    /// [`host_matches`] syntax) whose known-mutating tools are denied.
    #[serde(default)]
    pub read_only: Vec<String>,
    /// B12: MCP response enforcement — per-server trust classes on governed-lane ingress.
    #[serde(default)]
    pub mcp_response: Option<McpResponsePolicy>,
}

/// B5: flag destinations whose leftmost subdomain label has unusually high character entropy (the
/// classic DNS-exfiltration shape: stolen data base32/hex/base64-encoded into a subdomain of an
/// otherwise-allowed domain). This is a HEURISTIC on top of the egress tier's own allow/deny — it
/// exists to catch abuse of an *already-allowed* wildcard domain, not to replace the allowlist.
#[derive(Debug, Clone, Deserialize)]
pub struct DnsExfilPolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Shannon-entropy threshold in bits/char above which a subdomain label is flagged. Ordinary
    /// hostnames score roughly 2.5–3.5; base32/hex-encoded exfil payloads commonly score 3.8+.
    /// Default chosen conservatively high to minimize false positives on legitimate CDN/hash-named
    /// subdomains (see the false-positive-safety test).
    #[serde(default = "default_entropy_threshold")]
    pub entropy_threshold: f64,
    #[serde(default)]
    pub action: AlertOrDeny,
}

fn default_entropy_threshold() -> f64 {
    4.0
}

/// Shannon entropy in bits/character of `s`, over its raw bytes. A hostname label is ASCII (or
/// punycode-encoded at the wire level for IDN, which is itself a high-entropy string and correctly
/// flagged), so byte-level entropy is the right granularity. Strings under 2 bytes score 0.0 — too
/// short for a meaningful character distribution either way.
pub fn shannon_entropy(s: &str) -> f64 {
    if s.len() < 2 {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<u8, u32> = std::collections::HashMap::new();
    for b in s.bytes() {
        *counts.entry(b).or_insert(0) += 1;
    }
    let len = s.len() as f64;
    -counts
        .values()
        .map(|&c| {
            let p = c as f64 / len;
            p * p.log2()
        })
        .sum::<f64>()
}

/// The highest per-label Shannon entropy among `host`'s SUBDOMAIN labels — every label except the
/// trailing two (assumed to be the registrable apex domain + TLD, e.g. `vendor`/`com`), since that
/// pair is the operator-allowlisted destination itself, not an attacker-controlled position.
/// Exfiltration tools commonly chunk encoded data across multiple labels (DNS's 63-byte label
/// limit), so this checks ALL of them and returns the max, not just the leftmost. `None` for a host
/// with fewer than 3 labels — an apex or single-label host has no subdomain to inspect at all.
pub fn max_subdomain_entropy(host: &str) -> Option<f64> {
    let labels: Vec<&str> = host.trim().trim_end_matches('.').split('.').collect();
    if labels.len() < 3 {
        return None;
    }
    labels[..labels.len() - 2]
        .iter()
        .map(|l| shannon_entropy(l))
        .fold(None, |acc: Option<f64>, e| {
            Some(acc.map_or(e, |a| a.max(e)))
        })
}

/// B6: reject private/link-local/cloud-metadata destinations and pin the resolved IP for the
/// connection so a rebind between the check and the connect can't swap in a different address. This
/// is a real security control, not a tunable heuristic — the only dial is whether it's on.
///
/// That one dial governs BOTH layers together: the GOVERNOR-level pre-check (a forbidden destination
/// gets a clean, policy-attributed pre-execute `kriya.io.*.deny` receipt) AND the HTTP transport's IP
/// pin (`mcp::client::HttpTransport`, the actually TOCTOU-proof enforcement). Gated, not
/// unconditional: a local dev/test upstream on `127.0.0.1`/`localhost` is a legitimate `url:`
/// target — this is real, not hypothetical, an existing broker integration test connects to one — so
/// pinning away from loopback by default would have real legitimate-traffic cost, unlike (say)
/// refusing the cloud metadata endpoint. Same house rule as every other detector in the pack (doc 24
/// §11's "never auto-block silently by default"): absent `detection.ssrf_guard`, both layers are off.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct SsrfGuardPolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// B6 core validator: why `ip` is a forbidden SSRF/rebinding target, or `None` if it's an ordinary
/// routable address. Covers loopback, RFC1918 private ranges, link-local (which subsumes the cloud
/// metadata endpoint `169.254.169.254` — it lives inside `169.254.0.0/16`), unspecified, broadcast,
/// IPv6 unique-local (`fc00::/7`) and link-local (`fe80::/10`), and IPv4-mapped IPv6 addresses
/// (checked against the same IPv4 rules after unwrapping). Used both by the governor's pre-check
/// (clean receipts) and the transport's resolver pin (actual enforcement) so the two layers can
/// never disagree about what's forbidden.
pub fn ssrf_disallowed_reason(ip: std::net::IpAddr) -> Option<&'static str> {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                Some("loopback (127.0.0.0/8)")
            } else if v4.is_private() {
                Some("RFC1918 private range")
            } else if v4.is_link_local() {
                Some("link-local (169.254.0.0/16, includes the cloud metadata endpoint)")
            } else if v4.is_unspecified() {
                Some("unspecified (0.0.0.0)")
            } else if v4.is_broadcast() {
                Some("broadcast (255.255.255.255)")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ssrf_disallowed_reason(IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            if v6.is_loopback() {
                Some("loopback (::1)")
            } else if v6.is_unspecified() {
                Some("unspecified (::)")
            } else if seg0 & 0xfe00 == 0xfc00 {
                Some("unique-local (fc00::/7)")
            } else if seg0 & 0xffc0 == 0xfe80 {
                Some("link-local (fe80::/10)")
            } else {
                None
            }
        }
    }
}

/// B7: scan outbound governed bodies for a closed set of secret/PII shapes. On a match, either
/// redact (record the match TYPE + a content hash only, never the value) or deny.
#[derive(Debug, Clone, Deserialize)]
pub struct SecretPiiPolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub action: RedactOrDeny,
}

/// B7 scan: which secret/PII shapes matched `payload` (the canonical-JSON outbound body), by TYPE
/// name only. Byte-level scanning throughout (never a `&str` slice at an arbitrary offset) so this
/// can never panic on attacker-controlled UTF-8 — every helper below only ever compares/indexes
/// `&[u8]`. The matched substring itself is never extracted or returned: a caller has structurally
/// nothing to leak into a flag or a "redact" — the value never leaves this function (doc 24 L9's
/// hash-only rule, applied here as *no value at all*, not even hashed).
pub fn scan_secrets_pii(payload: &str) -> Vec<&'static str> {
    let b = payload.as_bytes();
    let mut hits = Vec::new();
    if has_fixed_prefix_token(b, b"AKIA", 16, is_upper_alnum) {
        hits.push("aws_access_key");
    }
    if has_fixed_prefix_token(b, b"ghp_", 36, is_alnum) {
        hits.push("github_pat");
    }
    if has_jwt(b) {
        hits.push("jwt");
    }
    if contains(b, b"-----BEGIN") && contains(b, b"PRIVATE KEY") {
        hits.push("private_key");
    }
    if has_email(b) {
        hits.push("email");
    }
    if has_luhn_card(b) {
        hits.push("credit_card");
    }
    if has_ssn(b) {
        hits.push("ssn");
    }
    hits
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

fn is_upper_alnum(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

fn is_alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// True if `prefix` appears anywhere in `b` followed immediately by exactly `tail_len` bytes all
/// satisfying `tail_ok` (e.g. an AWS key: `AKIA` + 16 upper-alnum chars).
fn has_fixed_prefix_token(
    b: &[u8],
    prefix: &[u8],
    tail_len: usize,
    tail_ok: fn(u8) -> bool,
) -> bool {
    let plen = prefix.len();
    if b.len() < plen + tail_len {
        return false;
    }
    (0..=b.len() - plen - tail_len).any(|i| {
        &b[i..i + plen] == prefix
            && b[i + plen..i + plen + tail_len]
                .iter()
                .copied()
                .all(tail_ok)
    })
}

fn is_b64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// A JWT: three dot-separated base64url segments, the first starting `eyJ` (base64 for `{"`) — a
/// strong anchor that keeps this from firing on an ordinary dotted token or version string. Minimum
/// segment lengths are conservative (a real header/payload/signature are all comfortably longer).
fn has_jwt(b: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 <= b.len() {
        if &b[i..i + 3] == b"eyJ" {
            let seg1_end = walk_while(b, i, is_b64url);
            if seg1_end >= i + 10 && b.get(seg1_end) == Some(&b'.') {
                let seg2_start = seg1_end + 1;
                let seg2_end = walk_while(b, seg2_start, is_b64url);
                if seg2_end >= seg2_start + 10 && b.get(seg2_end) == Some(&b'.') {
                    let seg3_start = seg2_end + 1;
                    let seg3_end = walk_while(b, seg3_start, is_b64url);
                    if seg3_end >= seg3_start + 5 {
                        return true;
                    }
                }
            }
        }
        i += 1;
    }
    false
}

fn walk_while(b: &[u8], mut i: usize, pred: fn(u8) -> bool) -> usize {
    while i < b.len() && pred(b[i]) {
        i += 1;
    }
    i
}

/// A conservative `local@domain.tld` shape: at least one local-part char, a domain of word chars and
/// dots, a final label of 2+ letters.
fn has_email(b: &[u8]) -> bool {
    for (idx, &c) in b.iter().enumerate() {
        if c != b'@' {
            continue;
        }
        let local_ok = idx > 0
            && matches!(b[idx - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'+' | b'-' | b'_');
        if !local_ok {
            continue;
        }
        let dom_start = idx + 1;
        let dom_end = walk_while(b, dom_start, |c| {
            c.is_ascii_alphanumeric() || c == b'.' || c == b'-'
        });
        let domain = &b[dom_start..dom_end];
        if domain.len() < 4 {
            continue;
        }
        if let Some(dot) = domain.iter().rposition(|&c| c == b'.') {
            let tld = &domain[dot + 1..];
            if tld.len() >= 2 && tld.iter().all(u8::is_ascii_alphabetic) {
                return true;
            }
        }
    }
    false
}

/// A 13–19 digit run that passes the Luhn checksum (the standard card-number DLP heuristic — Luhn's
/// own ~1-in-10 accept rate on random digits is an inherent property of the checksum, not a flaw
/// here; it is what every mainstream DLP tool uses for exactly this shape).
fn has_luhn_card(b: &[u8]) -> bool {
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            let end = walk_while(b, i, |c| c.is_ascii_digit());
            let len = end - start;
            if (13..=19).contains(&len) && luhn_valid(&b[start..end]) {
                return true;
            }
            i = end;
        } else {
            i += 1;
        }
    }
    false
}

fn luhn_valid(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut v = (d - b'0') as u32;
        if double {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
        double = !double;
    }
    sum % 10 == 0
}

/// `\d{3}-\d{2}-\d{4}` not immediately bordered by another digit (so it doesn't fire inside a longer
/// dash-delimited digit run, e.g. a tracking or account number).
fn has_ssn(b: &[u8]) -> bool {
    if b.len() < 11 {
        return false;
    }
    (0..=b.len() - 11).any(|i| {
        b[i..i + 3].iter().all(u8::is_ascii_digit)
            && b[i + 3] == b'-'
            && b[i + 4..i + 6].iter().all(u8::is_ascii_digit)
            && b[i + 6] == b'-'
            && b[i + 7..i + 11].iter().all(u8::is_ascii_digit)
            && (i == 0 || !b[i - 1].is_ascii_digit())
            && (i + 11 >= b.len() || !b[i + 11].is_ascii_digit())
    })
}

/// B9: the first configured canary token that appears verbatim in `payload`, if any. Canary tokens
/// are operator-planted honeytoken strings (bait credentials that should never legitimately appear
/// in real traffic) — ANY match is always-deny regardless of `AlertOrDeny`/`RedactOrDeny` (doc 24
/// §11 B9 is the one detector with no soft mode: there is no legitimate reason for a canary to ever
/// cross a governed lane, so there is nothing an "alert" mode would be hedging against).
pub fn canary_match<'a>(payload: &str, tokens: &'a [String]) -> Option<&'a str> {
    tokens
        .iter()
        .find(|t| !t.is_empty() && payload.contains(t.as_str()))
        .map(String::as_str)
}

/// B8: one operation rail — allow/deny/approve a specific outbound API operation, narrower than a
/// host-level egress rule. `host` uses the same pattern syntax as egress rules (`*` / `*.domain` /
/// exact); `method` is an HTTP verb or `*`; `path` is an optional `prefix_*` glob or exact match;
/// `graphql_mutation` optionally matches a GraphQL mutation NAME inside a JSON body. Rails are
/// evaluated top-to-bottom, first match wins; a body the rail must parse to decide (a `path`/
/// `graphql_mutation` rail against a non-JSON or malformed body) that fails to parse is a DENY
/// (fail-closed for the rail — an uninspectable body can't be cleared).
#[derive(Debug, Clone, Deserialize)]
pub struct OperationRail {
    #[serde(default = "default_star")]
    pub host: String,
    #[serde(default = "default_star")]
    pub method: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub graphql_mutation: Option<String>,
    pub tier: EgressTier,
}

fn default_star() -> String {
    "*".to_string()
}

/// B8 evaluation outcome for one call against the configured `operation_rails`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RailOutcome {
    /// No configured rail's `host` pattern matches this destination — rails are opt-in per
    /// destination, so an unrailed host is completely unaffected (false-positive safety).
    NoRailApplies,
    Allowed,
    RequiresApproval,
    Denied(String),
    /// A rail applies to this destination but the operation (verb+path or GraphQL mutation name)
    /// could not be extracted from `params` — fail-closed (doc 24 §11 B8).
    ParseFailed,
}

/// Best-effort `(METHOD, path)` from an action's params: an explicit `method`/`path` pair, or a
/// `method` + the path component of a `url` field (defaulting method to `GET`, matching a plain
/// WebFetch's implicit-GET shape).
fn extract_operation(params: &Value) -> Option<(String, String)> {
    let method = params
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET")
        .to_ascii_uppercase();
    if let Some(path) = params.get("path").and_then(Value::as_str) {
        return Some((method, path.to_string()));
    }
    let url = params.get("url").and_then(Value::as_str)?;
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let path = after_scheme
        .find('/')
        .map(|i| &after_scheme[i..])
        .unwrap_or("/");
    Some((method, path.to_string()))
}

/// A GraphQL mutation NAME out of a `query`/`body` string field: text after the first `mutation`
/// keyword, up to the first non-identifier character. `None` for an anonymous mutation (`mutation {
/// ... }`) or no mutation keyword at all — anonymous mutations can't be named-matched by a rail.
fn extract_graphql_mutation(params: &Value) -> Option<String> {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .or_else(|| params.get("body").and_then(Value::as_str))?;
    let idx = query.find("mutation")?;
    let rest = query[idx + "mutation".len()..].trim_start();
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// B8: evaluate `params` against the operation rails that apply to `host`. Rails are an ALLOWLIST
/// FENCE (the "rail" in the name) — an operation that doesn't match any applicable rail is denied,
/// not silently passed through, consistent with the rest of kriya's deny-by-default posture. Rails
/// are evaluated top-to-bottom; first match wins.
pub fn evaluate_operation_rails(
    rails: &[OperationRail],
    host: &str,
    params: &Value,
) -> RailOutcome {
    let applicable: Vec<&OperationRail> = rails
        .iter()
        .filter(|r| r.host == "*" || host_matches(&r.host, host))
        .collect();
    if applicable.is_empty() {
        return RailOutcome::NoRailApplies;
    }

    let op = extract_operation(params);
    let mutation = extract_graphql_mutation(params);
    if op.is_none() && mutation.is_none() {
        return RailOutcome::ParseFailed;
    }

    for rail in applicable {
        let matched = if let Some(want) = &rail.graphql_mutation {
            mutation.as_deref() == Some(want.as_str())
        } else if let Some((method, path)) = &op {
            let method_ok = rail.method == "*" || rail.method.eq_ignore_ascii_case(method);
            let path_ok = rail
                .path
                .as_deref()
                .map_or(true, |pattern| matches(pattern, path));
            method_ok && path_ok
        } else {
            false
        };
        if matched {
            return match rail.tier {
                EgressTier::Allow => RailOutcome::Allowed,
                EgressTier::Approval => RailOutcome::RequiresApproval,
                EgressTier::Deny => RailOutcome::Denied(format!(
                    "operation rail explicitly denies {} on '{host}' (B8)",
                    op.as_ref()
                        .map(|(m, p)| format!("{m} {p}"))
                        .unwrap_or_else(|| format!(
                            "mutation {}",
                            mutation.as_deref().unwrap_or("?")
                        )),
                )),
            };
        }
    }
    RailOutcome::Denied(format!(
        "no operation rail on '{host}' permits {} (B8, fail-closed)",
        op.as_ref()
            .map(|(m, p)| format!("{m} {p}"))
            .unwrap_or_else(|| format!("mutation {}", mutation.as_deref().unwrap_or("?"))),
    ))
}

/// B10: the connector registry. A discovered MCP tool `(upstream, tool)` is disabled-until-approved
/// unless it appears here with a matching `description_hash`; a hash MISMATCH against an approved
/// entry (the tool's description/schema changed since approval) is drift — the tool-poisoning
/// signal — and disables it again until re-approved. Approval is authored in policy (via the
/// Console), never a runtime-mutable file, so it travels with the signed fleet PolicyBundle exactly
/// like every other policy dial.
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectorRegistryPolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub approved: Vec<ApprovedConnectorTool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovedConnectorTool {
    /// The broker upstream namespace (the slugified `name:` from `broker.yaml`, e.g. `"widgets"`).
    pub upstream: String,
    /// The inner (un-namespaced) tool name as the upstream reports it.
    pub tool: String,
    /// SHA-256 hex of the canonical tool description at approval time — see
    /// `connector_tool_hash` in `bin/kriya-gateway.rs`, the only place a full `Tool` (with its
    /// description) is in hand at discovery time. A live mismatch is drift.
    pub description_hash: String,
}

/// B12: per-server trust class for governed MCP ingress (responses). `Trusted` passes through
/// unchanged; `Scan` runs the B7 secret/PII pass over the response too; `Block` denies the response
/// outright. Default class is `Scan`, never `Block` — the house rule against silently auto-blocking
/// a server the operator hasn't explicitly classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustClass {
    Trusted,
    #[default]
    Scan,
    Block,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpResponsePolicy {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The class an unlisted server gets. Default `Scan` (never `Block`).
    #[serde(default)]
    pub default_class: TrustClass,
    #[serde(default)]
    pub per_server: std::collections::BTreeMap<String, TrustClass>,
}

impl McpResponsePolicy {
    /// The effective trust class for `server` (the broker upstream namespace).
    pub fn class_for(&self, server: &str) -> TrustClass {
        self.per_server
            .get(server)
            .copied()
            .unwrap_or(self.default_class)
    }
}

impl DetectionPolicy {
    /// B11: whether `action_id` is a known-mutating tool on a connector the operator marked
    /// read-only — a hard override the explicit action `rules` can never widen back open (checked
    /// before them in [`Policy::check`]). `read_only` entries are connector NAMESPACE patterns using
    /// the action policy's own prefix-glob syntax (`"widgets"`/`"widgets__*"`/`"widgets__delete_*"`
    /// — a bare namespace like `"widgets"` is normalized to `"widgets__*"`), never a host: the
    /// namespace is what a "connector" means in the broker's `<namespace>__<tool>` scheme, and
    /// resolving a namespace to a destination host isn't information `Policy` has.
    pub fn read_only_denies(&self, action_id: &str) -> bool {
        if !is_destructive_name(action_id) {
            return false;
        }
        self.read_only.iter().any(|pattern| {
            let pattern = if pattern.contains('*') {
                pattern.clone()
            } else {
                format!("{pattern}__*")
            };
            matches(&pattern, action_id)
        })
    }
}

// ─── Endpoint budget gate (C2, doc 27 §4 / `docs/design/c2-budget-gate.md`) ────────────────────────
//
// A "trailing-state budget gate" — NEVER a hard cap (Red-team a / the naming-check test): it blocks
// or routes-to-approval the NEXT gated action once C1's Console-written spend state (read-only,
// [`crate::spend_state::SpendState`]) crosses an operator-authored USD threshold. This crate never
// prices anything — every `observed_usd` figure here was already priced by the Console and copied
// through a state file.
//
// Composition (D3, the tier-surprise guard, axiom 4): on the `kriya-hook pre` lane the action tier
// (`Policy::check`) is the ONLY pre-execution tier that precedes this gate. A budget can escalate an
// Allowed action to approval/deny; it can NEVER loosen an earlier Deny/ungranted-approval back to
// allow — [`Policy::check`]'s own early returns already guarantee that (this gate is only ever
// consulted once `check` resolved `Allow` or a GRANTED `RequiresApproval`).

/// Which observed-spend key a budget rule is scoped against (D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetScope {
    /// The current governed session (`session_id`/`run_id` from the hook payload).
    Session,
    /// The current UTC day's total across every session (F-MB: each session's whole priced total
    /// attributes to the UTC day of its `window_end_ms` — a coarse, honest, never-split boundary;
    /// see `docs/TRUST.md`).
    RollingDay,
    /// The OS user's cross-session total (this device's single local operator).
    User,
}

impl BudgetScope {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetScope::Session => "session",
            BudgetScope::RollingDay => "rolling-day",
            BudgetScope::User => "user",
        }
    }
}

/// What a breaching budget rule does (D3). Strictest-wins across rules: `deny` > `require-approval`
/// > `warn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetBreachAction {
    Deny,
    RequireApproval,
    Warn,
}

impl BudgetBreachAction {
    fn kind(self) -> BreachKind {
        match self {
            BudgetBreachAction::Deny => BreachKind::Deny,
            BudgetBreachAction::RequireApproval => BreachKind::Approval,
            BudgetBreachAction::Warn => BreachKind::Warn,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetBreachAction::Deny => "deny",
            BudgetBreachAction::RequireApproval => "require-approval",
            BudgetBreachAction::Warn => "warn",
        }
    }
}

/// What happens when the state needed to evaluate a rule is missing or stale (D2). Each rule's
/// EFFECTIVE posture, when this is omitted, defaults per-tier from [`BudgetBreachAction`] — see
/// [`default_on_missing`] — so every tier's fail-closed-by-default posture is exactly its own tier's
/// natural behavior; an operator opting a `deny` rule INTO `fail-open` is an explicit, receipted
/// loosening (never the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnMissingState {
    FailClosed,
    FailOpen,
    RequireApproval,
}

impl OnMissingState {
    fn kind(self) -> BreachKind {
        match self {
            OnMissingState::FailClosed => BreachKind::Deny,
            OnMissingState::RequireApproval => BreachKind::Approval,
            OnMissingState::FailOpen => BreachKind::Warn,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            OnMissingState::FailClosed => "fail-closed",
            OnMissingState::FailOpen => "fail-open",
            OnMissingState::RequireApproval => "require-approval",
        }
    }
}

/// The per-tier default `on_missing_state` posture (D2's table): `deny`'s default IS fail-closed,
/// `require-approval`'s default IS route-to-approval, `warn`'s default IS fail-open — i.e. every
/// tier's own natural behavior is ALSO its missing-state default. This is what makes B0's fail-closed
/// matrix an EXTENSION rather than a new, separate posture to reason about.
fn default_on_missing(action: BudgetBreachAction) -> OnMissingState {
    match action {
        BudgetBreachAction::Deny => OnMissingState::FailClosed,
        BudgetBreachAction::RequireApproval => OnMissingState::RequireApproval,
        BudgetBreachAction::Warn => OnMissingState::FailOpen,
    }
}

/// One authored budget rule (D3 YAML shape).
#[derive(Debug, Clone, Deserialize)]
pub struct BudgetRule {
    /// A stable id, surfaced on the receipt + the Console's UI.
    pub id: String,
    pub scope: BudgetScope,
    pub threshold_usd: f64,
    pub action: BudgetBreachAction,
    /// Overrides the per-tier missing/stale-state default (D2). `None` = use [`default_on_missing`].
    #[serde(default)]
    pub on_missing_state: Option<OnMissingState>,
}

fn default_max_staleness_secs() -> u64 {
    900
}

/// The budget gate's policy section (D3). Additive nullable — mirrors `egress`/`detection`/
/// `secrets`/`model`'s `Option<...>` BC discipline exactly.
#[derive(Debug, Clone, Deserialize)]
pub struct BudgetPolicy {
    /// State older than this (seconds) is "stale" — treated exactly like missing state (D2).
    #[serde(default = "default_max_staleness_secs")]
    pub max_staleness_secs: u64,
    #[serde(default)]
    pub rules: Vec<BudgetRule>,
}

impl Default for BudgetPolicy {
    fn default() -> Self {
        BudgetPolicy { max_staleness_secs: default_max_staleness_secs(), rules: Vec::new() }
    }
}

/// The evaluation context a budget check needs beyond the spend state itself — the scope keys
/// [`BudgetScope::Session`]/[`BudgetScope::User`] resolve against.
#[derive(Debug, Clone, Copy)]
pub struct BudgetCtx<'a> {
    pub session_id: Option<&'a str>,
    pub os_user: &'a str,
}

/// The three receipt-shaped breach kinds — `deny` > `approval` > `warn` (strictest-wins, D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreachKind {
    Deny,
    Approval,
    Warn,
}
impl BreachKind {
    fn rank(self) -> u8 {
        match self {
            BreachKind::Deny => 2,
            BreachKind::Approval => 1,
            BreachKind::Warn => 0,
        }
    }
}

/// The full, privacy-audited param set for a `kriya.spend.gate.*` receipt (D4) — count/cost/hash/
/// id/timestamp only, NEVER transcript content. `scope_key` is always a hash or a non-sensitive
/// label, never a raw session id / OS username string.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetGateRecord {
    pub budget_id: String,
    pub scope: BudgetScope,
    pub scope_key: String,
    pub threshold_usd: f64,
    pub observed_usd: f64,
    pub action: BudgetBreachAction,
    pub state_source: &'static str, // "live-tick" | "statusline" | "none"
    pub state_as_of_ms: u64,
    pub state_stale: bool,
    /// Set only when missing/stale state (rather than a fresh over-threshold reading) produced this
    /// record (D2) — `None` on a native over-threshold breach.
    pub on_missing_state: Option<OnMissingState>,
    pub pricing_sheet: Option<String>,
    pub pricing_sheet_hash: Option<String>,
}

/// The gate's decision for one `kriya-hook pre` call, over EVERY matching rule (strictest wins).
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetGateDecision {
    /// No rule breached at all (every rule is either absent, or resolved fresh-and-under-threshold).
    /// The action proceeds with no gate receipt — the common case, byte-identical to no `budgets:`
    /// section at all.
    Pass,
    /// A `warn`-shaped breach (fresh-and-over-threshold `warn` rule, or an explicit/default
    /// fail-open posture on missing/stale state) — observe-only, never blocks.
    Warn(BudgetGateRecord),
    /// A `require-approval`-shaped breach — the caller must route through the SAME approval gate the
    /// action tier uses; granted -> proceed, not granted -> block (D4).
    Approval(BudgetGateRecord),
    /// A `deny`-shaped breach — the caller must block immediately (exit 2 on the hook lane).
    Deny(BudgetGateRecord),
}

fn utc_day_string(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = y + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

impl BudgetPolicy {
    /// Evaluate EVERY matching rule against `state`, returning the single strictest-wins decision
    /// (D3). Pure — no filesystem/signing here; the caller (the hook's `pre` lane) decides what to
    /// DO with the decision (sign a receipt, exit 2, route through the approval gate).
    pub fn check(
        &self,
        state: &crate::spend_state::SpendState,
        ctx: &BudgetCtx,
        now_ms: u64,
    ) -> BudgetGateDecision {
        let mut winner: Option<(BreachKind, BudgetGateRecord)> = None;

        for rule in &self.rules {
            let reading = state.resolve(rule.scope, ctx.session_id);
            let (kind, record) = match reading {
                None => {
                    // Missing state — includes an unresolvable scope key (e.g. no session_id for a
                    // session-scope rule), never silently skipped (Red-team b).
                    // Missing state, on a `warn`-tier rule with no explicit override, still emits a
                    // receipted warn (D2: "the chosen on_missing_state is written into the gate
                    // receipt so the posture is auditable") — never a silent no-op.
                    let posture = rule.on_missing_state.unwrap_or_else(|| default_on_missing(rule.action));
                    let kind = posture.kind();
                    let record = BudgetGateRecord {
                        budget_id: rule.id.clone(),
                        scope: rule.scope,
                        scope_key: scope_key_for(rule.scope, ctx),
                        threshold_usd: rule.threshold_usd,
                        observed_usd: 0.0,
                        action: rule.action,
                        state_source: "none",
                        state_as_of_ms: 0,
                        state_stale: true,
                        on_missing_state: Some(posture),
                        pricing_sheet: None,
                        pricing_sheet_hash: None,
                    };
                    (kind, record)
                }
                Some(r) => {
                    let age_secs = now_ms.saturating_sub(r.as_of_ms) / 1000;
                    let stale = age_secs > self.max_staleness_secs;
                    if stale {
                        let posture =
                            rule.on_missing_state.unwrap_or_else(|| default_on_missing(rule.action));
                        let record = BudgetGateRecord {
                            budget_id: rule.id.clone(),
                            scope: rule.scope,
                            scope_key: scope_key_for(rule.scope, ctx),
                            threshold_usd: rule.threshold_usd,
                            observed_usd: r.observed_usd,
                            action: rule.action,
                            state_source: r.source.as_str(),
                            state_as_of_ms: r.as_of_ms,
                            state_stale: true,
                            on_missing_state: Some(posture),
                            pricing_sheet: r.pricing_sheet.clone(),
                            pricing_sheet_hash: r.pricing_sheet_hash.clone(),
                        };
                        (posture.kind(), record)
                    } else if r.observed_usd >= rule.threshold_usd {
                        let record = BudgetGateRecord {
                            budget_id: rule.id.clone(),
                            scope: rule.scope,
                            scope_key: scope_key_for(rule.scope, ctx),
                            threshold_usd: rule.threshold_usd,
                            observed_usd: r.observed_usd,
                            action: rule.action,
                            state_source: r.source.as_str(),
                            state_as_of_ms: r.as_of_ms,
                            state_stale: false,
                            on_missing_state: None,
                            pricing_sheet: r.pricing_sheet.clone(),
                            pricing_sheet_hash: r.pricing_sheet_hash.clone(),
                        };
                        (rule.action.kind(), record)
                    } else {
                        continue; // fresh + under threshold — this rule does not fire
                    }
                }
            };

            let better = match &winner {
                None => true,
                Some((wk, _)) => kind.rank() > wk.rank(),
            };
            if better {
                winner = Some((kind, record));
            }
        }

        match winner {
            None => BudgetGateDecision::Pass,
            Some((BreachKind::Deny, r)) => BudgetGateDecision::Deny(r),
            Some((BreachKind::Approval, r)) => BudgetGateDecision::Approval(r),
            Some((BreachKind::Warn, r)) => BudgetGateDecision::Warn(r),
        }
    }
}

/// Lowercase-hex SHA-256 — a private copy of `audit.rs`'s helper (not itself `pub`) so `scope_key`
/// never carries a raw session id / OS username, only a commitment to it (D4).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// The receipt's `scope_key` (D4): never a raw session id / OS username — always a hash (session,
/// user) or a non-sensitive label (rolling-day: a UTC date string, not sensitive on its own).
fn scope_key_for(scope: BudgetScope, ctx: &BudgetCtx) -> String {
    match scope {
        BudgetScope::Session => match ctx.session_id {
            Some(sid) => sha256_hex(sid.as_bytes()),
            None => "unresolved-session".to_string(),
        },
        BudgetScope::User => sha256_hex(ctx.os_user.as_bytes()),
        BudgetScope::RollingDay => utc_day_string(now_for_day_label()),
    }
}

/// The wall-clock "now", used ONLY to LABEL the rolling-day scope_key with today's UTC date — never
/// to compute `observed_usd` (that number always comes straight from the state reading).
fn now_for_day_label() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spend_state::SpendState;

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

    // ─── A2A seam (doc 24 §11 B18 / EG-F) ──────────────────────────────────────────────────────────

    #[test]
    fn a2a_absent_by_default_is_backward_compatible() {
        let p = policy_from(r#"rules: [{action: "*", allow: false}]"#);
        assert!(p.a2a().is_none());
        assert!(p.evaluate_a2a_target("any-agent").is_none());
    }

    #[test]
    fn a2a_reuses_the_egress_allowlist_engine_keyed_by_agent_id() {
        let p = policy_from(
            r#"
rules: [{action: "*", allow: true}]
a2a:
  unlisted: deny
  rules:
    - host: "*.trusted-fleet"
      tier: allow
    - host: "quarantined-agent"
      tier: deny
"#,
        );
        assert_eq!(
            p.evaluate_a2a_target("worker.trusted-fleet"),
            Some(EgressDecision::Allow {
                rule: Some("*.trusted-fleet".into())
            })
        );
        assert!(matches!(
            p.evaluate_a2a_target("quarantined-agent"),
            Some(EgressDecision::Deny { .. })
        ));
        // Unlisted under deny-by-default → Deny, same posture semantics as the egress tier.
        assert!(matches!(
            p.evaluate_a2a_target("never-seen-agent"),
            Some(EgressDecision::Deny { .. })
        ));
    }

    /// The EG-F acceptance proof: a YAML shaped EXACTLY like kriya-console's
    /// `control_plane::policy::policy_yaml_from_bundle` produces (top-level `rules`/`egress`/
    /// `detection`, plus `budgets` merged in under a `budget:` key) round-trips through this crate's
    /// own `Policy` and actually enforces — "a bundle with an egress policy converges on a device
    /// and enforces" (doc 24 §11 / EG-F).
    #[test]
    fn a_policy_bundle_shaped_yaml_converges_and_enforces() {
        let p = policy_from(
            r#"
rules:
  - action: "*"
    allow: true
egress:
  unlisted: deny
  rules:
    - host: "*.allowed.example.com"
      tier: allow
    - host: "evil.example.com"
      tier: deny
detection:
  dns_exfil:
    enabled: true
budget:
  max_actions_per_minute: 42
"#,
        );

        let egress = p.egress().expect("the bundle's egress section converged");
        assert_eq!(
            egress.evaluate("api.allowed.example.com"),
            EgressDecision::Allow {
                rule: Some("*.allowed.example.com".into())
            }
        );
        assert!(matches!(
            egress.evaluate("evil.example.com"),
            EgressDecision::Deny { .. }
        ));
        assert!(matches!(egress.evaluate("never-listed.example"), EgressDecision::Deny { .. }));

        let detection = p.detection().expect("the bundle's detection section converged");
        assert!(detection.dns_exfil.as_ref().expect("dns_exfil configured").enabled);

        assert_eq!(p.max_actions_per_minute(), Some(42), "budgets merged under budget: converged");
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

    // ─── Detection pack (doc 24 §11 B5–B12 / EG-P) ──────────────────────────────────────────────

    #[test]
    fn detection_absent_by_default_and_every_sub_detector_independently_gated() {
        let p = policy_from(r#"rules: [{action: "*", allow: true}]"#);
        assert!(
            p.detection().is_none(),
            "opting into nothing changes nothing"
        );

        // Opting into `detection:` at all must NOT silently enable any specific sub-detector.
        let p = policy_from(
            r#"
rules: [{action: "*", allow: true}]
detection: {}
"#,
        );
        let d = p.detection().expect("detection section present");
        assert!(d.dns_exfil.is_none());
        assert!(d.ssrf_guard.is_none());
        assert!(d.secret_pii.is_none());
        assert!(d.operation_rails.is_empty());
        assert!(d.canary_tokens.is_empty());
        assert!(d.connector_registry.is_none());
        assert!(d.read_only.is_empty());
        assert!(d.mcp_response.is_none());
    }

    #[test]
    fn detection_sub_policies_default_to_the_safe_never_auto_block_choice() {
        let p = policy_from(
            r#"
rules: [{action: "*", allow: true}]
detection:
  dns_exfil: {}
  secret_pii: {}
  mcp_response: {}
"#,
        );
        let d = p.detection().unwrap();
        assert_eq!(
            d.dns_exfil.as_ref().unwrap().action,
            AlertOrDeny::Alert,
            "default alert, never deny"
        );
        assert_eq!(
            d.secret_pii.as_ref().unwrap().action,
            RedactOrDeny::Redact,
            "default redact, never deny"
        );
        assert_eq!(
            d.mcp_response.as_ref().unwrap().default_class,
            TrustClass::Scan,
            "default scan, never block"
        );
        assert_eq!(d.dns_exfil.as_ref().unwrap().entropy_threshold, 4.0);
    }

    #[test]
    fn b11_read_only_denies_only_destructive_names_on_a_marked_connector() {
        let p = policy_from(
            r#"
rules:
  - action: "*"
    allow: true
detection:
  read_only: ["widgets"]
"#,
        );
        // Observe: a non-destructive tool on the read-only connector is unaffected.
        assert_eq!(p.check("widgets__list_items"), Decision::Allow);
        assert_eq!(p.check("widgets__get_item"), Decision::Allow);
        // Deny: a destructive-named tool on the read-only connector is hard-denied...
        assert_eq!(p.check("widgets__delete_item"), Decision::Deny);
        assert_eq!(p.check("widgets__wipe_all"), Decision::Deny);
        // False-positive-safety: a DIFFERENT connector's destructive tool is untouched by this
        // preset (governed only by the explicit rules, which here allow everything).
        assert_eq!(p.check("gadgets__delete_item"), Decision::Allow);
    }

    #[test]
    fn b11_read_only_override_cannot_be_widened_back_open_by_an_explicit_allow_rule() {
        // Even an operator-authored rule that explicitly allows the exact destructive action must
        // NOT override the read-only preset — it is a hard override, checked first.
        let p = policy_from(
            r#"
rules:
  - action: "widgets__delete_item"
    allow: true
  - action: "*"
    allow: true
detection:
  read_only: ["widgets"]
"#,
        );
        assert_eq!(
            p.check("widgets__delete_item"),
            Decision::Deny,
            "read-only is a hard override, not just a default"
        );
    }

    // ─── B5: DNS-exfil / subdomain-entropy ───────────────────────────────────────────────────────

    #[test]
    fn max_subdomain_entropy_ignores_the_apex_and_single_label_hosts() {
        assert_eq!(
            max_subdomain_entropy("vendor.com"),
            None,
            "apex only, nothing to inspect"
        );
        assert_eq!(max_subdomain_entropy("localhost"), None, "single label");
        assert!(
            max_subdomain_entropy("api.vendor.com").is_some(),
            "one real subdomain label"
        );
    }

    #[test]
    fn max_subdomain_entropy_stays_well_under_the_default_threshold_for_ordinary_hosts() {
        // False-positive-safety: common, legitimate subdomain shapes must not approach 4.0 bits/char.
        for host in [
            "api.vendor.com",
            "www.example.org",
            "cdn.assets.example.com",
            "eu-west-1.s3.amazonaws.com",
            "docs.github.com",
        ] {
            let e = max_subdomain_entropy(host).unwrap();
            assert!(
                e < 4.0,
                "{host} scored {e:.2}, expected well under the 4.0 default threshold"
            );
        }
    }

    #[test]
    fn max_subdomain_entropy_flags_a_base32_shaped_exfil_payload() {
        // A realistic DNS-exfil shape: encoded payload chunks as subdomain labels.
        let exfil = "khbwy4dxovss4z3jf5xweidwmn2gk4dsn5wg65lsmvzq";
        let e = max_subdomain_entropy(&format!("{exfil}.vendor.com")).unwrap();
        assert!(
            e >= 4.0,
            "expected the encoded payload label to score >= 4.0, got {e:.2}"
        );

        // Multi-label chunking: the flag must fire even if the high-entropy chunk isn't leftmost.
        let chunked = max_subdomain_entropy(&format!("a.b.{exfil}.vendor.com")).unwrap();
        assert!(
            chunked >= 4.0,
            "a high-entropy label anywhere before the apex must be caught, got {chunked:.2}"
        );
    }

    #[test]
    fn shannon_entropy_handles_degenerate_inputs_without_panicking() {
        assert_eq!(shannon_entropy(""), 0.0);
        assert_eq!(shannon_entropy("a"), 0.0);
        assert_eq!(shannon_entropy("aaaa"), 0.0, "zero variety -> zero entropy");
        assert!(shannon_entropy("abcd") > 0.0);
    }

    #[test]
    fn b11_bare_namespace_and_explicit_glob_forms_are_equivalent() {
        let bare = policy_from(
            r#"rules: [{action: "*", allow: true}]
detection: { read_only: ["widgets"] }"#,
        );
        let glob = policy_from(
            r#"rules: [{action: "*", allow: true}]
detection: { read_only: ["widgets__*"] }"#,
        );
        for p in [bare, glob] {
            assert_eq!(p.check("widgets__delete_item"), Decision::Deny);
        }
    }

    // ─── F1 model-identity gate (doc 28 §F1) ─────────────────────────────────────────────────────

    #[test]
    fn model_policy_default_warns_on_every_digest_never_blocking() {
        let policy = ModelPolicy::default();
        for digest in [Some("some-digest"), None] {
            let d = policy.evaluate(digest);
            assert!(matches!(d, ModelGateDecision::Warn { .. }), "{d:?}");
            assert!(!d.blocks());
            assert_eq!(d.facet(), "warn");
        }
    }

    #[test]
    fn model_policy_approved_digest_is_allowed() {
        let policy = ModelPolicy {
            approved: vec![ApprovedModelRule {
                digest: "abc123".to_string(),
                tier: EgressTier::Allow,
                label: None,
            }],
            unknown_model: UnknownModelAction::Warn,
        };
        let d = policy.evaluate(Some("abc123"));
        assert_eq!(
            d,
            ModelGateDecision::Allow {
                rule: Some("abc123".to_string())
            }
        );
        assert!(!d.blocks());
        assert_eq!(d.facet(), "allow");
        assert_eq!(d.matched_rule(), Some("abc123"));

        // A DIFFERENT digest is still unknown -> the default (warn) applies.
        let d2 = policy.evaluate(Some("other-digest"));
        assert!(matches!(d2, ModelGateDecision::Warn { .. }));
    }

    #[test]
    fn model_policy_per_digest_deny_and_approval_tiers_block() {
        let policy = ModelPolicy {
            approved: vec![
                ApprovedModelRule {
                    digest: "denied-digest".to_string(),
                    tier: EgressTier::Deny,
                    label: None,
                },
                ApprovedModelRule {
                    digest: "approval-digest".to_string(),
                    tier: EgressTier::Approval,
                    label: None,
                },
            ],
            unknown_model: UnknownModelAction::Warn,
        };
        let deny = policy.evaluate(Some("denied-digest"));
        assert_eq!(deny.facet(), "deny");
        assert!(deny.blocks());
        assert_eq!(deny.matched_rule(), Some("denied-digest"));

        let approval = policy.evaluate(Some("approval-digest"));
        assert_eq!(approval.facet(), "approval_required");
        assert!(
            approval.blocks(),
            "no sync approval channel in v1 — refused honestly"
        );
    }

    #[test]
    fn model_policy_unknown_model_deny_blocks_and_require_approval_blocks() {
        let deny_policy = ModelPolicy {
            approved: vec![],
            unknown_model: UnknownModelAction::Deny,
        };
        let d = deny_policy.evaluate(None);
        assert_eq!(d.facet(), "deny");
        assert!(d.blocks());
        assert_eq!(d.matched_rule(), None);

        let approval_policy = ModelPolicy {
            approved: vec![],
            unknown_model: UnknownModelAction::RequireApproval,
        };
        let a = approval_policy.evaluate(Some("unresolved-but-present"));
        assert_eq!(a.facet(), "approval_required");
        assert!(a.blocks());
    }

    #[test]
    fn model_policy_parses_from_the_same_agent_policy_yaml_shape() {
        let yaml = r#"
rules: [{action: "*", allow: true}]
model:
  approved:
    - digest: "abc123"
      tier: allow
      label: "llama3.1:8b (verified)"
  unknown_model: deny
"#;
        let policy: Policy = serde_yaml::from_str(yaml).expect("model: section parses");
        let model = policy.model().expect("model policy present");
        assert_eq!(model.approved.len(), 1);
        assert_eq!(model.approved[0].digest, "abc123");
        assert_eq!(model.unknown_model, UnknownModelAction::Deny);
    }

    #[test]
    fn model_policy_absent_from_yaml_means_the_default_warn_posture() {
        let yaml = "rules: [{action: \"*\", allow: true}]\n";
        let policy: Policy = serde_yaml::from_str(yaml).expect("parses");
        assert!(policy.model().is_none());
    }

    // ─── F4-wasm — deterministic-execution lane routing (doc 28 §F4) ─────────────────────────────

    #[test]
    fn exec_absent_from_yaml_means_no_routing_at_all() {
        let yaml = "rules: [{action: \"*\", allow: true}]\n";
        let policy: Policy = serde_yaml::from_str(yaml).expect("parses");
        assert!(policy.exec().is_none());
        assert_eq!(policy.resolve_wasm_variant("summarize_text"), None);
    }

    #[test]
    fn exec_policy_parses_from_the_same_agent_policy_yaml_shape() {
        let yaml = r#"
rules: [{action: "*", allow: true}]
exec:
  prefer_deterministic_lane: true
  wasm_variants:
    summarize_text: "tools/text-transform.wasm"
    filter_records: "tools/json-filter.wasm"
"#;
        let policy: Policy = serde_yaml::from_str(yaml).expect("exec: section parses");
        let exec = policy.exec().expect("exec policy present");
        assert!(exec.prefer_deterministic_lane);
        assert_eq!(exec.wasm_variants.len(), 2);
        assert_eq!(
            policy.resolve_wasm_variant("summarize_text"),
            Some("tools/text-transform.wasm")
        );
        assert_eq!(policy.resolve_wasm_variant("filter_records"), Some("tools/json-filter.wasm"));
    }

    #[test]
    fn exec_policy_never_routes_an_unregistered_action() {
        let yaml = r#"
rules: [{action: "*", allow: true}]
exec:
  prefer_deterministic_lane: true
  wasm_variants:
    summarize_text: "tools/text-transform.wasm"
"#;
        let policy: Policy = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(policy.resolve_wasm_variant("some_other_action"), None);
    }

    #[test]
    fn exec_policy_registry_is_inert_when_the_tier_switch_is_off() {
        // Authoring the registry ahead of flipping the switch must be safe — no surprise routing
        // change just from listing variants.
        let yaml = r#"
rules: [{action: "*", allow: true}]
exec:
  wasm_variants:
    summarize_text: "tools/text-transform.wasm"
"#;
        let policy: Policy = serde_yaml::from_str(yaml).expect("parses");
        assert!(!policy.exec().unwrap().prefer_deterministic_lane, "default is off");
        assert_eq!(
            policy.resolve_wasm_variant("summarize_text"),
            None,
            "a registered variant must not route while the tier switch is off"
        );
    }

    // ─── C2 — endpoint budget gate (doc 27 §4 / docs/design/c2-budget-gate.md) ───────────────────

    #[test]
    fn budgets_absent_from_yaml_means_no_gate_at_all() {
        let yaml = "rules: [{action: \"*\", allow: true}]\n";
        let policy: Policy = serde_yaml::from_str(yaml).expect("parses");
        assert!(policy.budgets().is_none());
    }

    #[test]
    fn budgets_parse_from_the_same_agent_policy_yaml_shape() {
        let yaml = r#"
rules: [{action: "*", allow: true}]
budgets:
  max_staleness_secs: 600
  rules:
    - id: "daily-cap"
      scope: rolling-day
      threshold_usd: 25.0
      action: require-approval
      on_missing_state: fail-closed
"#;
        let policy: Policy = serde_yaml::from_str(yaml).expect("budgets: section parses");
        let budgets = policy.budgets().expect("budgets present");
        assert_eq!(budgets.max_staleness_secs, 600);
        assert_eq!(budgets.rules.len(), 1);
        assert_eq!(budgets.rules[0].id, "daily-cap");
        assert_eq!(budgets.rules[0].scope, BudgetScope::RollingDay);
        assert_eq!(budgets.rules[0].action, BudgetBreachAction::RequireApproval);
        assert_eq!(budgets.rules[0].on_missing_state, Some(OnMissingState::FailClosed));
    }

    #[test]
    fn max_staleness_secs_defaults_to_900_when_omitted() {
        let yaml = "rules: [{action: \"*\", allow: true}]\nbudgets:\n  rules: []\n";
        let policy: Policy = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(policy.budgets().unwrap().max_staleness_secs, 900);
    }

    fn ctx<'a>(session_id: Option<&'a str>) -> BudgetCtx<'a> {
        BudgetCtx { session_id, os_user: "alice" }
    }

    #[test]
    fn deny_tier_over_threshold_denies() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 0.01,
                action: BudgetBreachAction::Deny,
                on_missing_state: None,
            }],
        };
        let state = SpendState::synthetic(Some("sheet-1"), Some("h1"), &[("s1", 0.02, 1_000)], None, None);
        match budgets.check(&state, &ctx(Some("s1")), 1_500) {
            BudgetGateDecision::Deny(r) => {
                assert_eq!(r.budget_id, "d1");
                assert_eq!(r.observed_usd, 0.02);
                assert_eq!(r.threshold_usd, 0.01);
                assert!(!r.state_stale);
                assert_eq!(r.on_missing_state, None);
                assert_eq!(r.state_source, "live-tick");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn under_threshold_passes() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 5.0,
                action: BudgetBreachAction::Deny,
                on_missing_state: None,
            }],
        };
        let state = SpendState::synthetic(None, None, &[("s1", 1.0, 1_000)], None, None);
        assert_eq!(budgets.check(&state, &ctx(Some("s1")), 1_500), BudgetGateDecision::Pass);
    }

    #[test]
    fn missing_state_on_a_deny_rule_fails_closed_by_default() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 0.01,
                action: BudgetBreachAction::Deny,
                on_missing_state: None,
            }],
        };
        let state = SpendState::empty();
        match budgets.check(&state, &ctx(Some("s1")), 1_500) {
            BudgetGateDecision::Deny(r) => {
                assert_eq!(r.on_missing_state, Some(OnMissingState::FailClosed));
                assert!(r.state_stale);
                assert_eq!(r.state_source, "none");
            }
            other => panic!("expected fail-closed Deny, got {other:?}"),
        }
    }

    #[test]
    fn missing_state_on_a_require_approval_rule_routes_to_approval_by_default() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "a1".into(),
                scope: BudgetScope::RollingDay,
                threshold_usd: 25.0,
                action: BudgetBreachAction::RequireApproval,
                on_missing_state: None,
            }],
        };
        let state = SpendState::empty();
        match budgets.check(&state, &ctx(None), 1_500) {
            BudgetGateDecision::Approval(r) => {
                assert_eq!(r.on_missing_state, Some(OnMissingState::RequireApproval));
            }
            other => panic!("expected Approval, got {other:?}"),
        }
    }

    #[test]
    fn missing_state_on_a_warn_rule_fails_open_by_default_but_still_receipts() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "w1".into(),
                scope: BudgetScope::User,
                threshold_usd: 100.0,
                action: BudgetBreachAction::Warn,
                on_missing_state: None,
            }],
        };
        let state = SpendState::empty();
        match budgets.check(&state, &ctx(None), 1_500) {
            BudgetGateDecision::Warn(r) => {
                assert_eq!(r.on_missing_state, Some(OnMissingState::FailOpen));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn an_explicit_fail_open_override_on_a_deny_rule_is_a_receipted_loosening_never_the_default() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 0.01,
                action: BudgetBreachAction::Deny,
                on_missing_state: Some(OnMissingState::FailOpen),
            }],
        };
        let state = SpendState::empty();
        match budgets.check(&state, &ctx(Some("s1")), 1_500) {
            BudgetGateDecision::Warn(r) => {
                assert_eq!(r.on_missing_state, Some(OnMissingState::FailOpen));
                assert_eq!(r.action, BudgetBreachAction::Deny, "the rule's OWN tier is still deny");
            }
            other => panic!("expected an explicit fail-open Warn, got {other:?}"),
        }
    }

    #[test]
    fn stale_state_is_treated_exactly_like_missing_state() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 60, // 1 minute
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 0.01,
                action: BudgetBreachAction::Deny,
                on_missing_state: None,
            }],
        };
        // state as_of_ms = 0, now = 120_000ms (2 minutes) later -> 120s stale, > 60s max.
        let state = SpendState::synthetic(None, None, &[("s1", 0.02, 0)], None, None);
        match budgets.check(&state, &ctx(Some("s1")), 120_000) {
            BudgetGateDecision::Deny(r) => {
                assert!(r.state_stale);
                assert_eq!(r.on_missing_state, Some(OnMissingState::FailClosed));
            }
            other => panic!("expected stale-state Deny, got {other:?}"),
        }
    }

    #[test]
    fn a_session_scope_rule_with_no_session_id_is_missing_state_never_silently_skipped() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 0.01,
                action: BudgetBreachAction::Deny,
                on_missing_state: None,
            }],
        };
        let state = SpendState::synthetic(None, None, &[("s1", 100.0, 1_000)], None, None);
        match budgets.check(&state, &ctx(None), 1_500) {
            BudgetGateDecision::Deny(r) => {
                assert_eq!(r.scope_key, "unresolved-session");
            }
            other => panic!("expected fail-closed Deny on unresolvable scope key, got {other:?}"),
        }
    }

    #[test]
    fn strictest_wins_across_multiple_breaching_rules() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![
                BudgetRule {
                    id: "warn-rule".into(),
                    scope: BudgetScope::Session,
                    threshold_usd: 1.0,
                    action: BudgetBreachAction::Warn,
                    on_missing_state: None,
                },
                BudgetRule {
                    id: "approval-rule".into(),
                    scope: BudgetScope::RollingDay,
                    threshold_usd: 1.0,
                    action: BudgetBreachAction::RequireApproval,
                    on_missing_state: None,
                },
                BudgetRule {
                    id: "deny-rule".into(),
                    scope: BudgetScope::User,
                    threshold_usd: 1.0,
                    action: BudgetBreachAction::Deny,
                    on_missing_state: None,
                },
            ],
        };
        let state = SpendState::synthetic(
            None,
            None,
            &[("s1", 5.0, 1_000)],
            Some((5.0, 1_000)),
            Some((5.0, 1_000)),
        );
        match budgets.check(&state, &ctx(Some("s1")), 1_500) {
            BudgetGateDecision::Deny(r) => assert_eq!(r.budget_id, "deny-rule"),
            other => panic!("expected the strictest (deny) rule to win, got {other:?}"),
        }
    }

    #[test]
    fn action_allow_plus_budget_deny_denies_never_the_reverse() {
        // Mirrors the hook-lane composition row (Red-team b): a budget can only ESCALATE an
        // Allowed action — never loosen a prior deny (which this crate's Policy::check already
        // guarantees by early-returning before the budget consult is ever reached).
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 0.01,
                action: BudgetBreachAction::Deny,
                on_missing_state: None,
            }],
        };
        let state = SpendState::synthetic(None, None, &[("s1", 1.0, 1_000)], None, None);
        assert!(matches!(
            budgets.check(&state, &ctx(Some("s1")), 1_500),
            BudgetGateDecision::Deny(_)
        ));
    }

    #[test]
    fn scope_key_is_hashed_never_the_raw_session_id_or_username() {
        let budgets = BudgetPolicy {
            max_staleness_secs: 900,
            rules: vec![BudgetRule {
                id: "d1".into(),
                scope: BudgetScope::Session,
                threshold_usd: 0.01,
                action: BudgetBreachAction::Deny,
                on_missing_state: None,
            }],
        };
        let state = SpendState::synthetic(None, None, &[("super-secret-session-id", 1.0, 1_000)], None, None);
        match budgets.check(&state, &ctx(Some("super-secret-session-id")), 1_500) {
            BudgetGateDecision::Deny(r) => {
                assert_ne!(r.scope_key, "super-secret-session-id");
                assert_eq!(r.scope_key.len(), 64, "a sha256 hex digest");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }
}
