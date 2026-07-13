//! EG-2 end-to-end: the governed-lane I/O ledger over the REAL broker stack (doc 24 §7.3 / §4).
//!
//! Built on the `broker_http.rs` template — a minimal std-only HTTP MCP server stands in for a
//! hosted upstream, and the same composition the broker binary uses (`McpProxyExecutor` → `Front` →
//! `RouterServer`, now with an `EgressControl`) routes every governed `tools/call`. This proves the
//! full path: `HttpTransport` captures the destination + observed WIRE bytes + content hash, the
//! governor decides the egress tier, and each decision produces the right `kriya.io.egress.mcp.*`
//! receipt — `allow` (with wire-bytes), `deny` (at the decision point, executor never reached),
//! `approve` (rides the ApprovalGate). Every receipt is Ed25519-verified and the chain is checked,
//! exactly as `kriya-verify` / the released `kriya-audit` CLI do.
#![cfg(feature = "mcp-http")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use kriya::audit::{Actor, Signer};
use kriya::mcp::{
    AutoApprove, DenyApproval, EgressControl, EgressTarget, Front, IoKind, McpClient,
    McpProxyExecutor, RouterServer,
};
use kriya::permissions::{url_host, EgressPolicy};
use serde_json::{json, Value};

/// Serve exactly `n_requests` MCP HTTP requests on a fresh ephemeral port, then stop. One connection
/// per request (`Connection: close`). Adapted verbatim from `broker_http.rs`.
fn spawn_http_mcp_server(n_requests: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for _ in 0..n_requests {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
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
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let id = req.get("id").cloned();
            let method = req.get("method").and_then(Value::as_str).unwrap_or("");
            if id.is_none() {
                write_http(&mut stream, "202 Accepted", None);
                continue;
            }
            let id = id.unwrap();
            let result = match method {
                "initialize" => json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "remote-widgets", "version": "9.9.9" }
                }),
                "tools/list" => json!({ "tools": [
                    { "name": "list_widgets", "description": "read", "inputSchema": {"type":"object"} },
                ]}),
                "tools/call" => {
                    let tool = req["params"]["name"].as_str().unwrap_or("");
                    json!({ "content": [{ "type": "text", "text": format!("ran {tool}") }], "isError": false })
                }
                _ => Value::Null,
            };
            let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            write_http(&mut stream, "200 OK", Some(&resp.to_string()));
        }
    });
    (url, handle)
}

/// Like [`spawn_http_mcp_server`] but replies to id-bearing methods with an SSE stream (a progress
/// notification event then the result event), so the client exercises the SSE byte-counting +
/// `bytes_in_is_partial` flag (doc 24 L2).
fn spawn_http_mcp_server_sse(n_requests: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for _ in 0..n_requests {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let id = req.get("id").cloned();
            let method = req.get("method").and_then(Value::as_str).unwrap_or("");
            if id.is_none() {
                write_http(&mut stream, "202 Accepted", None);
                continue;
            }
            let id = id.unwrap();
            let result = match method {
                "initialize" => json!({
                    "protocolVersion": "2025-06-18", "capabilities": { "tools": {} },
                    "serverInfo": { "name": "remote-sse", "version": "9.9.9" }
                }),
                "tools/list" => json!({ "tools": [
                    { "name": "list_widgets", "description": "read", "inputSchema": {"type":"object"} },
                ]}),
                "tools/call" => {
                    json!({ "content": [{ "type": "text", "text": "ran via sse" }], "isError": false })
                }
                _ => Value::Null,
            };
            let progress =
                json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {} });
            let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            write_sse(&mut stream, &[progress.to_string(), reply.to_string()]);
        }
    });
    (url, handle)
}

