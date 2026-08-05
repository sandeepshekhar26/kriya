//! F-4 Inc 3 (kriya-console doc 31 §3.6 / doc 33 §4-A) — the payment-lane emission helpers.
//!
//! The Console already fixed the on-the-wire format: `src/lib/purchases.ts` folds a verified
//! `AuditRow[]` into one purchase-receipt *chain* per `pay_id`, and the committed parity fixture
//! (`tests/pay_fixture.rs` → `tests/fixtures/pay_ledger.jsonl`) pins the exact receipt bytes the TS
//! verifier re-derives. This module is the runtime's honest **producer** of that same chain —
//! `kriya.pay.{intent,decision,outcome}` — which `kriya-hook` emits when a governed call matches a
//! `payment`-class action gate (the class promoted to `primary` in F-4 Inc 1).
//!
//! Everything here is **pure + deterministic** so it can be unit-tested in isolation and the bytes
//! it shapes fold byte-identically in the Console:
//!
//! - **`pay_id` needs no marker file.** The hook is a fresh process per call, and the `outcome`
//!   receipt is written by the *post* invocation, the `intent`/`decision` by the *pre* one. They
//!   chain by a `pay_id` derived from fields present and identical in BOTH invocations — the
//!   session id, the tool name, and the canonical `tool_input` — so the post-hook outcome links
//!   onto the pre-hook intent without any cross-process state (contrast O-3's marker, which the
//!   duration lane needs because it correlates *timestamps*, not identity).
//! - **Amount extraction is best-effort and honest.** Only the unambiguous minor-unit shape
//!   (an integer `amount` in minor units + a `currency` string — the Stripe/PaymentIntent shape)
//!   yields `amount_known: true`; a float/major-unit/absent amount yields `amount_known: false`
//!   and no number (never a guess — the design's honesty discipline). A PAN is NEVER read; custody
//!   stays in the EG-B credential broker, and `merchant` is a processor/host string only.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The three chain-linked action ids (the closed pay vocabulary — mirrors `purchases.ts`).
pub const PAY_INTENT: &str = "kriya.pay.intent";
pub const PAY_DECISION: &str = "kriya.pay.decision";
pub const PAY_OUTCOME: &str = "kriya.pay.outcome";

/// The schema-version marker every pay receipt carries (`v:1`, asserted by the parity fixture).
const PAY_SCHEMA_V: u64 = 1;

/// Deterministic `pay_id` shared by the whole chain. Stable across the separate *pre* and *post*
/// hook processes because it hashes only fields identical in both: the session id, the tool name,
/// and the canonical `tool_input`. Prefixed `pay-` + 24 hex chars (collision-resistant enough to
/// group a session's payment calls; never security-bearing — the signatures are).
pub fn pay_id_for(session_id: Option<&str>, tool_name: &str, canonical_input: &str) -> String {
    let mut h = Sha256::new();
    h.update(session_id.unwrap_or("").as_bytes());
    h.update([0u8]);
    h.update(tool_name.as_bytes());
    h.update([0u8]);
    h.update(canonical_input.as_bytes());
    let digest = h.finalize();
    format!("pay-{}", hex::encode(&digest[..12]))
}

/// Best-effort minor-unit `amount` + lowercased `currency` from a payment `tool_input`. Returns
/// `None` (⇒ the caller records `amount_known: false`) for anything ambiguous: only a NON-negative
/// INTEGER `amount` paired with a non-empty string `currency` is trusted as minor units. A float
/// `amount` is a major-unit convention we deliberately will NOT guess-scale.
pub fn extract_amount_minor(tool_input: &Value) -> Option<(i64, String)> {
    let obj = tool_input.as_object()?;
    let amount = obj.get("amount")?;
    let minor = match amount {
        // Integer only. `is_f64()` (e.g. `42.00`) is rejected — we do not know the minor-unit scale.
        Value::Number(n) if n.is_i64() || n.is_u64() => n.as_i64()?,
        _ => return None,
    };
    if minor < 0 {
        return None;
    }
    let currency = obj.get("currency").and_then(Value::as_str)?;
    if currency.is_empty() {
        return None;
    }
    Some((minor, currency.to_ascii_lowercase()))
}

