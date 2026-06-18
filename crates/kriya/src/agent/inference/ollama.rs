//! An inference backend that calls a locally-running Ollama server via its REST API.
//!
//! Configuration via environment variables:
//!   `OLLAMA_HOST`  – base URL of the Ollama server (default: `http://localhost:11434`)
//!   `OLLAMA_MODEL` – model name to use            (default: `llama3.1`)
//!
//! Enable with `AGENT_BACKEND=ollama`.

use serde_json::Value;

use super::{
    build_prompt, extract_json, is_loopback_url, parse_decision, Inference, NetworkProfile,
    StepContext, StepDecision,
};

pub struct Ollama {
    host: String,
    model: String,
}

impl Ollama {
    pub fn new() -> Self {
        Self {
            host: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://localhost:11434".to_string()),
            model: std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.1".to_string()),
        }
    }
}

impl Inference for Ollama {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn network_profile(&self) -> NetworkProfile {
        // On-device only when the model server is on the loopback interface. If OLLAMA_HOST
        // points at a remote box, prompts leave the device — report that honestly.
        if is_loopback_url(&self.host) {
            NetworkProfile::Localhost
        } else {
            NetworkProfile::Remote
        }
    }

    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String> {
        let prompt = build_prompt(ctx);
        let url = format!("{}/api/generate", self.host.trim_end_matches('/'));

        let body = serde_json::json!({
            "model":  self.model,
            "prompt": prompt,
            "stream": false,
            "format": "json",
        });

        let response: Value = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| format!("ollama request to {url} failed: {e}"))?
            .into_json()
            .map_err(|e| format!("ollama response is not valid JSON: {e}"))?;

        let raw = response
            .get("response")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("ollama response missing \"response\" field: {response}"))?;

        let json_str = extract_json(raw)
            .ok_or_else(|| format!("no JSON object found in ollama response: {raw}"))?;
        parse_decision(json_str)
    }
}
