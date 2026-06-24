//! `wrap_action` for Rust — the authoring SDK that mirrors `kriya-core`'s `wrapAction`.
//!
//! `kriya-core` (TypeScript) lets a JS app bolt on a governed agent surface by *wrapping* the
//! functions it already has:
//! ```js
//! wrapAction(app.updateTransaction, { id: "update_transaction", parameters: {...}, mapParams });
//! ```
//! The Rust crate was missing the equivalent — a Rust/Tauri app had to hand-write its MCP tool
//! schemas *and* a dispatch `match`. [`Registry`] closes that gap: declare each action once
//! (schema + a closure that calls the method the app already has), and the registry both
//! **generates the MCP tool schemas** ([`Registry::tool_schemas`] / [`Registry::tools_json`]) and
//! **dispatches** calls ([`Registry::dispatch`], and it implements [`ActionExecutor`] so it drops
//! straight into the governor). No second implementation of the app's logic, no hand-written
//! `tools.json`.
//!
//! Handlers take `(&C, &Params)` where `C` is a shared context the app supplies once (e.g.
//! `Arc<Database>`), so wrapping a method is one line and needs no per-closure cloning:
//! ```ignore
//! let mut reg = Registry::new(db);                       // C = Arc<Database>
//! reg.wrap(Action::new("get_containers", "List containers."), |db, _p| json_result(db.get_containers()));
//! reg.wrap(Action::new("delete_container", "Delete a container.").param("id", Param::int()).require_approval(),
//!          |db, p| json_result(db.delete_container(p.i64("id")?)));
//! ```

use std::fmt::Display;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::mcp::{ActionExecutor, ActionOutcome};
use crate::protocol::ToolSchema;

/// JSON-Schema scalar/aggregate type of a parameter (mirrors `kriya-core`'s `ParameterType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
}

impl ParamType {
    fn json_name(self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Integer => "integer",
            ParamType::Number => "number",
            ParamType::Boolean => "boolean",
            ParamType::Array => "array",
            ParamType::Object => "object",
        }
    }
}

/// One parameter the agent supplies. Required by default; call [`Param::optional`] to relax.
#[derive(Debug, Clone)]
pub struct Param {
    ty: ParamType,
    required: bool,
    description: Option<String>,
}

impl Param {
    pub fn string() -> Self {
        Self {
            ty: ParamType::String,
            required: true,
            description: None,
        }
    }
    pub fn int() -> Self {
        Self {
            ty: ParamType::Integer,
            required: true,
            description: None,
        }
    }
    pub fn number() -> Self {
        Self {
            ty: ParamType::Number,
            required: true,
            description: None,
        }
    }
    pub fn boolean() -> Self {
        Self {
            ty: ParamType::Boolean,
            required: true,
            description: None,
        }
    }
    pub fn of(ty: ParamType) -> Self {
        Self {
            ty,
            required: true,
            description: None,
        }
    }
    /// Mark this parameter optional (omitted from the schema's `required[]`).
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }
    pub fn describe(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// The declarative spec for one wrapped action — everything the agent and the host need, minus
/// the handler. Build it fluently: `Action::new(id, desc).param(...).require_approval()`.
#[derive(Debug, Clone)]
pub struct Action {
    id: String,
    description: String,
    version: u32,
    permissions: Vec<String>,
    params: Vec<(String, Param)>,
}

impl Action {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            version: 1,
            permissions: Vec::new(),
            params: Vec::new(),
        }
    }
    /// Add a parameter (ordered as added). Re-adding the same name replaces it.
    pub fn param(mut self, name: impl Into<String>, param: Param) -> Self {
        let name = name.into();
        self.params.retain(|(n, _)| n != &name);
        self.params.push((name, param));
        self
    }
    /// Declare a permission scope the host's policy can gate on.
    pub fn permission(mut self, scope: impl Into<String>) -> Self {
        self.permissions.push(scope.into());
        self
    }
    /// Convenience: tag this action as one whose policy should require human approval. Records
    /// the conventional `requires_approval` scope — the policy still decides; this only labels it.
    pub fn require_approval(self) -> Self {
        self.permission("requires_approval")
    }
    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// The MCP tool schema for this action — `inputSchema` is JSON Schema (draft 2020-12) with an
    /// object-level `required[]`, matching what strict validators (and `kriya-mcp`) expect.
    fn tool_schema(&self) -> ToolSchema {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for (name, p) in &self.params {
            let mut prop = Map::new();
            prop.insert("type".into(), json!(p.ty.json_name()));
            if let Some(d) = &p.description {
                prop.insert("description".into(), json!(d));
            }
            properties.insert(name.clone(), Value::Object(prop));
            if p.required {
                required.push(name.clone());
            }
        }
        let input_schema = json!({
            "type": "object",
            "properties": Value::Object(properties),
            "required": required,
        });
        ToolSchema {
            name: self.id.clone(),
            version: self.version,
            description: self.description.clone(),
            permissions: self.permissions.clone(),
            input_schema,
        }
    }

    fn validate(&self, params: &Value) -> Result<(), String> {
        for (name, p) in &self.params {
            if p.required && params.get(name).map_or(true, Value::is_null) {
                return Err(format!("parameter '{name}' is required"));
            }
        }
        Ok(())
    }
}

