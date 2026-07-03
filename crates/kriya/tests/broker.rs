//! Broker end-to-end (W2-1): two real MCP upstreams multiplexed under ONE governor, served as
//! `<namespace>__<tool>`, every call landing in one signed `broker.jsonl` chain. Exercises the exact
//! composition `kriya-gateway broker` assembles — `McpClient::spawn` → `McpProxyExecutor` → `Front`
//! → `RouterServer` under a per-namespace `default_broker_policy` — against the zero-dep mock MCP
//! server. Skips (does not fail) when `node` is unavailable, so it never blocks a Rust-only CI box.
#![cfg(feature = "mcp-client")]

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use kriya::audit::{Signer, Actor};
use kriya::mcp::{Front, McpClient, McpProxyExecutor, RouterServer, AutoApprove};
use kriya::permissions::default_broker_policy;
use serde_json::Value;

fn mock_server_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/gateway-proxy-demo/mock-mcp-server.js")
}

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Spawn one mock-notes upstream and wrap it as a broker Front under `namespace`.
fn upstream_front(namespace: &str) -> Front {
    let client = Arc::new(Mutex::new(
        McpClient::spawn("node", &[mock_server_path().to_string_lossy().into_owned()])
            .expect("spawn mock server"),
    ));
    let tools = {
        let mut g = client.lock().unwrap();
        g.initialize().expect("initialize upstream");
        g.list_tools().expect("list upstream tools")
    };
    Front::new(namespace, tools, Box::new(McpProxyExecutor::new(client)))
}

fn call(server: &mut RouterServer, name: &str, arguments: Value) -> Value {
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let mut out = Vec::new();
    server
        .serve(Cursor::new(format!("{req}\n")), &mut out)
        .expect("serve one call");
    serde_json::from_slice(&out).expect("one JSON-RPC response line")
}

#[test]
fn broker_multiplexes_two_upstreams_into_one_signed_chain() {
    if !node_available() {
        eprintln!("skipping broker e2e: node not available");
        return;
    }

    let dir = std::env::temp_dir().join(format!("kriya-broker-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("broker.jsonl");

    // Two upstreams, distinct namespaces — the "one endpoint, N servers" shape.
    let namespaces = vec!["notes_a".to_string(), "notes_b".to_string()];
    let policy = Arc::new(default_broker_policy(&namespaces));
    let signer = Arc::new(Signer::with_log_path(log.clone()));
    let fronts = vec![upstream_front("notes_a"), upstream_front("notes_b")];

    let mut server = RouterServer::from_parts(
        "kriya-broker-test",
        fronts,
        policy,
        signer,
        Box::new(AutoApprove), // so the approval-gated delete runs and is recorded
        Some(Actor::new("claude-desktop", "tester")),
    );

    // tools/list is the namespaced union of BOTH upstreams, **policy-filtered**: each mock server
    // has 4 tools (list_notes, get_note, create_note, delete_note), but the per-namespace default
    // policy DENIES the non-read, non-destructive `create_note`, so it never appears to the agent —
    // 3 visible per upstream × 2 = 6.
    let mut list_out = Vec::new();
    server
        .serve(
            Cursor::new("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n"),
            &mut list_out,
        )
        .unwrap();
    let list: Value = serde_json::from_slice(&list_out).unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"notes_a__list_notes"), "{names:?}");
    assert!(names.contains(&"notes_b__delete_note"), "{names:?}");
    assert!(
        !names.contains(&"notes_a__create_note"),
        "a policy-denied tool is hidden from the served union: {names:?}"
    );
    assert_eq!(names.len(), 6, "3 visible tools per upstream, namespaced: {names:?}");

    // A read on upstream A: allowed, routed to A, returns A's real content.
    let read = call(&mut server, "notes_a__list_notes", serde_json::json!({}));
    assert!(read["result"].get("isError").is_none(), "allowed read: {read}");
    assert!(read["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Groceries"));

    // A destructive call on upstream B: approval-gated by policy, AutoApprove clears it, routed to B,
    // and actually deletes B's own note-1 (proving the call reached the right upstream and executed).
    let del = call(&mut server, "notes_b__delete_note", serde_json::json!({ "id": "note-1" }));
    assert!(del["result"].get("isError").is_none(), "approved delete: {del}");
    assert!(del["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Deleted note-1"));

    // A create is neither read- nor destructive-shaped → denied by the per-namespace default policy,
    // NEVER forwarded to the upstream, and (being blocked) NOT signed.
    let create = call(
        &mut server,
        "notes_a__create_note",
        serde_json::json!({ "title": "x", "body": "y" }),
    );
    assert_eq!(create["result"]["isError"], true, "create denied: {create}");

    // The one broker chain holds exactly the two executed calls (read + delete), both attributed,
    // both verifying, hash-chained. The denied create left no receipt.
    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "only the two executed calls are signed: {text}");
    let actions: Vec<String> = lines
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["action_id"].as_str().unwrap().to_string())
        .collect();
    assert!(actions.contains(&"notes_a__list_notes".to_string()), "{actions:?}");
    assert!(actions.contains(&"notes_b__delete_note".to_string()), "{actions:?}");

    let _ = std::fs::remove_dir_all(&dir);
}
