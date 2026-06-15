//! A scripted, zero-cost backend that proves the full protocol without any LLM.
//!
//! Its "reasoning" is keyword classification: for each still-uncategorized note it emits
//! an `edit_note` call assigning a category, then reports done. This is the planner the
//! Phase 0 demo runs by default.

use std::collections::HashSet;

use serde_json::{json, Value};

use kriya::{Inference, StepContext, StepDecision};

pub struct DeterministicOrganizer {
    /// Note ids we've already issued an edit for, so we never loop on one.
    attempted: HashSet<String>,
}

impl DeterministicOrganizer {
    pub fn new() -> Self {
        Self { attempted: HashSet::new() }
    }
}

impl Inference for DeterministicOrganizer {
    fn name(&self) -> &'static str {
        "deterministic"
    }

    fn next_step(&mut self, ctx: &StepContext) -> Result<StepDecision, String> {
        let notes = ctx
            .state
            .get("notes")
            .and_then(Value::as_array)
            .ok_or_else(|| "state.notes missing or not an array".to_string())?;

        // Delete-style goals propose delete_note (which the policy holds for approval).
        // Match explicit removal phrases so an organize goal that merely says "do not
        // delete notes" is NOT mistaken for a deletion task.
        let goal = ctx.goal.to_lowercase();
        let wants_delete = ["delete every", "delete all", "remove every", "remove all"]
            .iter()
            .any(|p| goal.contains(p));
        if wants_delete {
            let target = ["work", "shopping", "personal", "ideas"]
                .into_iter()
                .find(|c| goal.contains(c));
            for note in notes {
                let id = note.get("id").and_then(Value::as_str).unwrap_or_default();
                if id.is_empty() || self.attempted.contains(id) {
                    continue;
                }
                let category = note.get("category").and_then(Value::as_str).unwrap_or_default();
                if let Some(t) = target {
                    if category != t {
                        continue;
                    }
                }
                let title = note.get("title").and_then(Value::as_str).unwrap_or_default();
                self.attempted.insert(id.to_string());
                return Ok(StepDecision::Call {
                    action_id: "delete_note".to_string(),
                    params: json!({ "id": id }),
                    reasoning: format!(
                        "Goal asks to remove {} notes; proposing to delete \"{title}\".",
                        target.unwrap_or("matching")
                    ),
                });
            }
            return Ok(StepDecision::Done {
                summary: "No more matching notes to remove.".to_string(),
            });
        }

        for note in notes {
            let id = note.get("id").and_then(Value::as_str).unwrap_or_default();
            let category = note.get("category").and_then(Value::as_str).unwrap_or_default();
            if id.is_empty() || self.attempted.contains(id) {
                continue;
            }
            if !category.trim().is_empty() {
                continue; // already categorized by a human or earlier step
            }
            let title = note.get("title").and_then(Value::as_str).unwrap_or_default();
            let content = note.get("content").and_then(Value::as_str).unwrap_or_default();
            let chosen = classify(title, content);
            self.attempted.insert(id.to_string());
            return Ok(StepDecision::Call {
                action_id: "edit_note".to_string(),
                params: json!({ "id": id, "category": chosen }),
                reasoning: format!("\"{title}\" looks like {chosen}; assigning that category."),
            });
        }

        let categorized = notes
            .iter()
            .filter(|n| {
                n.get("category")
                    .and_then(Value::as_str)
                    .map(|c| !c.trim().is_empty())
                    .unwrap_or(false)
            })
            .count();
        Ok(StepDecision::Done {
            summary: format!("Categorized {categorized} of {} notes.", notes.len()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{StepContext, StepDecision, StepRecord};
    use super::*;
    use crate::protocol::ToolSchema;

    fn note(id: &str, title: &str, content: &str) -> Value {
        json!({ "id": id, "title": title, "content": content, "category": "" })
    }

    fn note_cat(id: &str, category: &str) -> Value {
        json!({ "id": id, "title": id, "content": "", "category": category })
    }

    fn step(org: &mut DeterministicOrganizer, state: &Value, goal: &str) -> StepDecision {
        let tools: Vec<ToolSchema> = vec![];
        let history: Vec<StepRecord> = vec![];
        let ctx = StepContext { goal, state, tools: &tools, history: &history, recent_memory: &[] };
        org.next_step(&ctx).unwrap()
    }

    /// Drive the organizer through the full loop the host would run, applying each
    /// edit_note to the state, and assert every note ends correctly categorized.
    #[test]
    fn organizes_all_five_notes() {
        let mut state = json!({
            "notes": [
                note("n1", "Buy groceries", "Milk, eggs, bread from the store."),
                note("n2", "Q3 planning meeting", "Prep slides for the client project deadline."),
                note("n3", "Call the dentist", "Schedule a checkup appointment next week."),
                note("n4", "App idea: tide tracker", "Maybe build a small surf app someday."),
                note("n5", "Reply to Sam's email", "Send the updated report to the client Friday."),
            ]
        });

        let tools: Vec<ToolSchema> = vec![];
        let mut org = DeterministicOrganizer::new();
        let mut history: Vec<StepRecord> = Vec::new();
        let mut steps = 0;

        loop {
            let decision = {
                let ctx = StepContext {
                    goal: "organize",
                    state: &state,
                    tools: &tools,
                    history: &history,
                    recent_memory: &[],
                };
                org.next_step(&ctx).unwrap()
            };
            match decision {
                StepDecision::Done { .. } => break,
                StepDecision::Call { action_id, params, .. } => {
                    assert_eq!(action_id, "edit_note");
                    let id = params["id"].as_str().unwrap().to_string();
                    let category = params["category"].as_str().unwrap().to_string();
                    for n in state["notes"].as_array_mut().unwrap() {
                        if n["id"] == json!(id) {
                            n["category"] = json!(category);
                        }
                    }
                    history.push(StepRecord { action_id, params, success: true });
                    steps += 1;
                    assert!(steps <= 10, "organizer did not terminate");
                }
            }
        }

        assert_eq!(steps, 5, "should take exactly one edit per note");
        let cat = |i: usize| state["notes"][i]["category"].as_str().unwrap().to_string();
        assert_eq!(cat(0), "shopping");
        assert_eq!(cat(1), "work");
        assert_eq!(cat(2), "personal");
        assert_eq!(cat(3), "ideas");
        assert_eq!(cat(4), "work");
    }

    #[test]
    fn organize_goal_mentioning_delete_does_not_delete() {
        // The organize goal can contain the word "delete" in a prohibition; it must still
        // categorize, never propose a delete_note.
        let state = json!({ "notes": [note("n1", "Buy groceries", "milk from the store")] });
        let mut org = DeterministicOrganizer::new();
        match step(&mut org, &state, "Organize notes. Do not delete notes.") {
            StepDecision::Call { action_id, .. } => assert_eq!(action_id, "edit_note"),
            other => panic!("expected edit_note, got {other:?}"),
        }
    }

    #[test]
    fn remove_goal_proposes_delete_for_matching_category() {
        let state = json!({
            "notes": [note_cat("keep", "shopping"), note_cat("drop", "ideas")]
        });
        let mut org = DeterministicOrganizer::new();
        match step(&mut org, &state, "Delete every note in the ideas category.") {
            StepDecision::Call { action_id, params, .. } => {
                assert_eq!(action_id, "delete_note");
                assert_eq!(params["id"], json!("drop"));
            }
            other => panic!("expected delete_note for the ideas note, got {other:?}"),
        }
    }
}

/// Cheap keyword classifier into the demo's four buckets.
fn classify(title: &str, content: &str) -> &'static str {
    let text = format!("{title} {content}").to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));

    // Personal/health terms are checked first so "Call the dentist" beats the work bucket.
    if has(&["dentist", "doctor", "gym", "birthday", "family", "friend", "appointment", "checkup"]) {
        "personal"
    } else if has(&["meeting", "project", "deadline", "email", "client", "report", "slides"]) {
        "work"
    } else if has(&["buy", "groceries", "milk", "store", "purchase", "order", "shopping"]) {
        "shopping"
    } else if has(&["idea", "maybe", "someday", "build", "app", "consider"]) {
        "ideas"
    } else {
        "personal"
    }
}