type Handler<C> = Box<dyn Fn(&C, &Params) -> Result<Value, String> + Send + Sync>;

struct Entry<C> {
    action: Action,
    handler: Handler<C>,
}

/// A set of wrapped actions over a shared context `C` (e.g. `Arc<Database>`). Generates MCP tool
/// schemas and dispatches calls; implements [`ActionExecutor`] so the governor can drive it.
pub struct Registry<C> {
    ctx: C,
    entries: Vec<Entry<C>>,
}

impl<C> Registry<C> {
    /// Create a registry bound to a shared context handed to every handler.
    pub fn new(ctx: C) -> Self {
        Self {
            ctx,
            entries: Vec::new(),
        }
    }

    /// Wrap an existing function as a governed, agent-callable action. The handler receives the
    /// shared context and the validated [`Params`]; return `Ok(json)` for success (use
    /// [`json_result`] to serialize a method's `Result`) or `Err(msg)` for a readable failure.
    pub fn wrap(
        &mut self,
        action: Action,
        handler: impl Fn(&C, &Params) -> Result<Value, String> + Send + Sync + 'static,
    ) -> &mut Self {
        self.entries.push(Entry {
            action,
            handler: Box::new(handler),
        });
        self
    }

    /// All registered actions as MCP tool schemas — write these to `tools.json`.
    pub fn tool_schemas(&self) -> Vec<ToolSchema> {
        self.entries
            .iter()
            .map(|e| e.action.tool_schema())
            .collect()
    }

    /// `tools.json` content: the tool-schema array as pretty JSON (what `kriya-mcp --tools` loads).
    pub fn tools_json(&self) -> String {
        serde_json::to_string_pretty(&self.tool_schemas())
            .unwrap_or_else(|e| format!("[]  // could not serialize tool schemas: {e}"))
    }

    /// Run one cleared action: validate required params, call its handler, normalize the outcome.
    /// Unknown ids fail cleanly so the agent can only reach the registered surface.
    pub fn dispatch(&self, action_id: &str, params: &Value) -> ActionOutcome {
        let Some(entry) = self.entries.iter().find(|e| e.action.id == action_id) else {
            return ActionOutcome::failed(format!("unknown action '{action_id}'"));
        };
        if let Err(err) = entry.action.validate(params) {
            return ActionOutcome::failed(err);
        }
        match (entry.handler)(&self.ctx, &Params(params)) {
            Ok(data) => ActionOutcome::ok(data),
            Err(err) => ActionOutcome::failed(err),
        }
    }
}

impl<C: Send> ActionExecutor for Registry<C> {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        self.dispatch(action_id, params)
    }
}

