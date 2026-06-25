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
//! - **inputSchema = `{type:object}`** — AX *press*-style actions take no parameters (a press is a
//!   press). The **typed-input** tools added on top carry a real schema: a `set_*` tool takes a
//!   `value` string, the global `type_text` takes a `text` string, the global `press_key` takes a
//!   `key` from a fixed enum. These let an agent fill data (e.g. a spreadsheet cell), not just click.
//!
//! Typed-input tools synthesized in addition to the per-(element, action) `press_*` tools:
//! - **`set_<role>_<title>`** — one per element whose value is *settable* ([`AxNode::settable`]);
//!   routes via the synthetic marker [`ACTION_SET_VALUE`] to [`super::AxBackend::set_value`].
//! - **`type_text`** (global, always present) — types free text into the focused element; marker
//!   [`ACTION_TYPE_TEXT`]. Its `node_id` is empty (it is element-free).
//! - **`press_key`** (global, always present) — sends a named key from [`SUPPORTED_KEYS`]; marker
//!   [`ACTION_PRESS_KEY`]. Also element-free. Together these let the agent select a cell, type a
//!   value, and `Tab`/`Return` to commit + move — all still routed through the unchanged governor.

use std::collections::HashMap;

use serde_json::json;

use crate::mcp::jsonrpc::Tool;

use super::AxNode;

/// Synthetic "action" markers carried on a [`SynthesizedTool`] for the **typed-input** tools. These
/// are NOT real AX action constants (which all start `AX…`); the [`super::executor::AxExecutor`]
/// branches on them to call the right [`super::AxBackend`] typed-input method instead of `perform`.
/// Namespaced under `kriya.` so they can never collide with an AX action name pulled from the tree.
pub const ACTION_SET_VALUE: &str = "kriya.set_value";
/// Marker for the global `type_text` tool — routes to [`super::AxBackend::type_text`].
pub const ACTION_TYPE_TEXT: &str = "kriya.type_text";
/// Marker for the global `press_key` tool — routes to [`super::AxBackend::send_key`].
pub const ACTION_PRESS_KEY: &str = "kriya.press_key";

/// The closed set of named keys `press_key` accepts, in the order they appear in the tool's schema
/// `enum`. Shared as the single source of truth between synthesis (the `enum`), the
/// [`super::FakeBackend`] / executor validation ([`is_known_key`]), and the macOS keycode map — so
/// the advertised keys, the accepted keys, and the keys with a keycode can never drift apart.
pub const SUPPORTED_KEYS: &[&str] = &[
    "return",
    "enter",
    "tab",
    "space",
    "delete",
    "backspace",
    "escape",
    "left",
    "right",
    "down",
    "up",
];