/// `kriya.pay.intent` params — what the agent asked. `merchant` is a processor/host string only
/// (never a PAN); `tool` is the raw hook tool name; `action_id` is the governed action id this
/// payment rode in on. Amount is folded in best-effort per [`extract_amount_minor`].
pub fn intent_params(
    pay_id: &str,
    merchant: Option<&str>,
    tool: &str,
    action_id: &str,
    tool_input: &Value,
) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("v".into(), json!(PAY_SCHEMA_V));
    m.insert("pay_id".into(), json!(pay_id));
    if let Some(mch) = merchant {
        m.insert("merchant".into(), json!(mch));
    }
    match extract_amount_minor(tool_input) {
        Some((minor, cur)) => {
            m.insert("amount_minor".into(), json!(minor));
            m.insert("currency".into(), json!(cur));
            m.insert("amount_known".into(), json!(true));
        }
        None => {
            m.insert("amount_known".into(), json!(false));
        }
    }
    m.insert("tool".into(), json!(tool));
    m.insert("action_id".into(), json!(action_id));
    Value::Object(m)
}

/// `kriya.pay.decision` params — the policy result. `decision` ∈ `approved|held|denied`;
/// `matched_rule` is the gate rule id; `approver` is present only when a human granted an
/// approve-tier gate. `corr_step` points back at the chain root (`pay_id`), the gate corr idiom.
pub fn decision_params(
    pay_id: &str,
    decision: &str,
    matched_rule: &str,
    approver: Option<&str>,
) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("v".into(), json!(PAY_SCHEMA_V));
    m.insert("pay_id".into(), json!(pay_id));
    m.insert("decision".into(), json!(decision));
    m.insert("matched_rule".into(), json!(matched_rule));
    if let Some(a) = approver {
        m.insert("approver".into(), json!(a));
    }
    m.insert("corr_step".into(), json!(pay_id));
    Value::Object(m)
}

/// `kriya.pay.outcome` params — what actually happened. `result` ∈ `executed|denied|timed_out`;
/// `status` is the governed lane's response status when knowable ([`status_of`]), else absent
/// (honest absence, never invented).
pub fn outcome_params(pay_id: &str, result: &str, status: Option<&str>) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("v".into(), json!(PAY_SCHEMA_V));
    m.insert("pay_id".into(), json!(pay_id));
    m.insert("result".into(), json!(result));
    if let Some(s) = status {
        m.insert("status".into(), json!(s));
    }
    m.insert("corr_step".into(), json!(pay_id));
    Value::Object(m)
}

