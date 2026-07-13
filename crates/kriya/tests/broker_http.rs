//! Remote broker upstream end-to-end (W2-2): the broker governs a REMOTE MCP server over the
//! Streamable-HTTP transport (`McpClient::connect_http`), not a spawned process. A minimal std-only
//! HTTP MCP server (one `TcpListener`, no extra deps) stands in for a hosted server; the broker
//! connects to it, and the same composition as the stdio path (proxy executor → Front →
//! RouterServer under `default_broker_policy`) routes + signs every call. Also proves the MCP
//! session id is captured on `initialize` and echoed on later requests (the server rejects a
//! tools/call that lacks it).
#![cfg(feature = "mcp-http")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use kriya::audit::{Actor, Signer};
use kriya::mcp::{AutoApprove, Front, McpClient, McpProxyExecutor, RouterServer};
use kriya::permissions::default_broker_policy;
use serde_json::{json, Value};

const SESSION_ID: &str = "sess-w2-2";

/// Serve exactly `n_requests` MCP HTTP requests on a fresh ephemeral port, then stop. Returns the
/// bound `http://127.0.0.1:PORT/` URL and the join handle. Each request is one connection
/// (`Connection: close`), which keeps the parser trivial and matches how ureq drives it here.
fn spawn_http_mcp_server(n_requests: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for _ in 0..n_requests {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().unwrap());

            // Parse request line + headers (we only need Content-Length + Mcp-Session-Id).
            let mut content_length = 0usize;
            let mut session_hdr: Option<String> = None;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap(); // request line, ignored
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                let lower = h.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                } else if lower.starts_with("mcp-session-id:") {
                    session_hdr = Some(h.splitn(2, ':').nth(1).unwrap().trim().to_string());
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let id = req.get("id").cloned();
            let method = req.get("method").and_then(Value::as_str).unwrap_or("");

            // A notification (no id) — e.g. notifications/initialized — gets 202 with no body.
            if id.is_none() {
                write_http(&mut stream, "202 Accepted", None, None);
                continue;
            }
            let id = id.unwrap();

            let (result, set_session): (Value, bool) = match method {
                "initialize" => (
                    json!({
                        "protocolVersion": "2025-06-18",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "remote-widgets", "version": "9.9.9" }
                    }),
                    true, // assign the session id on initialize
                ),
                "tools/list" => (
                    json!({ "tools": [
                        { "name": "list_widgets", "description": "read", "inputSchema": {"type":"object"} },
                        { "name": "delete_widget", "description": "destroy", "inputSchema": {"type":"object"} },
                    ]}),
                    false,
                ),
                "tools/call" => {
                    // Prove the client captured + echoed the session id: reject if absent/wrong.
                    if session_hdr.as_deref() != Some(SESSION_ID) {
                        let err = json!({
                            "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32000, "message": "missing/invalid Mcp-Session-Id" }
                        });
                        write_http(&mut stream, "200 OK", Some(&err.to_string()), None);
                        continue;
                    }
                    let tool = req["params"]["name"].as_str().unwrap_or("");
                    (
                        json!({ "content": [{ "type": "text", "text": format!("ran {tool}") }], "isError": false }),
                        false,
                    )
                }
                _ => (Value::Null, false),
            };

            let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            let session = if set_session { Some(SESSION_ID) } else { None };
            write_http(&mut stream, "200 OK", Some(&resp.to_string()), session);
        }
    });
    (url, handle)
}

fn write_http(
    stream: &mut std::net::TcpStream,
    status: &str,
    body: Option<&str>,
    session: Option<&str>,
) {
    let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    if let Some(s) = session {
        head.push_str(&format!("Mcp-Session-Id: {s}\r\n"));
    }
    match body {
        Some(b) => {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                b.len()
            ));
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(b.as_bytes());
        }
        None => {
            head.push_str("Content-Length: 0\r\n\r\n");
            let _ = stream.write_all(head.as_bytes());
        }
    }
    let _ = stream.flush();
}

fn call(server: &mut RouterServer, name: &str, arguments: Value) -> Value {
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let mut out = Vec::new();
    server
        .serve(std::io::Cursor::new(format!("{req}\n")), &mut out)
        .expect("serve one call");
    serde_json::from_slice(&out).expect("one JSON-RPC response line")
}

#[test]
fn broker_governs_a_remote_http_upstream_and_signs_it() {
    // Requests the broker makes to the remote server, in order: initialize, notifications/initialized,
    // tools/list, tools/call (read), tools/call (delete) = 5. The denied create never leaves the
    // broker, so it is NOT one of them.
    let (url, handle) = spawn_http_mcp_server(5);

    let namespaces = vec!["widgets".to_string()];
    let policy = Arc::new(default_broker_policy(&namespaces));
    let dir = std::env::temp_dir().join(format!("kriya-broker-http-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("broker.jsonl");
    let signer = Arc::new(Signer::with_log_path(log.clone()));

    // Connect the REMOTE upstream over HTTP and wrap it exactly as the broker binary does.
    let client = Arc::new(Mutex::new(
        McpClient::connect_http(&url, vec![], false, None).expect("connect http upstream"),
    ));
    let tools = {
        let mut g = client.lock().unwrap();
        g.initialize().expect("initialize remote"); // captures the session id
        g.list_tools().expect("list remote tools")
    };
    assert_eq!(tools.len(), 2, "remote tools cached");
    let front = Front::new("widgets", tools, Box::new(McpProxyExecutor::new(client)));

    let mut server = RouterServer::from_parts(
        "kriya-broker-http-test",
        vec![front],
        policy,
        signer,
        Box::new(AutoApprove),
        Some(Actor::new("claude-desktop", "tester")),
    );

    // Read is allowed and routed to the remote server.
    let read = call(&mut server, "widgets__list_widgets", json!({}));
    assert!(
        read["result"].get("isError").is_none(),
        "allowed remote read: {read}"
    );
    assert!(read["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("ran list_widgets"));

    // Destructive is approval-gated (AutoApprove clears it), routed to the remote, and the remote
    // ONLY answered because the session id captured on initialize was echoed on this call.
    let del = call(&mut server, "widgets__delete_widget", json!({ "id": "w1" }));
    assert!(
        del["result"].get("isError").is_none(),
        "approved remote delete (session echoed): {del}"
    );
    assert!(del["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("ran delete_widget"));

    // The one broker chain holds exactly the two executed remote calls, both verifying.
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "two executed remote calls signed: {text}");
    for l in &lines {
        let v: Value = serde_json::from_str(l).unwrap();
        assert!(v["action_id"].as_str().unwrap().starts_with("widgets__"));
    }

    handle.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
