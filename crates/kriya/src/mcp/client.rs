//! The MCP **client** half — the one genuinely new subsystem Front 1 needs. kriya has only
//! ever been an MCP *server* (it exposes an app's actions as governed tools); the stdio
//! governance proxy must also *speak to* a downstream server it spawns as a child.
//!
//! [`McpClient`] spawns the downstream MCP server (`std::process::Command` with piped stdio —
//! the exact pattern already proven in [`super::executor::PersistentProcessExecutor`]), keeps a
//! request-id counter, and exchanges newline-delimited JSON-RPC over the child's stdin/stdout.
//! It reuses the `jsonrpc.rs` types unchanged — the same shapes serve both directions of MCP.
//!
//! The wire transport is split out into a private generic [`Transport`] over `BufRead + Write`
//! so the framing + id-correlation logic is unit-testable with in-memory pipes, no subprocess.
//!
//! std-only by design: no tokio, no async — a synchronous request → response client is all the
//! MVP proxy needs (see `proxy_server.rs`). EOF / broken pipe surfaces as `Err`, never a panic,
//! so a dead downstream degrades into a readable failure rather than taking down the session.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

use super::jsonrpc::{CallToolResult, InitializeResult, ListToolsResult, Tool, PROTOCOL_VERSION};

/// The proxy's *own* downstream request ids live in a disjoint range from the client ids it
/// passes through, so a downstream reply can never be confused with an echoed client id. The
/// MVP is strictly request→response (one outstanding request at a time), so a simple counter
/// from this base is enough; the range keeps the invariant explicit for the full-lifecycle
/// version (two reader threads correlating by id).
const PROXY_ID_BASE: u64 = 1;

/// A downstream MCP server connection plus the JSON-RPC transport over its stdio. Single-threaded
/// MVP: one request out, one response in. Holds the [`Child`] (when spawned) so dropping the client
/// tears the subprocess down with it.
///
/// The transport streams are boxed trait objects so the *same* `McpClient` type drives either a real
/// subprocess (the product path) or in-memory pipes (tests, via [`McpClient::from_streams`]) — the
/// proxy and the binary depend on one concrete type, not a generic.
pub struct McpClient {
    /// `None` for an in-memory test client; `Some` for a spawned downstream (killed on drop).
    child: Option<Child>,
    transport: Transport<Box<dyn BufRead + Send>, Box<dyn Write + Send>>,
}

impl McpClient {
    /// Spawn `program` with `args` as the downstream MCP server. stdin/stdout are piped (the
    /// JSON-RPC channel); stderr is inherited so the downstream's own diagnostics reach the
    /// operator's terminal without polluting the protocol stream — same posture as the executors.
    pub fn spawn(program: &str, args: &[String]) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        // A piped child always yields both handles; treat their absence as a spawn IO error
        // rather than panicking inside the proxy.
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("child stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("child stdout unavailable"))?;
        let reader: Box<dyn BufRead + Send> = Box::new(BufReader::new(stdout));
        let writer: Box<dyn Write + Send> = Box::new(stdin);
        Ok(Self {
            child: Some(child),
            transport: Transport::new(reader, writer),
        })
    }

    /// Build a client over arbitrary streams instead of a subprocess — used by the proxy's tests to
    /// drive the full client/transport with a scripted in-memory downstream (no real process).
    #[cfg(test)]
    pub(crate) fn from_streams<R, W>(reader: R, writer: W) -> Self
    where
        R: std::io::Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let reader: Box<dyn BufRead + Send> = Box::new(BufReader::new(reader));
        let writer: Box<dyn Write + Send> = Box::new(writer);
        Self {
            child: None,
            transport: Transport::new(reader, writer),
        }
    }

    /// MCP handshake: send `initialize`, read the downstream's `InitializeResult`, then send the
    /// `notifications/initialized` notification the downstream expects before it will serve tools.
    pub fn initialize(&mut self) -> Result<InitializeResult, String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            // The proxy is itself an MCP client to the downstream; advertise nothing we don't
            // proxy. Downstream-initiated sampling/elicitation is the full-lifecycle concern.
            "capabilities": {},
            "clientInfo": { "name": "kriya-gateway", "version": env!("CARGO_PKG_VERSION") },
        });
        let result = self.request("initialize", Some(params))?;
        let init: InitializeResult = parse_init(&result)?;
        // A spec-compliant server waits for this notification before considering itself ready.
        self.notify("notifications/initialized", None)?;
        Ok(init)
    }

    /// `tools/list` against the downstream — the dynamic catalog the proxy caches and serves
    /// (policy-filtered) to its own client.
    pub fn list_tools(&mut self) -> Result<Vec<Tool>, String> {
        let result = self.request("tools/list", Some(json!({})))?;
        let list: ListToolsResult = parse_tools(&result)?;
        Ok(list.tools)
    }

    /// Forward a cleared `tools/call` to the downstream and return its `CallToolResult`. This is
    /// the last hop the [`super::proxy_executor::McpProxyExecutor`] makes on a governed call.
    pub fn call_tool(&mut self, name: &str, arguments: &Value) -> Result<CallToolResult, String> {
        let params = json!({ "name": name, "arguments": arguments });
        let result = self.request("tools/call", Some(params))?;
        Ok(parse_call_result(&result))
    }

    /// Generic request: send `method` with `params`, block for the matching response, return its
    /// `result`. Used for transparent passthrough of arbitrary methods (`resources/*`, `prompts/*`,
    /// `ping`, …) the proxy doesn't model. A JSON-RPC error reply becomes an `Err` string.
    pub fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        self.transport.request(method, params)
    }

    /// Fire-and-forget notification (no id, no response) — forwarded verbatim from the client to
    /// the downstream (e.g. `notifications/initialized`, `notifications/cancelled`).
    pub fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        self.transport.notify(method, params)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Best-effort: don't leave a downstream server running after the proxy exits. Dropping
        // stdin already signals EOF to a read-to-end server; kill covers the rest. No-op for an
        // in-memory test client (no child).
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The framing + id-correlation logic, generic over the streams so it is unit-testable with
/// in-memory pipes (`Vec<u8>` writer + `Cursor`/`&[u8]` reader) instead of a real subprocess.
struct Transport<R: BufRead, W: Write> {
    reader: R,
    writer: W,
    /// Monotonic counter for the proxy's own downstream requests (disjoint from client ids).
    next_id: u64,
}

