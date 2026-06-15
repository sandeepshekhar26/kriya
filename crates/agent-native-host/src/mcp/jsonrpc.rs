//! JSON-RPC 2.0 framing and the subset of the Model Context Protocol (MCP) we speak:
//! `initialize`, `tools/list`, and `tools/call`. Kept deliberately small — just enough
//! wire shape for an external agent (Claude Desktop, Cursor) to discover and invoke an
//! app's governed actions. The governance lives one layer up in `governor`; this file is
//! pure (de)serialization with no policy opinions.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The MCP protocol revision this server implements. Echoed back in `initialize` so the
/// client can negotiate. We accept any client version and answer with ours.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// An incoming JSON-RPC message. A request carries an `id`; a notification omits it (and
/// must not be answered). `params` is left as raw JSON for the method handler to interpret.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    #[allow(dead_code)]
    #[serde(default)]
    pub jsonrpc: String,
    /// Absent for notifications. Number or string per the spec — kept as a `Value` so we
    /// echo back exactly what the client sent.
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

impl Request {
    /// A notification has no id and expects no response.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// Standard JSON-RPC error codes plus the ones we actually emit.
pub mod error_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

#[derive(Debug, Clone, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A JSON-RPC response — either a `result` or an `error`, never both. Build with
/// [`Response::success`] / [`Response::error`] so the invariant holds.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn success(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError { code, message: message.into(), data: None }),
        }
    }

    pub fn to_line(&self) -> String {
        // Serialization of a plain struct can't realistically fail; fall back to a minimal
        // hand-built error rather than panicking inside the read loop.
        serde_json::to_string(self).unwrap_or_else(|_| {
            json!({"jsonrpc":"2.0","id":self.id,"error":{"code":error_code::INTERNAL_ERROR,"message":"failed to serialize response"}}).to_string()
        })
    }
}

/// Result of `initialize`. Advertises that this server offers tools.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: &'static str,
    pub capabilities: Value,
    pub server_info: ServerInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

impl InitializeResult {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            // We expose tools and don't mutate the list mid-session.
            capabilities: json!({ "tools": { "listChanged": false } }),
            server_info: ServerInfo { name: name.into(), version: version.into() },
        }
    }
}

/// One entry in a `tools/list` reply. Mirrors the MCP `Tool` shape; `input_schema`
/// serializes to `inputSchema` to match the spec.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
}

/// Params of a `tools/call` request.
#[derive(Debug, Clone, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    /// The tool's arguments object. Defaults to `{}` when the client omits it.
    #[serde(default)]
    pub arguments: Value,
}

/// Result of `tools/call`. MCP convention: tool *execution* failures (including a
/// governance block) are returned as a normal result with `is_error: true`, not as a
/// JSON-RPC protocol error. Protocol errors are reserved for malformed/unknown requests.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    pub content: Vec<Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
}

impl CallToolResult {
    /// A successful tool call carrying a text block (typically the refreshed state JSON).
    pub fn ok(text: impl Into<String>) -> Self {
        Self { content: vec![text_content(text)], is_error: false }
    }

    /// A failed tool call — handler error or a governance block — with an explanation the
    /// calling agent can read and reason about.
    pub fn err(text: impl Into<String>) -> Self {
        Self { content: vec![text_content(text)], is_error: true }
    }
}

fn text_content(text: impl Into<String>) -> Value {
    json!({ "type": "text", "text": text.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_with_numeric_id() {
        let r: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#).unwrap();
        assert_eq!(r.method, "tools/list");
        assert!(!r.is_notification());
        assert_eq!(r.id, Some(json!(1)));
    }

    #[test]
    fn notification_has_no_id() {
        let r: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .unwrap();
        assert!(r.is_notification());
    }

    #[test]
    fn success_and_error_are_mutually_exclusive() {
        let ok = Response::success(json!(1), json!({"ok": true}));
        assert!(ok.result.is_some() && ok.error.is_none());
        let err = Response::error(json!(2), error_code::METHOD_NOT_FOUND, "nope");
        assert!(err.result.is_none() && err.error.is_some());
    }

    #[test]
    fn call_tool_result_omits_is_error_when_false() {
        let line = serde_json::to_string(&CallToolResult::ok("hi")).unwrap();
        assert!(!line.contains("isError"), "clean result should omit isError: {line}");
        let line = serde_json::to_string(&CallToolResult::err("blocked")).unwrap();
        assert!(line.contains("\"isError\":true"), "error result must flag isError: {line}");
    }

    #[test]
    fn tool_serializes_camel_case_input_schema() {
        let t = Tool {
            name: "create_note".into(),
            description: "make a note".into(),
            input_schema: json!({"type":"object"}),
        };
        let line = serde_json::to_string(&t).unwrap();
        assert!(line.contains("inputSchema"), "got: {line}");
    }

    #[test]
    fn initialize_result_advertises_tools() {
        let init = InitializeResult::new("verb-mcp", "0.1.0");
        let v = serde_json::to_value(&init).unwrap();
        assert_eq!(v["protocolVersion"], PROTOCOL_VERSION);
        assert!(v["capabilities"]["tools"].is_object());
        assert_eq!(v["serverInfo"]["name"], "verb-mcp");
    }
}
