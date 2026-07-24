//! `kriya.model.*` receipt vocabulary (F1, doc 28 §F1): the three receipts `kriya-llm-proxy` emits.
//! Built entirely on the SAME signer path every other lane uses ([`crate::audit::Signer`]) — no new
//! signing code. Field names follow OT-1 (FRONTIER-EXECUTE-PROMPT.md §2): anywhere a GenAI-semconv
//! attribute exists (`gen_ai.request.model`, `gen_ai.usage.input_tokens`/`output_tokens`) it is used
//! verbatim; everything else is `kriya.*`-shaped by construction (namespaced when exported, per OT-2).
//!
//! **No content ever.** Every params builder below takes hashes/counts/names only — never prompt or
//! output TEXT (doc 27 §3's "no content in receipts" invariant, C1 discipline). This is what the F1
//! build's no-content-leak test checks for on the console side.

use serde_json::{json, Map, Value};

/// First sight of a served (model name, resolved digest) pair.
pub const MODEL_IDENTITY: &str = "kriya.model.identity";
/// The policy verdict for one served completion — emitted on EVERY completion (see the module doc
/// on [`crate::permissions::ModelPolicy`] for why "warn" is the common, expected case, not silence).
pub const MODEL_GATE: &str = "kriya.model.gate";
/// Usage/perf for one served (forwarded) completion.
pub const MODEL_SERVE: &str = "kriya.model.serve";

/// `kriya.model.identity` params — emitted once per (model name, digest) first sight.
pub struct IdentityParams<'a> {
    pub model: &'a str,
    pub digest: Option<&'a str>,
    /// One of [`crate::llm::manifest::DigestSource::as_str`]'s three values.
    pub digest_source: &'a str,
    /// From the upstream's own `/api/version` (Ollama) when reachable — `None` if the upstream
    /// doesn't expose one or the lookup hasn't completed yet.
    pub runtime: Option<&'a str>,
    pub runtime_version: Option<&'a str>,
}

impl IdentityParams<'_> {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("gen_ai.request.model".into(), json!(self.model));
        if let Some(d) = self.digest {
            m.insert("digest".into(), json!(d));
        }
        m.insert("digest_source".into(), json!(self.digest_source));
        if let Some(r) = self.runtime {
            m.insert("runtime".into(), json!(r));
        }
        if let Some(v) = self.runtime_version {
            m.insert("runtime_version".into(), json!(v));
        }
        Value::Object(m)
    }
}

/// `kriya.model.gate` params — the policy verdict, emitted on EVERY completion.
pub struct GateParams<'a> {
    pub model: &'a str,
    pub digest: Option<&'a str>,
    /// One of `ModelGateDecision::facet()`'s four values: `"allow" | "warn" | "approval_required" |
    /// "deny"`.
    pub decision: &'a str,
    pub matched_rule: Option<&'a str>,
    pub reason: &'a str,
}

impl GateParams<'_> {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("gen_ai.request.model".into(), json!(self.model));
        if let Some(d) = self.digest {
            m.insert("digest".into(), json!(d));
        }
        m.insert("decision".into(), json!(self.decision));
        if let Some(r) = self.matched_rule {
            m.insert("matched_rule".into(), json!(r));
        }
        m.insert("reason".into(), json!(self.reason));
        Value::Object(m)
    }
}

/// `kriya.model.serve` params — one per completed (forwarded) request.
pub struct ServeParams<'a> {
    pub model: &'a str,
    pub digest: Option<&'a str>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Present-only sampler params (`temperature`, `top_p`, `top_k`, `seed`,
    /// `max_tokens`/`num_predict`, `stop`, …) as an object — never the prompt/output itself.
    pub sampler: Value,
    /// sha256 hex of the canonicalized request prompt/messages — never the text.
    pub prompt_hash: Option<&'a str>,
    /// sha256 hex of the exact response bytes relayed to the client — never the text.
    pub output_hash: Option<&'a str>,
    pub streaming: bool,
    pub duration_ms: u64,
    pub upstream_path: &'a str,
}

impl ServeParams<'_> {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("gen_ai.request.model".into(), json!(self.model));
        if let Some(d) = self.digest {
            m.insert("digest".into(), json!(d));
        }
        if let Some(t) = self.input_tokens {
            m.insert("gen_ai.usage.input_tokens".into(), json!(t));
        }
        if let Some(t) = self.output_tokens {
            m.insert("gen_ai.usage.output_tokens".into(), json!(t));
        }
        if self
            .sampler
            .as_object()
            .map(|o| !o.is_empty())
            .unwrap_or(false)
        {
            m.insert("sampler".into(), self.sampler.clone());
        }
        if let Some(h) = self.prompt_hash {
            m.insert("prompt_hash".into(), json!(h));
        }
        if let Some(h) = self.output_hash {
            m.insert("output_hash".into(), json!(h));
        }
        m.insert("streaming".into(), json!(self.streaming));
        m.insert("duration_ms".into(), json!(self.duration_ms));
        m.insert("upstream_path".into(), json!(self.upstream_path));
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_params_never_carry_a_content_field() {
        let p = IdentityParams {
            model: "llama3.1:8b",
            digest: Some("abc123"),
            digest_source: "ollama-manifest",
            runtime: Some("ollama"),
            runtime_version: Some("0.5.1"),
        };
        let v = p.to_value();
        assert_eq!(v["gen_ai.request.model"], json!("llama3.1:8b"));
        assert_eq!(v["digest"], json!("abc123"));
        assert_eq!(v["digest_source"], json!("ollama-manifest"));
        // Closed field set — nothing beyond what's set above.
        assert_eq!(v.as_object().unwrap().len(), 5);
    }

    #[test]
    fn serve_params_omit_absent_fields_and_never_carry_prompt_or_output_text() {
        let p = ServeParams {
            model: "llama3.1:8b",
            digest: None,
            input_tokens: Some(12),
            output_tokens: Some(34),
            sampler: json!({"temperature": 0.7, "seed": 42}),
            prompt_hash: Some("deadbeef"),
            output_hash: Some("cafef00d"),
            streaming: true,
            duration_ms: 1500,
            upstream_path: "/api/chat",
        };
        let v = p.to_value();
        assert_eq!(v["gen_ai.usage.input_tokens"], json!(12));
        assert_eq!(v["gen_ai.usage.output_tokens"], json!(34));
        assert_eq!(v["sampler"]["seed"], json!(42));
        assert_eq!(v["prompt_hash"], json!("deadbeef"));
        assert_eq!(v["output_hash"], json!("cafef00d"));
        assert!(
            v.get("digest").is_none(),
            "absent digest must be omitted, not null"
        );
        let dump = v.to_string();
        assert!(
            !dump.contains("temperature\":0.7") || dump.contains("sampler"),
            "sampler nested, not top-level leak"
        );
    }

    #[test]
    fn gate_params_round_trip() {
        let p = GateParams {
            model: "mystery-model",
            digest: None,
            decision: "warn",
            matched_rule: None,
            reason: "model digest is not on the approved allowlist",
        };
        let v = p.to_value();
        assert_eq!(v["decision"], json!("warn"));
        assert!(v.get("digest").is_none());
        assert!(v.get("matched_rule").is_none());
    }
}
