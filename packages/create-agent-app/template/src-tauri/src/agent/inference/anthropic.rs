//! An inference backend that calls the Anthropic Messages API directly over HTTPS.
//!
//! Configuration via environment variables:
//!   `ANTHROPIC_API_KEY` – required; your Anthropic secret key.
//!   `ANTHROPIC_MODEL`   – model ID to use (default: `claude-haiku-4-5-20251001`).
//!
//! Enable with `AGENT_BACKEND=anthropic`.

use serde_json::Value;

use super::{build_prompt, extract_json, parse_decision, Inference, StepContext, StepDecision};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";
const MAX_TOKENS: u32 = 1024;

pub struct Anthropic {
    api_key: Option<String>,
    model: String,
}

impl Anthropic {
    pub fn new() -> Self {
        Self {
            api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            model: std::env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
        }
    }
}

impl Inference for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| "ANTHROPIC_API_KEY is not set; cannot use anthropic backend".to_string())?;

        let prompt = build_prompt(ctx);

        let body = serde_json::json!({
            "model":      self.model,
            "max_tokens": MAX_TOKENS,
            "messages": [
                { "role": "user", "content": prompt }
            ],
        });

        let response: Value = ureq::post(API_URL)
            .set("x-api-key", api_key)
            .set("anthropic-version", API_VERSION)
            .set("content-type", "application/json")
            .send_json(body)
            .map_err(|e| format!("anthropic API request failed: {e}"))?
            .into_json()
            .map_err(|e| format!("anthropic response is not valid JSON: {e}"))?;

        let raw = response
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("anthropic response missing content[0].text: {response}")
            })?;

        let json_str = extract_json(raw)
            .ok_or_else(|| format!("no JSON object found in anthropic response: {raw}"))?;
        parse_decision(json_str)
    }
}