/// Serialize a wrapped method's `Result<T, E>` into the `Ok(json)` / `Err(message)` shape
/// [`Registry::wrap`] expects, so a handler body is usually one line:
/// `json_result(db.get_containers())`.
pub fn json_result<T: Serialize, E: Display>(result: Result<T, E>) -> Result<Value, String> {
    match result {
        Ok(value) => {
            serde_json::to_value(value).map_err(|e| format!("could not serialize result: {e}"))
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Thin typed accessors over the agent's `params` object, so handlers extract arguments without
/// hand-rolling `serde_json` calls. Missing/ill-typed required values surface as readable errors.
pub struct Params<'a>(&'a Value);

impl Params<'_> {
    pub fn i64(&self, key: &str) -> Result<i64, String> {
        self.0
            .get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| missing(key, "an integer"))
    }
    pub fn opt_i64(&self, key: &str) -> Option<i64> {
        self.0.get(key).and_then(Value::as_i64)
    }
    pub fn usize(&self, key: &str) -> Result<usize, String> {
        let n = self
            .0
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| missing(key, "a non-negative integer"))?;
        Ok(n as usize)
    }
    pub fn str(&self, key: &str) -> Result<String, String> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| missing(key, "a string"))
    }
    pub fn opt_str(&self, key: &str) -> Option<String> {
        self.0.get(key).and_then(Value::as_str).map(str::to_string)
    }
    pub fn bool(&self, key: &str) -> Result<bool, String> {
        self.0
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| missing(key, "a boolean"))
    }
    pub fn opt_bool(&self, key: &str) -> Option<bool> {
        self.0.get(key).and_then(Value::as_bool)
    }
    /// The raw params object, for handlers that take a whole options struct.
    pub fn raw(&self) -> &Value {
        self.0
    }
}

fn missing(key: &str, expected: &str) -> String {
    format!("parameter '{key}' is required and must be {expected}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    fn sample() -> Registry<Arc<AtomicI64>> {
        let mut reg = Registry::new(Arc::new(AtomicI64::new(0)));
        reg.wrap(Action::new("get_count", "Read the counter."), |c, _p| {
            json_result::<_, String>(Ok(c.load(Ordering::SeqCst)))
        });
        reg.wrap(
            Action::new("add", "Add n to the counter.").param("n", Param::int()),
            |c, p| {
                let n = p.i64("n")?;
                c.fetch_add(n, Ordering::SeqCst);
                json_result::<_, String>(Ok(c.load(Ordering::SeqCst)))
            },
        );
        reg.wrap(
            Action::new("reset", "Reset the counter.").require_approval(),
            |c, _p| {
                c.store(0, Ordering::SeqCst);
                json_result::<_, String>(Ok(0))
            },
        );
        reg
    }

    #[test]
    fn schemas_are_generated_from_specs() {
        let schemas = sample().tool_schemas();
        assert_eq!(schemas.len(), 3);
        let add = schemas.iter().find(|s| s.name == "add").unwrap();
        assert_eq!(add.input_schema["properties"]["n"]["type"], "integer");
        assert_eq!(add.input_schema["required"], json!(["n"]));
        let reset = schemas.iter().find(|s| s.name == "reset").unwrap();
        assert_eq!(reset.permissions, vec!["requires_approval".to_string()]);
    }

    #[test]
    fn dispatch_runs_the_wrapped_handler() {
        let mut reg = sample();
        assert_eq!(reg.dispatch("add", &json!({"n": 5})).data, json!(5));
        assert_eq!(reg.dispatch("add", &json!({"n": 3})).data, json!(8));
        assert_eq!(reg.dispatch("get_count", &json!({})).data, json!(8));
        // ActionExecutor path goes through the same dispatch.
        assert_eq!(
            ActionExecutor::execute(&mut reg, "reset", &json!({})).data,
            json!(0)
        );
    }

    #[test]
    fn unknown_action_and_missing_param_fail_cleanly() {
        let reg = sample();
        let unknown = reg.dispatch("wire_money", &json!({}));
        assert!(!unknown.success);
        assert!(unknown.error.unwrap().contains("unknown action"));

        let missing = reg.dispatch("add", &json!({})); // required `n` absent
        assert!(!missing.success);
        assert!(missing.error.unwrap().contains("'n' is required"));
    }

    #[test]
    fn tools_json_round_trips_to_tool_schemas() {
        let text = sample().tools_json();
        let parsed: Vec<ToolSchema> = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.len(), 3);
        assert!(parsed.iter().any(|s| s.name == "get_count"));
    }
}