fn write_sse(stream: &mut std::net::TcpStream, events: &[String]) {
    let mut body = String::new();
    for e in events {
        body.push_str(&format!("data: {e}\n\n"));
    }
    let head = format!(
        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

fn write_http(stream: &mut std::net::TcpStream, status: &str, body: Option<&str>) {
    let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
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

/// Connect + handshake the upstream, then build a `RouterServer` governing it under `egress_yaml`.
/// The resolver maps `remote__<tool>` to the upstream's host (mirroring the broker binary).
fn broker_with_egress(
    url: &str,
    egress_yaml: &str,
    approval_auto: bool,
    log: std::path::PathBuf,
) -> RouterServer {
    let client = Arc::new(Mutex::new(
        McpClient::connect_http(url, vec![], false, None).expect("connect http upstream"),
    ));
    let tools = {
        let mut g = client.lock().unwrap();
        g.initialize().expect("initialize remote");
        g.list_tools().expect("list remote tools")
    };
    let front = Front::new("remote", tools, Box::new(McpProxyExecutor::new(client)));
    let signer = Arc::new(Signer::with_log_path(log));
    let policy = Arc::new(
        serde_yaml::from_str::<kriya::permissions::Policy>(
            "rules:\n  - action: \"*\"\n    allow: true\nbudget:\n  max_actions_per_minute: 1000\n",
        )
        .unwrap(),
    );
    let host = url_host(url);
    let ep: EgressPolicy = serde_yaml::from_str(egress_yaml).unwrap();
    let control = EgressControl::new(ep, move |action_id: &str, _p: &Value| {
        let ns = action_id
            .split_once("__")
            .map(|(n, _)| n)
            .unwrap_or(action_id);
        (ns == "remote").then(|| EgressTarget {
            host: host.clone(),
            kind: IoKind::Mcp,
            server: Some("remote".into()),
        })
    });
    let approval: Box<dyn kriya::mcp::ApprovalGate> = if approval_auto {
        Box::new(AutoApprove)
    } else {
        Box::new(DenyApproval)
    };
    RouterServer::from_parts_with_egress(
        "kriya-egress-test",
        vec![front],
        policy,
        signer,
        approval,
        Some(Actor::new("claude-desktop", "tester")),
        Some(control),
    )
}

fn call(server: &mut RouterServer, name: &str) -> Value {
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": {} },
    });
    let mut out = Vec::new();
    server
        .serve(std::io::Cursor::new(format!("{req}\n")), &mut out)
        .expect("serve one call");
    serde_json::from_slice(&out).expect("one JSON-RPC response line")
}

/// Verify every receipt line's Ed25519 signature and its `prev_hash` chain — the exact checks
/// `kriya-verify` / `kriya-audit` perform (re-derive the canonical bytes, verify, chain by prev line).
fn verify_log(log: &std::path::Path) -> Vec<Value> {
    let text = std::fs::read_to_string(log).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut prev: Option<String> = None;
    let mut parsed = Vec::new();
    for line in &lines {
        let v: Value = serde_json::from_str(line).unwrap();
        // The signed message is the `Receipt` struct serialized in DECLARATION order (step_id,
        // action_id, params, success, ts_ms, actor?, prev_hash?) with `params` key-sorted — exactly
        // what audit.rs signs. Rebuild that byte-for-byte.
        let msg = signed_bytes(&v);
        let pk: [u8; 32] = hex::decode(v["public_key"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let sig: [u8; 64] = hex::decode(v["signature"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let key = VerifyingKey::from_bytes(&pk).unwrap();
        assert!(
            key.verify(&msg, &Signature::from_bytes(&sig)).is_ok(),
            "signature must verify for {}",
            v["action_id"]
        );
        // Chain: prev_hash == SHA-256 of the previous line.
        let declared = v
            .get("prev_hash")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(declared, prev, "chain must be contiguous");
        prev = Some(sha256_hex(line.as_bytes()));
        parsed.push(v);
    }
    parsed
}

/// Reproduce audit.rs's signed bytes: the `Receipt` fields in declaration order, `params` (and
/// `actor`) key-sorted, optional fields omitted when absent. Independent of serde_json's
/// `preserve_order` feature, so it matches whatever the signer produced.
fn signed_bytes(v: &Value) -> Vec<u8> {
    let mut s = String::from("{");
    s.push_str(&format!("\"step_id\":{}", v["step_id"]));
    s.push_str(&format!(",\"action_id\":{}", v["action_id"]));
    s.push_str(&format!(
        ",\"params\":{}",
        serde_json::to_string(&canonical(&v["params"])).unwrap()
    ));
    s.push_str(&format!(",\"success\":{}", v["success"]));
    s.push_str(&format!(",\"ts_ms\":{}", v["ts_ms"]));
    if let Some(a) = v.get("actor") {
        if !a.is_null() {
            s.push_str(&format!(
                ",\"actor\":{}",
                serde_json::to_string(&canonical(a)).unwrap()
            ));
        }
    }
    if let Some(ph) = v.get("prev_hash") {
        if !ph.is_null() {
            s.push_str(&format!(",\"prev_hash\":{ph}"));
        }
    }
    s.push('}');
    s.into_bytes()
}

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonical(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn io_receipts(receipts: &[Value]) -> Vec<&Value> {
    receipts
        .iter()
        .filter(|v| v["action_id"].as_str().unwrap().starts_with("kriya.io."))
        .collect()
}

#[test]
fn egress_allow_emits_a_wire_bytes_io_receipt_over_the_real_transport() {
    // init, notifications/initialized, tools/list, tools/call = 4 upstream requests.
    let (url, handle) = spawn_http_mcp_server(4);
    let dir = std::env::temp_dir().join(format!("kriya-egress-allow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("broker.jsonl");
    let mut server = broker_with_egress(
        &url,
        "rules:\n  - host: \"127.0.0.1\"\n    tier: allow\nunlisted: deny\n",
        false,
        log.clone(),
    );

    let resp = call(&mut server, "remote__list_widgets");
    assert!(resp["result"].get("isError").is_none(), "allowed: {resp}");

    let receipts = verify_log(&log);
    let io = io_receipts(&receipts);
    assert_eq!(io.len(), 1, "one io receipt for the allowed call: {io:?}");
    let r = io[0];
    assert_eq!(r["action_id"], "kriya.io.egress.mcp.allow");
    assert_eq!(r["params"]["dest_host"], "127.0.0.1");
    assert_eq!(r["params"]["policy_rule"], "127.0.0.1");
    // The transport captured WIRE bytes (not the governor's canonical-json fallback).
    assert_eq!(r["params"]["hash_scheme"], "wire-bytes");
    assert!(
        r["params"]["bytes_out"].as_u64().unwrap() > 0,
        "real outbound bytes"
    );
    assert!(
        r["params"]["bytes_in"].as_u64().unwrap() > 0,
        "real inbound bytes"
    );
    assert!(r["params"]["content_sha256"].as_str().unwrap().len() == 64);
    // corr joins to the action receipt.
    let action = receipts
        .iter()
        .find(|v| v["action_id"] == "remote__list_widgets")
        .unwrap();
    assert_eq!(r["params"]["corr"], action["step_id"]);

    handle.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn egress_deny_never_reaches_the_upstream_and_emits_a_deny_receipt() {
    // Only the handshake (init, notify, list = 3) reaches the server; the denied call does NOT.
    let (url, handle) = spawn_http_mcp_server(3);
    let dir = std::env::temp_dir().join(format!("kriya-egress-deny-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("broker.jsonl");
    // Deny the upstream's host outright.
    let mut server = broker_with_egress(
        &url,
        "unlisted: deny\nrules:\n  - host: \"127.0.0.1\"\n    tier: deny\n",
        true,
        log.clone(),
    );

    let resp = call(&mut server, "remote__list_widgets");
    assert_eq!(
        resp["result"]["isError"], true,
        "denied call is an error result: {resp}"
    );

    let receipts = verify_log(&log);
    let io = io_receipts(&receipts);
    assert_eq!(io.len(), 1);
    assert_eq!(io[0]["action_id"], "kriya.io.egress.mcp.deny");
    assert_eq!(io[0]["params"]["decision"], "deny");
    // No action receipt for the blocked call — it never executed.
    assert!(receipts
        .iter()
        .all(|v| v["action_id"] != "remote__list_widgets"));

    handle.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn egress_allow_over_sse_flags_bytes_in_partial() {
    // The upstream replies over SSE, so bytes_in is a partial lower bound (L2) — the emitted receipt
    // must carry that flag, while bytes_out + content_sha256 stay wire-bytes.
    let (url, handle) = spawn_http_mcp_server_sse(4);
    let dir = std::env::temp_dir().join(format!("kriya-egress-sse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("broker.jsonl");
    let mut server = broker_with_egress(
        &url,
        "rules:\n  - host: \"127.0.0.1\"\n    tier: allow\nunlisted: deny\n",
        false,
        log.clone(),
    );

    let resp = call(&mut server, "remote__list_widgets");
    assert!(
        resp["result"].get("isError").is_none(),
        "allowed over sse: {resp}"
    );

    let receipts = verify_log(&log);
    let io = io_receipts(&receipts);
    assert_eq!(io.len(), 1);
    assert_eq!(io[0]["action_id"], "kriya.io.egress.mcp.allow");
    assert_eq!(
        io[0]["params"]["bytes_in_is_partial"], true,
        "SSE bytes_in is a partial lower bound (L2)"
    );
    assert_eq!(io[0]["params"]["hash_scheme"], "wire-bytes");
    assert!(io[0]["params"]["bytes_in"].as_u64().unwrap() > 0);

    handle.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn egress_approval_tier_produces_an_approve_receipt() {
    let (url, handle) = spawn_http_mcp_server(4);
    let dir = std::env::temp_dir().join(format!("kriya-egress-approve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("broker.jsonl");
    let mut server = broker_with_egress(
        &url,
        "unlisted: deny\nrules:\n  - host: \"127.0.0.1\"\n    tier: approval\n",
        true, // AutoApprove clears the egress approval
        log.clone(),
    );

    let resp = call(&mut server, "remote__list_widgets");
    assert!(resp["result"].get("isError").is_none(), "approved: {resp}");

    let receipts = verify_log(&log);
    let io = io_receipts(&receipts);
    assert_eq!(io.len(), 1);
    assert_eq!(io[0]["action_id"], "kriya.io.egress.mcp.approve");
    assert_eq!(io[0]["params"]["approved_by"], "tester");

    handle.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
