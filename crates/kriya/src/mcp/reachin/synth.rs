//! Tool synthesis — the pure transform at the heart of Front 2: `Vec<AxNode>` → `Vec<Tool>`.
//! One MCP tool per *actionable* element. No FFI, no policy, no I/O here — so it is fully
//! unit-testable and the macOS backend only has to produce honest [`AxNode`]s.
//!
//! Synthesis rules (kept conservative on purpose — this is the seam an agent reasons about):
//! - **Actionable only:** an element with at least one supported AX action and `enabled == true`.
//!   A disabled control can't be pressed, so exposing it as a tool would just invite failed calls.
//! - **One tool per (element, action):** an element supporting several actions (rare, e.g. press +
//!   showmenu) yields one tool each, so the agent picks the action explicitly.
//! - **Name = `<verb>_<role>_<title-slug>`**, sanitized to `[a-z0-9_]`, deduplicated with a numeric
//!   suffix. The verb is derived from the AX action (`AXPress` → `press`), the role from the AX role
//!   (`AXButton` → `button`), so the name reads like an MCP convention name the policy can match.
//! - **Description = human role + title**, so a model sees "Press the 'Delete' button" not a slug.
//! - **inputSchema = `{type:object}`** — AX actions take no parameters in the MVP (a press is a
//!   press); richer schemas (set a text field's value) are a follow-up gated on coverage.

use std::collections::HashMap;

use serde_json::json;

use crate::mcp::jsonrpc::Tool;

use super::AxNode;

/// One synthesized tool plus the `(node_id, action)` it routes to. The server keeps the `Tool`s
/// for `tools/list`; the [`super::executor::AxExecutor`] keeps the full mapping so it can turn a
/// tool name back into the AX call to perform. Returned together from [`synthesize`] so the two
/// stay in lockstep (same name → same routing).
#[derive(Debug, Clone)]
pub struct SynthesizedTool {
    pub tool: Tool,
    /// The [`AxNode::id`] this tool's call performs against.
    pub node_id: String,
    /// The AX action name to perform, e.g. `"AXPress"`.
    pub action: String,
}

/// Synthesize tools **with** their routing. The server and executor are both built from this so a
/// tool name always maps to exactly one `(node, action)`.
pub fn synthesize(nodes: &[AxNode]) -> Vec<SynthesizedTool> {
    let mut out = Vec::new();
    // Tracks how many times a base name has been used so duplicates get a stable numeric suffix.
    let mut seen: HashMap<String, usize> = HashMap::new();

    for node in nodes {
        // Disabled elements can't be actuated — skip rather than offer a tool that always fails.
        if !node.enabled {
            continue;
        }
        for action in &node.actions {
            let verb = action_verb(action);
            let role = role_word(&node.role);
            let base = sanitized_name(&verb, &role, &node.title);
            let name = dedupe(&mut seen, base);

            let description = describe(action, &node.role, &node.title);
            out.push(SynthesizedTool {
                tool: Tool {
                    name,
                    description,
                    // AX actions are parameterless in the MVP; an empty object schema is valid MCP.
                    input_schema: json!({ "type": "object", "properties": {} }),
                },
                node_id: node.id.clone(),
                action: action.clone(),
            });
        }
    }
    out
}

/// Just the `Tool` views, for the server's `tools/list`.
pub fn synthesize_tools(nodes: &[AxNode]) -> Vec<Tool> {
    synthesize(nodes).into_iter().map(|s| s.tool).collect()
}

/// Map an AX action constant to a short verb for the tool name. `AXPress` → `press`,
/// `AXShowMenu` → `show_menu`, `AXIncrement` → `increment`. Unknown actions fall back to a
/// sanitized form of the raw name so nothing is silently dropped.
fn action_verb(action: &str) -> String {
    let trimmed = action.strip_prefix("AX").unwrap_or(action);
    to_snake(trimmed)
}

/// Map an AX role to a short word for the tool name. `AXButton` → `button`, `AXMenuItem` →
/// `menu_item`. Empty/unknown roles become `element`.
fn role_word(role: &str) -> String {
    let trimmed = role.strip_prefix("AX").unwrap_or(role);
    let word = to_snake(trimmed);
    if word.is_empty() {
        "element".to_string()
    } else {
        word
    }
}

/// Build the base tool name `<verb>_<role>[_<title-slug>]`. The title slug is omitted when the
/// title is empty, so an untitled button is still a valid `press_button` (deduped if repeated).
///
/// Final `[a-z0-9_]` guard: `verb`/`role` come from raw AX action/role strings via `to_snake`, which
/// splits CamelCase but does NOT drop punctuation. Real apps expose pathological action names with
/// colons, newlines, and parens (e.g. macOS Calculator's "Name:Copy\nTarget:0x0\nSelector:(null)"),
/// which would violate MCP's tool-name charset and break a strict client's `tools/list`. So slugify
/// the assembled name and clamp its length so every synthesized name is a valid, sane identifier.
fn sanitized_name(verb: &str, role: &str, title: &str) -> String {
    let slug = slugify(title);
    let base = if slug.is_empty() {
        format!("{verb}_{role}")
    } else {
        format!("{verb}_{role}_{slug}")
    };
    let mut clean = slugify(&base);
    if clean.is_empty() {
        clean = "action".to_string();
    }
    // Some apps expose whole-sentence action names; keep tool names readable and bounded.
    if clean.len() > 64 {
        clean.truncate(64);
        while clean.ends_with('_') {
            clean.pop();
        }
    }
    clean
}

/// Ensure uniqueness: the first use of a base name keeps it; later collisions get `_2`, `_3`, …
/// Deterministic in input order so a stable snapshot yields stable names across runs.
fn dedupe(seen: &mut HashMap<String, usize>, base: String) -> String {
    let count = seen.entry(base.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        base
    } else {
        format!("{base}_{count}")
    }
}

