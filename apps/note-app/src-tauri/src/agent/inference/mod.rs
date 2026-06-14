//! Inference backends. A backend looks at the goal, the current app state, the available
//! tools, and the history so far, and decides the next step: call an action, or finish.
//!
//! Everything behind this trait is swappable — deterministic today, an LLM tomorrow —
//! without the host loop changing.

pub mod claude_cli;
pub mod deterministic;

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
        _ => Box::new(deterministic::DeterministicOrganizer::new()),
    }
}