/// Whether `key` is in the [`SUPPORTED_KEYS`] set (case-sensitive — the schema advertises lowercase).
/// The executor and the [`super::FakeBackend`] use this so an unknown key is rejected uniformly,
/// before any OS event is posted.
pub fn is_known_key(key: &str) -> bool {
    SUPPORTED_KEYS.contains(&key)
}

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

        // A settable element (text field, combo box, spreadsheet cell) also gets a `set_*` tool that
        // writes a value directly — the typed-input analogue of a press. Reuses the same name
        // sanitizer/deduper so `set_*` names obey the MCP charset and stay unique alongside `press_*`.
        if node.settable {
            let role = role_word(&node.role);
            let base = sanitized_name("set", &role, &node.title);
            let name = dedupe(&mut seen, base);
            let description = if node.title.is_empty() {
                format!("Set the value of the {} (no title)", node.role)
            } else {
                format!("Set the value of the '{}' {}", node.title, node.role)
            };
            out.push(SynthesizedTool {
                tool: Tool {
                    name,
                    description,
                    input_schema: json!({
                        "type": "object",
                        "properties": { "value": { "type": "string" } },
                        "required": ["value"],
                    }),
                },
                node_id: node.id.clone(),
                action: ACTION_SET_VALUE.to_string(),
            });
        }
    }

    // Two ALWAYS-present, element-free global tools: type free text into the focused element, and
    // press a named key. These are what let an agent type into a selected cell and Tab/Return to
    // commit + navigate — capabilities no single AX element exposes. Deduped through the same `seen`
    // map so a (vanishingly unlikely) app element named `type_text`/`press_key` can't collide.
    out.push(SynthesizedTool {
        tool: Tool {
            name: dedupe(&mut seen, "type_text".to_string()),
            description: "Type text into the focused element (e.g. a selected spreadsheet cell)"
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
            }),
        },
        node_id: String::new(), // element-free: targets whatever holds keyboard focus
        action: ACTION_TYPE_TEXT.to_string(),
    });
    out.push(SynthesizedTool {
        tool: Tool {
            name: dedupe(&mut seen, "press_key".to_string()),
            description:
                "Press a named key or chord (e.g. return/tab/escape/arrows) to commit or navigate"
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": { "key": { "type": "string", "enum": SUPPORTED_KEYS } },
                "required": ["key"],
            }),
        },
        node_id: String::new(),
        action: ACTION_PRESS_KEY.to_string(),
    });

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
            settable: false,
        }
    }

    /// Like [`node`], but value-settable — so synthesis emits a `set_*` tool for it.
    fn settable_node(id: &str, role: &str, title: &str, actions: &[&str]) -> AxNode {
        AxNode {
            settable: true,
            ..node(id, role, title, actions, true)
        }
    }

    /// Only the `press_*`/`set_*` element tools, dropping the two always-appended global tools, so
    /// the per-element assertions below stay focused on what synthesis derived from the snapshot.
    fn element_tool_names(tools: &[Tool]) -> Vec<&str> {
        tools
            .iter()
            .map(|t| t.name.as_str())
            .filter(|n| *n != "type_text" && *n != "press_key")
            .collect()
    }

    #[test]
    fn one_tool_per_enabled_actionable_node() {
        let nodes = vec![
            node("1", "AXButton", "Save", &["AXPress"], true),
            node("2", "AXButton", "Quit", &["AXPress"], true),
        ];
        let tools = synthesize_tools(&nodes);
        assert_eq!(
            element_tool_names(&tools),
            vec!["press_button_save", "press_button_quit"]
        );
    }

    #[test]
    fn disabled_and_actionless_nodes_yield_no_element_tool() {
        let nodes = vec![
            node("1", "AXButton", "Disabled", &["AXPress"], false),
            node("2", "AXStaticText", "label", &[], true), // no actions
        ];
        // No per-element tool — but the two global typed-input tools are always present.
        assert!(element_tool_names(&synthesize_tools(&nodes)).is_empty());
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
        assert_eq!(element_tool_names(&tools).len(), 1);
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
        assert_eq!(
            element_tool_names(&tools),
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
        // The element tool is first (globals are appended after it).
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

    #[test]
    fn the_two_global_typed_input_tools_are_always_present() {
        // Even over an empty snapshot, type_text + press_key are offered (they are element-free).
        let synth = synthesize(&[]);
        let by_name: HashMap<&str, &SynthesizedTool> =
            synth.iter().map(|s| (s.tool.name.as_str(), s)).collect();

        let type_text = by_name.get("type_text").expect("type_text present");
        assert_eq!(type_text.action, ACTION_TYPE_TEXT);
        assert_eq!(type_text.node_id, ""); // element-free
        assert_eq!(type_text.tool.input_schema["required"][0], "text");

        let press_key = by_name.get("press_key").expect("press_key present");
        assert_eq!(press_key.action, ACTION_PRESS_KEY);
        assert_eq!(press_key.node_id, "");
        assert_eq!(press_key.tool.input_schema["required"][0], "key");
        // The enum advertises exactly the supported key set, in order.
        let advertised: Vec<&str> = press_key.tool.input_schema["properties"]["key"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(advertised, SUPPORTED_KEYS);
    }

    #[test]
    fn settable_element_gets_a_set_tool_routed_with_the_marker() {
        let nodes = vec![settable_node("cell-7", "AXTextField", "B2", &["AXConfirm"])];
        let synth = synthesize(&nodes);
        let set = synth
            .iter()
            .find(|s| s.tool.name == "set_text_field_b2")
            .expect("set_* tool synthesized for a settable node");
        assert_eq!(set.action, ACTION_SET_VALUE);
        assert_eq!(set.node_id, "cell-7"); // routes to the element it was synthesized from
        assert_eq!(set.tool.input_schema["required"][0], "value");
        assert_eq!(
            set.tool.input_schema["properties"]["value"]["type"],
            "string"
        );
    }

    #[test]
    fn non_settable_element_gets_no_set_tool() {
        // A plain button is pressable but not value-settable → press_* yes, set_* no.
        let nodes = vec![node("1", "AXButton", "Save", &["AXPress"], true)];
        let tools = synthesize_tools(&nodes);
        let names = element_tool_names(&tools);
        assert!(names.contains(&"press_button_save"), "{names:?}");
        assert!(
            !names.iter().any(|n| n.starts_with("set_")),
            "no set_* for a non-settable element: {names:?}"
        );
    }

    #[test]
    fn set_tool_name_is_sanitized_and_deduped_like_press() {
        // A settable element with a punctuation-heavy title still yields a [a-z0-9_] name; and two
        // settable elements with the same title get a numeric suffix (shared dedupe with press_*).
        let nodes = vec![
            settable_node("1", "AXTextField", "Amount ($)", &[]),
            settable_node("2", "AXTextField", "Amount ($)", &[]),
        ];
        let tools = synthesize_tools(&nodes);
        let names = element_tool_names(&tools);
        assert_eq!(
            names,
            vec!["set_text_field_amount", "set_text_field_amount_2"]
        );
    }

    #[test]
    fn is_known_key_matches_the_supported_set() {
        assert!(is_known_key("tab"));
        assert!(is_known_key("return"));
        assert!(!is_known_key("f13"));
        assert!(!is_known_key("Tab")); // case-sensitive: schema advertises lowercase
    }
}