impl<R: BufRead, W: Write> Transport<R, W> {
    fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            next_id: PROXY_ID_BASE,
        }
    }

    /// Allocate the next downstream request id.
    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Send a request, read responses until the one carrying our id arrives, return its `result`.
    /// Responses with a different id (a stray downstream notification echoed as a response, say)
    /// are skipped rather than mistaken for our reply.
    fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.alloc_id();
        let mut msg = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.write_line(&msg.to_string())?;

        loop {
            let line = self.read_line()?;
            if line.trim().is_empty() {
                continue;
            }
            // Parse as a raw JSON value, not the `Response` struct: `jsonrpc::Response` is a
            // server-side (Serialize-only) shape we reuse UNCHANGED, so we read replies by hand.
            // A non-JSON / non-object line (a downstream-initiated request/notification) is skipped —
            // the MVP doesn't service those — and we keep waiting for our reply.
            let msg: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // Match our id exactly; interleaved notifications (no id) and stray ids are skipped.
            if msg.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                return Err(format!("downstream {method} error {code}: {message}"));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Send a notification — no id, no response awaited.
    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut msg = json!({ "jsonrpc": "2.0", "method": method });
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.write_line(&msg.to_string())
    }

    fn write_line(&mut self, line: &str) -> Result<(), String> {
        writeln!(self.writer, "{line}").map_err(|e| format!("write to downstream failed: {e}"))?;
        self.writer
            .flush()
            .map_err(|e| format!("flush to downstream failed: {e}"))
    }

    /// Read one line; a zero-length read is EOF (downstream closed its output / died).
    fn read_line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("read from downstream failed: {e}"))?;
        if n == 0 {
            return Err("downstream closed its output (EOF)".into());
        }
        Ok(line)
    }
}

