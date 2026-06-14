//! An inference backend that shells out to the locally-installed `claude` CLI in print
//! mode. A real LLM picks the next action — no API key or billing, it reuses the user's
//! existing CLI session. Enable with `AGENT_BACKEND=claude-cli`.

use std::process::Command;

use serde_json::Value;

use super::{Inference, StepContext, StepDecision};

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
        let decision: Value = serde_json::from_str(json_str)
            .map_err(|e| format!("invalid JSON from claude ({e}): {json_str}"))?;

        if decision.get("done").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(StepDecision::Done {
                summary: decision
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("Task complete.")
                    .to_string(),
            });
        }

        let action_id = decision
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("decision missing \"action\": {decision}"))?
            .to_string();
        let params = decision.get("params").cloned().unwrap_or(Value::Null);
        let reasoning = decision
            .get("reasoning")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        Ok(StepDecision::Call { action_id, params, reasoning })
    }
}

fn build_prompt(ctx: &StepContext) -> String {
    let tools = serde_json::to_string_pretty(ctx.tools.iter().map(|t| {
        serde_json::json!({
            "name": t.name,
            "version": t.version,
            "description": t.description,
            "input_schema": t.input_schema,
        })
    }).collect::<Vec<_>>().as_slice())
    .unwrap_or_default();

    let state = serde_json::to_string_pretty(ctx.state).unwrap_or_default();
    let history = ctx
        .history
        .iter()
        .map(|h| format!("- {} {} -> {}", h.action_id, h.params, if h.success { "ok" } else { "FAILED" }))
        .collect::<Vec<_>>()
        .join("\n");
    let history = if history.is_empty() { "(none yet)".to_string() } else { history };

    format!(
        r#"You are the agent driving a desktop note app. Decide the SINGLE next action.

GOAL:
{goal}

AVAILABLE ACTIONS (call one by name with params matching its input_schema):
{tools}

CURRENT APP STATE:
{state}

ACTIONS ALREADY TAKEN:
{history}

Respond with ONLY a JSON object, no prose, no code fences. Either:
  {{"action": "<action_name>", "params": {{...}}, "reasoning": "<one sentence>"}}
to take the next action, or:
  {{"done": true, "summary": "<one sentence>"}}
when the goal is fully met. Take exactly one action per response."#,
        goal = ctx.goal,
        tools = tools,
        state = state,
        history = history,
    )
}

/// Extract the first balanced top-level JSON object from arbitrary CLI output.
fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}