/// Human-readable description: "Press the 'Delete' AXButton". Falls back gracefully when the title
/// is empty so the agent still learns the role + action.
fn describe(action: &str, role: &str, title: &str) -> String {
    let verb = action.strip_prefix("AX").unwrap_or(action);
    if title.is_empty() {
        format!("{verb} the {role} (no title)")
    } else {
        format!("{verb} the '{title}' {role}")
    }
}

/// Convert a CamelCase/AX-style identifier to snake_case: `ShowMenu` → `show_menu`. ASCII only —
/// AX role/action constants are ASCII by definition.
fn to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Slugify a human title into `[a-z0-9_]`: lowercase, runs of non-alphanumerics collapse to a
/// single `_`, leading/trailing `_` trimmed. `"Save & Close…"` → `"save_close"`.
fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_sep = true; // start true so leading separators don't add a leading underscore
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, role: &str, title: &str, actions: &[&str], enabled: bool) -> AxNode {
        AxNode {
            id: id.into(),
            role: role.into(),
            title: title.into(),
            actions: actions.iter().map(|a| a.to_string()).collect(),
            enabled,
        }
    }

    #[test]
    fn one_tool_per_enabled_actionable_node() {
        let nodes = vec![
            node("1", "AXButton", "Save", &["AXPress"], true),
            node("2", "AXButton", "Quit", &["AXPress"], true),
        ];
        let tools = synthesize_tools(&nodes);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["press_button_save", "press_button_quit"]);
    }

    #[test]
    fn disabled_and_actionless_nodes_yield_no_tool() {
        let nodes = vec![
            node("1", "AXButton", "Disabled", &["AXPress"], false),
            node("2", "AXStaticText", "label", &[], true), // no actions
        ];
        assert!(synthesize_tools(&nodes).is_empty());
    }

    #[test]
    fn names_are_sanitized_to_safe_chars() {
        let nodes = vec![node("1", "AXButton", "Save & Close…", &["AXPress"], true)];
        let tools = synthesize_tools(&nodes);
        assert_eq!(tools[0].name, "press_button_save_close");
        // Only [a-z0-9_].
        assert!(tools[0]
            .name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
    }

    #[test]
    fn pathological_action_names_are_sanitized_to_valid_identifiers() {
        // A real app (macOS Calculator) exposed AX action names with colons, newlines, and parens.
        // Every synthesized tool name must still be [a-z0-9_] so a strict MCP client accepts it.
        let nodes = vec![node(
            "1",
            "AXScrollArea",
            "Last Expression",
            &["Name:Copy\nTarget:0x0\nSelector:(null)"],
            true,
        )];
        let tools = synthesize_tools(&nodes);
        assert_eq!(tools.len(), 1);
        let n = &tools[0].name;
        assert!(
            n.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "name must be [a-z0-9_], got: {n:?}"
        );
        assert!(!n.contains("__"), "no doubled underscores: {n:?}");
        assert!(!n.starts_with('_') && !n.ends_with('_'), "trimmed: {n:?}");
    }

    #[test]
    fn very_long_names_are_clamped() {
        let long_title = "word ".repeat(50); // ~250 chars before slugging
        let nodes = vec![node("1", "AXButton", &long_title, &["AXPress"], true)];
        let tools = synthesize_tools(&nodes);
        assert!(
            tools[0].name.len() <= 64,
            "len {} > 64",
            tools[0].name.len()
        );
        assert!(!tools[0].name.ends_with('_'));
    }

    #[test]
    fn duplicate_names_get_a_numeric_suffix() {
        // Two enabled buttons both titled "OK" → unique names.
        let nodes = vec![
            node("1", "AXButton", "OK", &["AXPress"], true),
            node("2", "AXButton", "OK", &["AXPress"], true),
        ];
        let tools = synthesize_tools(&nodes);
        assert_eq!(tools[0].name, "press_button_ok");
        assert_eq!(tools[1].name, "press_button_ok_2");
    }

    #[test]
    fn untitled_element_still_gets_a_valid_name() {
        let nodes = vec![node("1", "AXButton", "", &["AXPress"], true)];
        let tools = synthesize_tools(&nodes);
        assert_eq!(tools[0].name, "press_button");
    }

    #[test]
    fn multiple_actions_yield_one_tool_each() {
        let nodes = vec![node(
            "1",
            "AXButton",
            "Options",
            &["AXPress", "AXShowMenu"],
            true,
        )];
        let tools = synthesize_tools(&nodes);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["press_button_options", "show_menu_button_options"]
        );
    }

    #[test]
    fn description_is_human_readable() {
        let nodes = vec![node("1", "AXButton", "Delete", &["AXPress"], true)];
        let tools = synthesize_tools(&nodes);
        assert_eq!(tools[0].description, "Press the 'Delete' AXButton");
    }

    #[test]
    fn synthesize_preserves_routing_to_node_and_action() {
        let nodes = vec![node("node-7", "AXMenuItem", "Export", &["AXPress"], true)];
        let synth = synthesize(&nodes);
        assert_eq!(synth.len(), 1);
        assert_eq!(synth[0].tool.name, "press_menu_item_export");
        assert_eq!(synth[0].node_id, "node-7");
        assert_eq!(synth[0].action, "AXPress");
    }

    #[test]
    fn input_schema_is_an_empty_object_schema() {
        let nodes = vec![node("1", "AXButton", "Go", &["AXPress"], true)];
        let tools = synthesize_tools(&nodes);
        assert_eq!(tools[0].input_schema["type"], "object");
    }
}
