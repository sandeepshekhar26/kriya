//! An inference backend that shells out to the locally-installed `claude` CLI in print
//! mode. A real LLM picks the next action — no API key or billing, it reuses the user's
//! existing CLI session. Enable with `AGENT_BACKEND=claude-cli`.

use std::process::Command;

use super::{build_prompt, extract_json, parse_decision, Inference, StepContext, StepDecision};

pub struct ClaudeCli {
    binary: String,
}

impl ClaudeCli {
    pub fn new() -> Self {
        Self { binary: std::env::var("CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string()) }
    }
}

impl Inference for ClaudeCli {
    fn name(&self) -> &'static str {
        "claude-cli"
    }

    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String> {
        let prompt = build_prompt(ctx);

        let output = Command::new(&self.binary)
            .arg("-p")
            .arg(&prompt)
            .arg("--output-format")
            .arg("text")
            .output()
            .map_err(|e| format!("failed to run `{}`: {e}", self.binary))?;

        if !output.status.success() {
            return Err(format!(
                "claude CLI exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let json_str = extract_json(&stdout)
            .ok_or_else(|| format!("no JSON object found in claude output: {stdout}"))?;
        parse_decision(json_str)
    }
}
