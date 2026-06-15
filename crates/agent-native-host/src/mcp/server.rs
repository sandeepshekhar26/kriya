//! The stdio MCP server: a line-delimited JSON-RPC loop that answers `initialize`,
//! `tools/list`, and `tools/call`, routing every call through the [`Governor`]. The loop is
//! transport-thin on purpose — all the judgment lives in the governor. stdout is the
//! JSON-RPC channel; everything human-readable (startup banner, per-call governance
//! decisions) goes to stderr so it never corrupts the protocol stream.

use std::collections::HashSet;
use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::protocol::ToolSchema;

use super::governor::{DispatchOutcome, Governor};
use super::jsonrpc::{
    error_code, CallToolParams, InitializeResult, ListToolsResult, Request, Response, Tool,
};

/// Owns the tool catalog + the governor and turns JSON-RPC requests into responses.
pub struct Server {
    name: String,
    version: String,
    tools: Vec<Tool>,
    tool_names: HashSet<String>,
    governor: Governor,
}

impl Server {
    /// Build from the app's `getToolSchemas()` output and a configured governor.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        schemas: Vec<ToolSchema>,
        governor: Governor,
    ) -> Self {
        let tools: Vec<Tool> = schemas
            .into_iter()
            .map(|s| Tool {
                name: s.name,
                description: s.description,
                input_schema: s.input_schema,
            })
            .collect();
        let tool_names = tools.iter().map(|t| t.name.clone()).collect();
        Self { name: name.into(), version: version.into(), tools, tool_names, governor }
    }

    /// Number of exposed tools — used by the binary for its startup banner.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Read JSON-RPC lines from `reader`, write responses to `writer`. Blocks until EOF
    /// (the client closed stdin). One JSON object per line, NDJSON-style.
    pub fn serve<R: BufRead, W: Write>(&mut self, reader: R, writer: &mut W) -> std::io::Result<()> {
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => self.handle(req),
                // Malformed JSON: reply with a parse error and a null id, per JSON-RPC.
                Err(e) => Some(Response::error(
                    Value::Null,
                    error_code::PARSE_ERROR,
                    format!("parse error: {e}"),
                )),
            };
            if let Some(resp) = response {
                writeln!(writer, "{}", resp.to_line())?;
                writer.flush()?;
            }
        }
        Ok(())
    }

    /// Dispatch one parsed request. Returns `None` for notifications (which must not be
    /// answered). Pure apart from the governor's side effects — directly unit-testable.
    pub fn handle(&mut self, req: Request) -> Option<Response> {
        if req.is_notification() {
            // We only need `notifications/initialized` today, and it requires no action.
            return None;
        }
        // Safe: a non-notification has an id.
        let id = req.id.clone().unwrap_or(Value::Null);

        let resp = match req.method.as_str() {
            "initialize" => {
                Response::success(id, ok_value(InitializeResult::new(&self.name, &self.version)))
            }
            "tools/list" => {
                Response::success(id, ok_value(ListToolsResult { tools: self.tools.clone() }))
            }
            "tools/call" => self.handle_call(id, req.params),
            // MCP health check.
            "ping" => Response::success(id, json!({})),
            other => {
                Response::error(id, error_code::METHOD_NOT_FOUND, format!("unknown method: {other}"))
            }
        };
        Some(resp)
    }

    fn handle_call(&mut self, id: Value, params: Option<Value>) -> Response {
        let Some(params) = params else {
            return Response::error(id, error_code::INVALID_PARAMS, "tools/call requires params");
        };
        let call: CallToolParams = match serde_json::from_value(params) {
            Ok(c) => c,
            Err(e) => {
                return Response::error(id, error_code::INVALID_PARAMS, format!("bad tools/call params: {e}"))
            }
        };

        // Unknown tool name → an error *result* (the call was well-formed), not a protocol error.
        if !self.tool_names.contains(&call.name) {
            return Response::success(
                id,
                ok_value(super::jsonrpc::CallToolResult::err(format!(
                    "unknown tool: {}",
                    call.name
                ))),
            );
        }

        // The one line that matters: route through every governance gate.
        let outcome = self.governor.dispatch(&call.name, &call.arguments);
        log_outcome(&call.name, &outcome);

        let result = match outcome {
            DispatchOutcome::Denied => super::jsonrpc::CallToolResult::err(format!(
                "blocked by policy: '{}' is denied (deny-by-default)",
                call.name
            )),
            DispatchOutcome::NotApproved => super::jsonrpc::CallToolResult::err(format!(
                "blocked: '{}' requires human approval and it was not granted",
                call.name
            )),
            DispatchOutcome::BudgetExceeded(reason) => {
                super::jsonrpc::CallToolResult::err(format!("blocked: {reason}"))
            }
            DispatchOutcome::Executed { outcome, .. } => {
                if outcome.success {
                    super::jsonrpc::CallToolResult::ok(
                        serde_json::to_string(&outcome.data).unwrap_or_else(|_| "null".into()),
                    )
                } else {
                    super::jsonrpc::CallToolResult::err(
                        outcome.error.unwrap_or_else(|| "action failed".into()),
                    )
                }
            }
        };
        Response::success(id, ok_value(result))
    }
}

