//! Minimal but real permission layer. Every agent action is checked against a policy
//! before the host asks the app to run it. Default is deny-unknown; deletes require
//! human approval (no approval queue exists yet in Phase 0, so they are held/denied).

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    RequiresApproval,
    Deny,
}

#[derive(Debug, Clone, Deserialize)]
struct Rule {
    /// Exact action id, a `prefix_*` glob, or `*` for all.
    action: String,
    #[serde(default)]
    allow: bool,
    #[serde(default)]
    require_approval: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Policy {
    rules: Vec<Rule>,
}

impl Default for Policy {
    fn default() -> Self {
        // Allow writes/creates/edits; deletes need approval; everything else denied.
        Policy {
            rules: vec![
                Rule { action: "create_*".into(), allow: true, require_approval: false },
                Rule { action: "edit_*".into(), allow: true, require_approval: false },
                Rule { action: "delete_*".into(), allow: true, require_approval: true },
                Rule { action: "*".into(), allow: false, require_approval: false },
            ],
        }
    }
}

impl Policy {
    /// Load from a YAML file, falling back to the safe default if absent/invalid.
    pub fn load_or_default(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_yaml::from_str(&text).unwrap_or_else(|_| Policy::default()),
            Err(_) => Policy::default(),
        }
    }

    pub fn check(&self, action_id: &str) -> Decision {
        for rule in &self.rules {
            if matches(&rule.action, action_id) {
                if !rule.allow {
                    return Decision::Deny;
                }
                return if rule.require_approval {
                    Decision::RequiresApproval
                } else {
                    Decision::Allow
                };
            }
        }
        Decision::Deny
    }
}

fn matches(pattern: &str, action_id: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return action_id.starts_with(prefix);
    }
    pattern == action_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_decisions() {
        let p = Policy::default();
        assert_eq!(p.check("create_note"), Decision::Allow);
        assert_eq!(p.check("edit_note"), Decision::Allow);
        assert_eq!(p.check("delete_note"), Decision::RequiresApproval);
        assert_eq!(p.check("wire_money"), Decision::Deny);
    }
}