/// Parse an `InitializeResult` from a raw `result` value (camelCase wire shape).
fn parse_init(result: &Value) -> Result<InitializeResult, String> {
    // `InitializeResult` only derives Serialize (it's a server-side shape), so reconstruct the
    // fields we actually need from the wire JSON rather than deserializing into it.
    let protocol_version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    // We don't reuse the downstream's exact string for protocol_version (the field is &'static);
    // InitializeResult::new pins ours and we override server_info below. capabilities pass through.
    let _ = protocol_version;
    let name = result
        .get("serverInfo")
        .and_then(|s| s.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("downstream")
        .to_string();
    let version = result
        .get("serverInfo")
        .and_then(|s| s.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("0.0.0")
        .to_string();
    let mut init = InitializeResult::new(name, version);
    if let Some(caps) = result.get("capabilities") {
        init.capabilities = caps.clone();
    }
    Ok(init)
}

/// Parse a `ListToolsResult` from a raw `result` value.
fn parse_tools(result: &Value) -> Result<ListToolsResult, String> {
    let arr = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or("downstream tools/list result has no `tools` array")?;
    let mut tools = Vec::with_capacity(arr.len());
    for t in arr {
        let name = t
            .get("name")
            .and_then(Value::as_str)
            .ok_or("a downstream tool entry has no `name`")?
            .to_string();
        let description = t
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let input_schema = t
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object" }));
        tools.push(Tool {
            name,
            description,
            input_schema,
        });
    }
    Ok(ListToolsResult { tools })
}

/// Build a `CallToolResult` from a raw downstream `result` value. `jsonrpc::CallToolResult` is a
/// server-side (Serialize-only) shape we reuse UNCHANGED, so we read the wire fields by hand:
/// `content` (default empty) and `isError` (MCP convention: tool failure is a *result* flag, not a
/// protocol error). A downstream that omits `content` on success still yields a well-formed result.
fn parse_call_result(result: &Value) -> CallToolResult {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    CallToolResult { content, is_error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Drive a `Transport` with a canned downstream reply and assert the request framing + id
    /// correlation — no subprocess. The proxy's first downstream id is `PROXY_ID_BASE` (1).
    #[test]
    fn request_writes_framed_json_and_correlates_the_reply() {
        let downstream = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n";
        let mut sent: Vec<u8> = Vec::new();
        {
            let mut t = Transport::new(Cursor::new(downstream), &mut sent);
            let result = t.request("tools/list", Some(json!({}))).unwrap();
            assert_eq!(result, json!({ "ok": true }));
        }
        // The request we wrote carries jsonrpc, the allocated id (1), method, and params.
        let line = String::from_utf8(sent).unwrap();
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], json!(1));
        assert_eq!(v["method"], "tools/list");
        assert!(v["params"].is_object());
    }

    #[test]
    fn request_ids_are_monotonic_and_disjoint_from_client_ids() {
        // Two replies, ids 1 then 2 — the proxy's own counter advances per request.
        let downstream = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":1}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":2}\n",
        );
        let mut sent: Vec<u8> = Vec::new();
        let mut t = Transport::new(Cursor::new(downstream), &mut sent);
        assert_eq!(t.request("a", None).unwrap(), json!(1));
        assert_eq!(t.request("b", None).unwrap(), json!(2));
    }

    #[test]
    fn request_skips_interleaved_notifications_and_mismatched_ids() {
        // A downstream notification (no id) and a stray response with a different id precede our
        // real reply (id 1). Both must be skipped, not mistaken for the answer.
        let downstream = concat!(
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":999,\"result\":\"stray\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"mine\"}\n",
        );
        let mut sent: Vec<u8> = Vec::new();
        let mut t = Transport::new(Cursor::new(downstream), &mut sent);
        assert_eq!(t.request("tools/call", None).unwrap(), json!("mine"));
    }

    #[test]
    fn request_surfaces_a_downstream_error_as_err() {
        let downstream =
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32601,\"message\":\"nope\"}}\n";
        let mut sent: Vec<u8> = Vec::new();
        let mut t = Transport::new(Cursor::new(downstream), &mut sent);
        let err = t.request("frobnicate", None).unwrap_err();
        assert!(err.contains("nope"), "got: {err}");
    }

    #[test]
    fn request_errors_on_eof_instead_of_panicking() {
        let mut sent: Vec<u8> = Vec::new();
        let mut t = Transport::new(Cursor::new(""), &mut sent); // immediate EOF
        let err = t.request("tools/list", None).unwrap_err();
        assert!(err.contains("EOF"), "got: {err}");
    }

    #[test]
    fn notify_writes_no_id_and_awaits_nothing() {
        let mut sent: Vec<u8> = Vec::new();
        {
            let mut t = Transport::new(Cursor::new(""), &mut sent);
            t.notify("notifications/initialized", None).unwrap();
        }
        let line = String::from_utf8(sent).unwrap();
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["method"], "notifications/initialized");
        assert!(v.get("id").is_none(), "a notification must carry no id");
    }

    #[test]
    fn parse_tools_reads_camel_case_input_schema() {
        let result = json!({
            "tools": [
                { "name": "get_x", "description": "read x", "inputSchema": { "type": "object" } },
                { "name": "delete_x" } // description + schema default
            ]
        });
        let parsed = parse_tools(&result).unwrap();
        assert_eq!(parsed.tools.len(), 2);
        assert_eq!(parsed.tools[0].name, "get_x");
        assert_eq!(parsed.tools[0].input_schema, json!({ "type": "object" }));
        assert_eq!(parsed.tools[1].name, "delete_x");
        assert_eq!(parsed.tools[1].input_schema, json!({ "type": "object" }));
    }

    #[test]
    fn parse_init_keeps_downstream_capabilities_and_server_name() {
        let result = json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": { "listChanged": true }, "resources": {} },
            "serverInfo": { "name": "actual-mcp", "version": "1.2.3" }
        });
        let init = parse_init(&result).unwrap();
        assert_eq!(init.server_info.name, "actual-mcp");
        assert_eq!(init.server_info.version, "1.2.3");
        assert_eq!(init.capabilities["resources"], json!({}));
    }
}