/// Serialize a response body that is statically known to serialize cleanly.
fn ok_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// One stderr line per call so the operator watching the terminal sees governance happen.
fn log_outcome(action_id: &str, outcome: &DispatchOutcome) {
    let note = match outcome {
        DispatchOutcome::Denied => "DENIED by policy".to_string(),
        DispatchOutcome::NotApproved => "BLOCKED — approval not granted".to_string(),
        DispatchOutcome::BudgetExceeded(r) => format!("BLOCKED — {r}"),
        DispatchOutcome::Executed { outcome, receipt } => format!(
            "ran ({}) · receipt sig={}…",
            if outcome.success { "ok" } else { "failed" },
            &receipt.signature[..receipt.signature.len().min(16)]
        ),
    };
    eprintln!("[verb-mcp] tools/call {action_id}: {note}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Signer;
    use crate::mcp::approval::{AutoApprove, DenyApproval};
    use crate::mcp::executor::FnExecutor;
    use crate::mcp::executor::ActionOutcome;
    use crate::permissions::Policy;
    use std::sync::Arc;

    fn schemas() -> Vec<ToolSchema> {
        serde_json::from_value(json!([
            {"name":"create_note","description":"make a note","inputSchema":{"type":"object"}},
            {"name":"delete_note","description":"remove a note","inputSchema":{"type":"object"}}
        ]))
        .unwrap()
    }

    fn server(approval: Box<dyn crate::mcp::approval::ApprovalGate>) -> Server {
        let governor = Governor::new(
            Arc::new(Policy::default()),
            Arc::new(Signer::new()),
            approval,
            Box::new(FnExecutor(|_id: &str, _p: &Value| ActionOutcome::ok(json!({"ok": true})))),
        );
        Server::new("verb-mcp", "0.1.0", schemas(), governor)
    }

    fn req(method: &str, params: Value) -> Request {
        serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .unwrap()
    }

    #[test]
    fn initialize_returns_server_info() {
        let mut s = server(Box::new(DenyApproval));
        let resp = s.handle(req("initialize", json!({}))).unwrap();
        let r = resp.result.unwrap();
        assert_eq!(r["serverInfo"]["name"], "verb-mcp");
        assert!(r["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_exposes_registered_actions() {
        let mut s = server(Box::new(DenyApproval));
        let resp = s.handle(req("tools/list", json!({}))).unwrap();
        let tools = resp.result.unwrap()["tools"].clone();
        assert_eq!(tools.as_array().unwrap().len(), 2);
        assert_eq!(tools[0]["name"], "create_note");
        assert!(tools[0]["inputSchema"].is_object());
    }

    #[test]
    fn notification_gets_no_response() {
        let mut s = server(Box::new(DenyApproval));
        let note: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap();
        assert!(s.handle(note).is_none());
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let mut s = server(Box::new(DenyApproval));
        let resp = s.handle(req("frobnicate", json!({}))).unwrap();
        assert_eq!(resp.error.unwrap().code, error_code::METHOD_NOT_FOUND);
    }

    #[test]
    fn tools_call_allowed_action_returns_state() {
        let mut s = server(Box::new(DenyApproval));
        let resp = s
            .handle(req("tools/call", json!({"name":"create_note","arguments":{"title":"hi"}})))
            .unwrap();
        let result = resp.result.unwrap();
        // success result: isError omitted, content carries the handler data.
        assert!(result.get("isError").is_none());
        assert!(result["content"][0]["text"].as_str().unwrap().contains("\"ok\":true"));
    }

    #[test]
    fn tools_call_guarded_action_is_blocked_as_error_result() {
        let mut s = server(Box::new(DenyApproval));
        let resp = s
            .handle(req("tools/call", json!({"name":"delete_note","arguments":{"id":1}})))
            .unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("approval"));
    }

    #[test]
    fn tools_call_guarded_action_runs_when_approved() {
        let mut s = server(Box::new(AutoApprove));
        let resp = s
            .handle(req("tools/call", json!({"name":"delete_note","arguments":{"id":1}})))
            .unwrap();
        assert!(resp.result.unwrap().get("isError").is_none());
    }

    #[test]
    fn tools_call_unknown_tool_is_error_result_not_protocol_error() {
        let mut s = server(Box::new(DenyApproval));
        let resp = s
            .handle(req("tools/call", json!({"name":"nonexistent","arguments":{}})))
            .unwrap();
        assert!(resp.error.is_none(), "well-formed call → result, not protocol error");
        assert_eq!(resp.result.unwrap()["isError"], true);
    }

    #[test]
    fn serve_processes_a_stream_and_skips_notifications() {
        let mut s = server(Box::new(DenyApproval));
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n"
        );
        let mut out: Vec<u8> = Vec::new();
        s.serve(input.as_bytes(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // initialize + tools/list answered; the notification produced no line.
        assert_eq!(lines.len(), 2, "got: {text}");
        assert!(lines[0].contains("serverInfo"));
        assert!(lines[1].contains("create_note"));
    }
}