/// Best-effort textual status from a Claude Code `tool_response`: a `status_code`/`status` field
/// (HTTP-ish code or a short status string) when the payment tool surfaces one. Number or non-empty
/// string only; absent ⇒ `None` (the caller omits `status` rather than inventing one).
pub fn status_of(tool_response: Option<&Value>) -> Option<String> {
    let resp = tool_response?;
    for key in ["status_code", "statusCode", "status"] {
        match resp.get(key) {
            Some(Value::Number(n)) => return Some(n.to_string()),
            Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pay_id_is_deterministic_across_processes() {
        let input = r#"{"amount":4200,"currency":"usd"}"#;
        let a = pay_id_for(Some("sess-1"), "mcp__stripe__create_payment_intent", input);
        let b = pay_id_for(Some("sess-1"), "mcp__stripe__create_payment_intent", input);
        assert_eq!(a, b, "same inputs ⇒ same pay_id (pre/post chain link)");
        assert!(a.starts_with("pay-"));
        assert_eq!(a.len(), 4 + 24, "pay- + 24 hex chars");
    }

    #[test]
    fn pay_id_varies_by_session_tool_and_input() {
        let base = pay_id_for(Some("s"), "t", "{}");
        assert_ne!(base, pay_id_for(Some("s2"), "t", "{}"), "session distinguishes");
        assert_ne!(base, pay_id_for(Some("s"), "t2", "{}"), "tool distinguishes");
        assert_ne!(base, pay_id_for(Some("s"), "t", "{\"a\":1}"), "input distinguishes");
        // Absent session is stable (empty-string treated identically).
        assert_eq!(pay_id_for(None, "t", "{}"), pay_id_for(Some(""), "t", "{}"));
    }

    #[test]
    fn amount_extracted_only_for_integer_minor_units() {
        // Stripe/PaymentIntent shape: integer minor units + currency.
        assert_eq!(
            extract_amount_minor(&json!({"amount": 4200, "currency": "USD"})),
            Some((4200, "usd".to_string())),
            "currency lowercased; integer trusted as minor units"
        );
        // A float amount is major-unit-shaped — we refuse to guess-scale it.
        assert_eq!(extract_amount_minor(&json!({"amount": 42.00, "currency": "usd"})), None);
        // No currency ⇒ not trusted.
        assert_eq!(extract_amount_minor(&json!({"amount": 4200})), None);
        // Empty currency ⇒ not trusted.
        assert_eq!(extract_amount_minor(&json!({"amount": 4200, "currency": ""})), None);
        // Negative ⇒ not trusted (refund/adjust shapes are out of scope for v1).
        assert_eq!(extract_amount_minor(&json!({"amount": -100, "currency": "usd"})), None);
        // No amount ⇒ None.
        assert_eq!(extract_amount_minor(&json!({"note": "hi"})), None);
    }

    #[test]
    fn intent_params_honest_amount_absence() {
        let known = intent_params(
            "pay-x",
            Some("api.stripe.com"),
            "mcp__stripe__create_payment_intent",
            "claude-code__mcp__stripe__create_payment_intent",
            &json!({"amount": 4200, "currency": "usd"}),
        );
        assert_eq!(known["amount_known"], json!(true));
        assert_eq!(known["amount_minor"], json!(4200));
        assert_eq!(known["currency"], json!("usd"));
        assert_eq!(known["merchant"], json!("api.stripe.com"));
        assert_eq!(known["v"], json!(1));

        let unknown = intent_params(
            "pay-y",
            None,
            "mcp__acme__checkout_cart",
            "claude-code__mcp__acme__checkout_cart",
            &json!({"cart_id": "c1"}),
        );
        assert_eq!(unknown["amount_known"], json!(false));
        assert!(unknown.get("amount_minor").is_none(), "no guessed number");
        assert!(unknown.get("merchant").is_none(), "absent merchant omitted");
    }

    #[test]
    fn decision_and_outcome_shapes() {
        let approved = decision_params("pay-x", "approved", "pay-server", Some("platform-eng"));
        assert_eq!(approved["decision"], json!("approved"));
        assert_eq!(approved["matched_rule"], json!("pay-server"));
        assert_eq!(approved["approver"], json!("platform-eng"));
        assert_eq!(approved["corr_step"], json!("pay-x"));

        let held = decision_params("pay-y", "held", "pay-tool", None);
        assert_eq!(held["decision"], json!("held"));
        assert!(held.get("approver").is_none(), "no approver on a held decision");

        let executed = outcome_params("pay-x", "executed", Some("200"));
        assert_eq!(executed["result"], json!("executed"));
        assert_eq!(executed["status"], json!("200"));
        assert_eq!(executed["corr_step"], json!("pay-x"));

        let denied = outcome_params("pay-z", "denied", None);
        assert_eq!(denied["result"], json!("denied"));
        assert!(denied.get("status").is_none(), "absent status omitted, never invented");
    }

    #[test]
    fn status_of_reads_common_shapes_else_none() {
        assert_eq!(status_of(Some(&json!({"status_code": 200}))), Some("200".to_string()));
        assert_eq!(status_of(Some(&json!({"statusCode": 402}))), Some("402".to_string()));
        assert_eq!(status_of(Some(&json!({"status": "succeeded"}))), Some("succeeded".to_string()));
        assert_eq!(status_of(Some(&json!({"status": ""}))), None, "empty string is not a status");
        assert_eq!(status_of(Some(&json!({"body": "ok"}))), None);
        assert_eq!(status_of(None), None);
    }
}
