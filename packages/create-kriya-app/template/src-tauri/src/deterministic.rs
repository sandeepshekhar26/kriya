//! A scripted, zero-cost backend that proves the full protocol without any LLM.
//!
//! Reads `count` and the goal text. If the goal mentions a number, the planner steps
//! the counter toward it; otherwise it reports done immediately. This is the planner
//! the scaffolded app runs by default — swap it for a real backend with
//! `AGENT_BACKEND=claude-cli|ollama|anthropic` for live LLM reasoning.

use serde_json::{json, Value};

use kriya::{Inference, StepContext, StepDecision};

pub struct DeterministicPlanner {
    steps: u32,
}

impl DeterministicPlanner {
    pub fn new() -> Self {
        Self { steps: 0 }
    }

    fn extract_target(goal: &str) -> Option<i64> {
        let mut current = String::new();
        let mut found: Option<i64> = None;
        for c in goal.chars() {
            if c.is_ascii_digit() {
                current.push(c);
            } else if !current.is_empty() {
                if let Ok(n) = current.parse::<i64>() {
                    found = Some(n);
                }
                current.clear();
            }
        }
        if !current.is_empty() {
            if let Ok(n) = current.parse::<i64>() {
                found = Some(n);
            }
        }
        found
    }
}

impl Inference for DeterministicPlanner {
    fn name(&self) -> &'static str {
        "deterministic"
    }

    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String> {
        // Safety cap so an unreachable goal can't run forever.
        self.steps += 1;
        if self.steps > 50 {
            return Ok(StepDecision::Done {
                summary: "Step cap reached without meeting the goal.".to_string(),
            });
        }

        let count = ctx
            .state
            .get("count")
            .and_then(Value::as_i64)
            .ok_or_else(|| "state.count missing or not a number".to_string())?;

        let goal = ctx.goal.to_lowercase();

        // Reset phrasing → propose reset_counter (policy holds it for approval).
        if (goal.contains("reset") || goal.contains("zero")) && count != 0 {
            return Ok(StepDecision::Call {
                action_id: "reset_counter".to_string(),
                params: json!({}),
                reasoning: "Goal asks for a reset; proposing reset_counter.".to_string(),
            });
        }

        let target = Self::extract_target(&ctx.goal).unwrap_or(0);

        if count == target {
            return Ok(StepDecision::Done {
                summary: format!("Counter is {count}, matching the goal."),
            });
        }

        if count < target {
            let gap = target - count;
            let by = gap.min(5).max(1);
            return Ok(StepDecision::Call {
                action_id: "increment".to_string(),
                params: json!({ "by": by }),
                reasoning: format!("Counter is {count}, target {target}; +{by}."),
            });
        }

        // Counter overshoots; jump straight to the target.
        Ok(StepDecision::Call {
            action_id: "set_counter".to_string(),
            params: json!({ "value": target }),
            reasoning: format!("Counter {count} > target {target}; setting directly."),
        })
    }
}
