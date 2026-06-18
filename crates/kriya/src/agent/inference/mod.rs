//! Inference backends. A backend looks at the goal, the current app state, the available
//! tools, and the history so far, and decides the next step: call an action, or finish.
//!
//! Everything behind this trait is swappable — deterministic today, an LLM tomorrow —
//! without the host loop changing.
//!
//! The host crate ships three LLM backends. The "deterministic" backend (a scripted,
//! zero-cost planner used for tests and demos) is **app-specific** and lives in each
//! application; the app passes its deterministic as the `default` to
//! [`select_backend_with_default`].

pub mod anthropic;
pub mod claude_cli;
pub mod ollama;
pub mod scripted;

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
    /// Short summaries of actions from PRIOR runs, recalled from durable memory.
    pub recent_memory: &'a [String],
}

/// The backend's decision for one turn of the loop.
#[derive(Debug, Clone)]
pub enum StepDecision {
    Call { action_id: String, params: Value, reasoning: String },
    Done { summary: String },
}

/// Where a backend's inference happens, network-wise (R13). This is what lets the host
/// make and *attest* an on-device "nothing leaves the device" guarantee for regulated apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkProfile {
    /// No network access at all — a deterministic/scripted planner.
    None,
    /// Talks only to a loopback service on this machine (e.g. a local Ollama model). Data
    /// never leaves the device.
    Localhost,
    /// Reaches a remote third party — prompts and data can leave the device.
    Remote,
}

impl NetworkProfile {
    /// True when running this backend means data can leave the device. `None` and
    /// `Localhost` stay on-device; only `Remote` egresses.
    pub fn egresses(self) -> bool {
        matches!(self, NetworkProfile::Remote)
    }

    pub fn label(self) -> &'static str {
        match self {
            NetworkProfile::None => "no-network",
            NetworkProfile::Localhost => "localhost-only",
            NetworkProfile::Remote => "remote",
        }
    }
}

pub trait Inference: Send {
    fn name(&self) -> &'static str;
    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String>;

    /// The backend's network reach (R13). Defaults to [`NetworkProfile::Remote`] — a backend
    /// is assumed to egress unless it explicitly declares otherwise, so a newly added backend
    /// is never *silently* treated as on-device. The host consults this to enforce + attest
    /// the on-device guarantee.
    fn network_profile(&self) -> NetworkProfile {
        NetworkProfile::Remote
    }
}

/// Select a backend from the `AGENT_BACKEND` env var, falling back to `default` when no
/// LLM backend is requested. The default is whatever app-specific deterministic planner
/// the consuming app supplies.
pub fn select_backend_with_default(default: Box<dyn Inference>) -> Box<dyn Inference> {
    match std::env::var("AGENT_BACKEND").unwrap_or_default().as_str() {
        "claude-cli" | "claude" => Box::new(claude_cli::ClaudeCli::new()),
        "ollama" => Box::new(ollama::Ollama::new()),
        "anthropic" => Box::new(anthropic::Anthropic::new()),
        _ => default,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (reused by every LLM backend)
// ---------------------------------------------------------------------------

/// True if `url`'s host is a loopback address, so traffic to it never leaves the machine.
/// Used by backends that talk to a configurable host (e.g. Ollama) to report an honest
/// [`NetworkProfile`] — pointing the host at a remote box correctly downgrades the guarantee.
pub(crate) fn is_loopback_url(url: &str) -> bool {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = after_scheme.split('/').next().unwrap_or(after_scheme);
    // Strip a trailing :port (but keep IPv6 bracketed hosts intact).
    let host = if authority.starts_with('[') {
        authority.split(']').next().unwrap_or(authority).trim_start_matches('[')
    } else {
        authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority)
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

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

    let memory = if ctx.recent_memory.is_empty() {
        "(no prior runs)".to_string()
    } else {
        ctx.recent_memory.iter().map(|m| format!("- {m}")).collect::<Vec<_>>().join("\n")
    };

    format!(
        r#"You are the agent driving a desktop application. Decide the SINGLE next action.

GOAL:
{goal}

AVAILABLE ACTIONS (call one by name with params matching its input_schema):
{tools}

CURRENT APP STATE:
{state}

MEMORY (recent actions from earlier runs, for context):
{memory}

ACTIONS ALREADY TAKEN (this run):
{history}

Respond with ONLY a JSON object, no prose, no code fences. Either:
  {{"action": "<action_name>", "params": {{...}}, "reasoning": "<one sentence>"}}
to take the next action, or:
  {{"done": true, "summary": "<one sentence>"}}
when the goal is fully met. Take exactly one action per response."#,
        goal = ctx.goal,
        tools = tools,
        state = state,
        memory = memory,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_profile_egress_classification() {
        assert!(!NetworkProfile::None.egresses());
        assert!(!NetworkProfile::Localhost.egresses());
        assert!(NetworkProfile::Remote.egresses());
    }

    #[test]
    fn loopback_urls_are_recognized() {
        assert!(is_loopback_url("http://localhost:11434"));
        assert!(is_loopback_url("http://127.0.0.1:11434"));
        assert!(is_loopback_url("http://127.0.0.1"));
        assert!(is_loopback_url("http://127.1.2.3:11434"));
        assert!(is_loopback_url("http://[::1]:11434"));
        // Anything off the loopback interface egresses.
        assert!(!is_loopback_url("http://ollama.example.com:11434"));
        assert!(!is_loopback_url("https://api.anthropic.com"));
        assert!(!is_loopback_url("http://10.0.0.5:11434"));
        assert!(!is_loopback_url("http://192.168.1.20:11434"));
    }

    #[test]
    fn backends_declare_honest_network_profiles() {
        // Deterministic = no network; the cloud backends egress; the local `claude` CLI is
        // convenient but still reaches the cloud, so it is honestly Remote.
        assert_eq!(scripted::ScriptedPlanner::from_decisions(vec![]).network_profile(), NetworkProfile::None);
        assert_eq!(anthropic::Anthropic::new().network_profile(), NetworkProfile::Remote);
        assert_eq!(claude_cli::ClaudeCli::new().network_profile(), NetworkProfile::Remote);
    }
}
