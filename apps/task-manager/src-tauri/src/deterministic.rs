//! A scripted, zero-cost planner that proves the full protocol without any LLM.
//!
//! Goal patterns it handles:
//!   - "complete every high-priority task" / "finish all high tasks" → one
//!     `complete_task` per remaining high-priority, undone task.
//!   - "delete every completed task" / "remove done tasks" → one `delete_task`
//!     per task whose `done == true` (policy holds each for human approval).
//!   - Anything else → reports done immediately.
//!
//! This is the planner the demo runs by default. Swap in a real LLM with
//! `AGENT_BACKEND=claude-cli|ollama|anthropic`.

use std::collections::HashSet;

use serde_json::{json, Value};

use agent_native_host::{Inference, StepContext, StepDecision};

pub struct TaskPlanner {
    /// Task ids we've already issued a step for, so we never loop on one.
    attempted: HashSet<String>,
}

impl TaskPlanner {
    pub fn new() -> Self {
        Self { attempted: HashSet::new() }
    }
}

#[derive(Clone, Copy)]
enum Intent {
    DeleteCompleted,
    CompleteByPriority(&'static str),
    CompleteAll,
    None,
}

fn classify(goal: &str) -> Intent {
    let g = goal.to_lowercase();

    let delete_completed = ["delete every completed", "delete all completed",
        "remove every completed", "remove all completed",
        "delete completed", "remove completed", "delete done", "remove done"];
    if delete_completed.iter().any(|p| g.contains(p)) {
        return Intent::DeleteCompleted;
    }

    if g.contains("complete") || g.contains("finish") || g.contains("done") {
        for prio in ["high", "medium", "low"] {
            if g.contains(prio) {
                return Intent::CompleteByPriority(prio);
            }
        }
        if g.contains("every task") || g.contains("all task") || g.contains("all tasks") {
            return Intent::CompleteAll;
        }
    }

    Intent::None
}

impl Inference for TaskPlanner {
    fn name(&self) -> &'static str {
        "deterministic"
    }

    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String> {
        let tasks = ctx
            .state
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(|| "state.tasks missing or not an array".to_string())?;

        let intent = classify(ctx.goal);

        match intent {
            Intent::DeleteCompleted => {
                for task in tasks {
                    let id = task.get("id").and_then(Value::as_str).unwrap_or_default();
                    if id.is_empty() || self.attempted.contains(id) { continue; }
                    let done = task.get("done").and_then(Value::as_bool).unwrap_or(false);
                    if !done { continue; }
                    let title = task.get("title").and_then(Value::as_str).unwrap_or_default();
                    self.attempted.insert(id.to_string());
                    return Ok(StepDecision::Call {
                        action_id: "delete_task".to_string(),
                        params: json!({ "id": id }),
                        reasoning: format!(
                            "Goal asks to clear completed tasks; proposing to delete \"{title}\"."
                        ),
                    });
                }
                Ok(StepDecision::Done {
                    summary: "No more completed tasks to delete.".to_string(),
                })
            }

            Intent::CompleteByPriority(prio) => {
                for task in tasks {
                    let id = task.get("id").and_then(Value::as_str).unwrap_or_default();
                    if id.is_empty() || self.attempted.contains(id) { continue; }
                    let task_prio = task.get("priority").and_then(Value::as_str).unwrap_or("");
                    if task_prio != prio { continue; }
                    let done = task.get("done").and_then(Value::as_bool).unwrap_or(false);
                    if done { continue; }
                    let title = task.get("title").and_then(Value::as_str).unwrap_or_default();
                    self.attempted.insert(id.to_string());
                    return Ok(StepDecision::Call {
                        action_id: "complete_task".to_string(),
                        params: json!({ "id": id }),
                        reasoning: format!(
                            "Goal asks to finish {prio}-priority tasks; completing \"{title}\"."
                        ),
                    });
                }
                Ok(StepDecision::Done {
                    summary: format!("All {prio}-priority tasks are done."),
                })
            }

            Intent::CompleteAll => {
                for task in tasks {
                    let id = task.get("id").and_then(Value::as_str).unwrap_or_default();
                    if id.is_empty() || self.attempted.contains(id) { continue; }
                    let done = task.get("done").and_then(Value::as_bool).unwrap_or(false);
                    if done { continue; }
                    let title = task.get("title").and_then(Value::as_str).unwrap_or_default();
                    self.attempted.insert(id.to_string());
                    return Ok(StepDecision::Call {
                        action_id: "complete_task".to_string(),
                        params: json!({ "id": id }),
                        reasoning: format!("Goal asks to finish every task; completing \"{title}\"."),
                    });
                }
                Ok(StepDecision::Done {
                    summary: "All tasks are done.".to_string(),
                })
            }

            Intent::None => Ok(StepDecision::Done {
                summary: "I don't recognize this goal — try \"finish all high-priority tasks\" or \"delete completed tasks\".".to_string(),
            }),
        }
    }
}
