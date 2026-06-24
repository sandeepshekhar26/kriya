//! A scripted inference backend: plays back a fixed list of decisions, then reports `Done`.
//!
//! This is the generic, app-agnostic counterpart to the reference apps' hand-written
//! deterministic planners. It exists so the runtime can run with **zero cost, full
//! determinism, and no LLM/API key** — which makes it the natural default for the stdio
//! sidecar (so an Electron/Node integration test can drive a real run) and a building block
//! for a future CI eval gate ("does my app still work with agents?").
//!
//! The script is a JSON array of the *same* decision objects an LLM emits, so a script is
//! just a recording of a known-good run:
//!
//! ```json
//! [
//!   {"action": "create_note", "params": {"title": "Demo"}, "reasoning": "seed a note"},
//!   {"done": true, "summary": "done"}
//! ]
//! ```

use std::path::Path;

use serde_json::Value;

use super::{parse_decision, Inference, NetworkProfile, StepContext, StepDecision};

pub struct ScriptedPlanner {
    steps: Vec<StepDecision>,
    cursor: usize,
}

impl ScriptedPlanner {
    /// Build directly from decisions (used by tests and embedders).
    pub fn from_decisions(steps: Vec<StepDecision>) -> Self {
        Self { steps, cursor: 0 }
    }

    /// Load a script from a JSON file: an array of decision objects (LLM decision shape).
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read script {}: {e}", path.display()))?;
        let items: Vec<Value> = serde_json::from_str(&text)
            .map_err(|e| format!("script must be a JSON array of decisions: {e}"))?;
        let mut steps = Vec::with_capacity(items.len());
        for item in items {
            // Reuse the canonical LLM-decision parser so a script and an LLM run identically.
            steps.push(parse_decision(&item.to_string())?);
        }
        Ok(Self::from_decisions(steps))
    }
}

impl Inference for ScriptedPlanner {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn network_profile(&self) -> NetworkProfile {
        // A scripted/deterministic planner makes no network calls — fully on-device.
        NetworkProfile::None
    }

    fn next_step(&mut self, _ctx: &StepContext) -> Result<StepDecision, String> {
        let decision = self
            .steps
            .get(self.cursor)
            .cloned()
            .unwrap_or(StepDecision::Done {
                summary: "script complete".into(),
            });
        self.cursor += 1;
        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx<'a>() -> StepContext<'a> {
        StepContext {
            goal: "",
            state: &Value::Null,
            tools: &[],
            history: &[],
            recent_memory: &[],
        }
    }

    #[test]
    fn plays_back_then_reports_done() {
        let mut p = ScriptedPlanner::from_decisions(vec![StepDecision::Call {
            action_id: "create_note".into(),
            params: json!({"title": "x"}),
            reasoning: "r".into(),
        }]);
        assert!(matches!(p.next_step(&ctx()), Ok(StepDecision::Call { .. })));
        // Past the end, it is Done — the loop terminates rather than looping forever.
        assert!(matches!(p.next_step(&ctx()), Ok(StepDecision::Done { .. })));
        assert!(matches!(p.next_step(&ctx()), Ok(StepDecision::Done { .. })));
    }

    #[test]
    fn parses_a_script_file_in_llm_decision_shape() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("kriya-script-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(
            &path,
            r#"[
                {"action":"create_note","params":{"title":"Demo"},"reasoning":"seed"},
                {"done":true,"summary":"finished"}
            ]"#,
        )
        .unwrap();

        let mut p = ScriptedPlanner::from_file(&path).unwrap();
        match p.next_step(&ctx()).unwrap() {
            StepDecision::Call {
                action_id, params, ..
            } => {
                assert_eq!(action_id, "create_note");
                assert_eq!(params["title"], "Demo");
            }
            other => panic!("expected Call, got {other:?}"),
        }
        assert!(matches!(
            p.next_step(&ctx()).unwrap(),
            StepDecision::Done { .. }
        ));
        let _ = std::fs::remove_file(&path);
    }
}
