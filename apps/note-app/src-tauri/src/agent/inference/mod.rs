//! Inference backends. A backend looks at the goal, the current app state, the available
//! tools, and the history so far, and decides the next step: call an action, or finish.
//!
//! Everything behind this trait is swappable — deterministic today, an LLM tomorrow —
//! without the host loop changing.

pub mod anthropic;
pub mod claude_cli;
pub mod deterministic;
pub mod ollama;

use serde_json::Value;

use crate::protocol::ToolSchema;

/// A record of a step the agent already took (fed back as context).
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub action_id: String,
    pub params: Value,
    pub success: bool,
}

/// Everything a backend needs to choose the next step.
pub struct StepContext<'a> {
    pub goal: &'a str,
    pub state: &'a Value,
    pub tools: &'a [ToolSchema],
    pub history: &'a [StepRecord],
}

/// The backend's decision for one turn of the loop.
#[derive(Debug, Clone)]
pub enum StepDecision {
    Call { action_id: String, params: Value, reasoning: String },
    Done { summary: String },
}

pub trait Inference: Send {
    fn name(&self) -> &'static str;
    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String>;
}

/// Select a backend from the `AGENT_BACKEND` env var. Defaults to the deterministic one.
pub fn select_backend() -> Box<dyn Inference> {
    match std::env::var("AGENT_BACKEND").unwrap_or_default().as_str() {
        "claude-cli" | "claude" => Box::new(claude_cli::ClaudeCli::new()),
        "ollama" => Box::new(ollama::Ollama::new()),
        "anthropic" => Box::new(anthropic::Anthropic::new()),
        _ => Box::new(deterministic::DeterministicOrganizer::new()),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (reused by every LLM backend)
// ---------------------------------------------------------------------------

/// Build the standard single-action prompt used by all LLM backends.
pub(crate) fn build_prompt(ctx: &StepContext) -> String {
    let tools = serde_json::to_string_pretty(
        ctx.tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "version": t.version,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect::<Vec<_>>()
            .as_slice(),
    )
    .unwrap_or_default();

    let state = serde_json::to_string_pretty(ctx.state).unwrap_or_default();
    let history = ctx
        .history
        .iter()
        .map(|h| {
            format!(
                "- {} {} -> {}",
                h.action_id,
                h.params,
                if h.success { "ok" } else { "FAILED" }
            )
        })
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

/// Extract the first balanced top-level JSON object from arbitrary LLM output.
pub(crate) fn extract_json(text: &str) -> Option<&str> {
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

/// Parse a JSON object (already extracted) into a `StepDecision`.
///
/// This is the canonical decision-parsing logic shared by all LLM backends.
pub(crate) fn parse_decision(json_str: &str) -> Result<StepDecision, String> {
    let decision: Value = serde_json::from_str(json_str)
        .map_err(|e| format!("invalid JSON from LLM ({e}): {json_str}"))?;

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
    let reasoning =
        decision.get("reasoning").and_then(Value::as_str).unwrap_or("").to_string();

    Ok(StepDecision::Call { action_id, params, reasoning })
}
