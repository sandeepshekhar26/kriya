//! End-to-end smoke test for `kriya-mcp --evidence` (O-10, doc 33 §5.8): the READ-ONLY evidence
//! reader over the verified receipt store. Builds the real binary, signs a small audit log with a
//! FIXED key, tampers one line, then drives the compiled server over stdio (NDJSON JSON-RPC) exactly
//! as a Claude Code / Cursor MCP client would — asserting on the real process boundary.
//!
//! The load-bearing claim proven here (the item's TEST section): **a tampered receipt never appears
//! in `receipts_search` results** — it is counted as `unverifiable` and excluded. Also proves the
//! reader is itself in evidence: its `kriya.evidence.mcp.start` boot receipt is queryable through
//! its own tools, and `chain_verify` flags the tampered log.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use kriya::audit::{Actor, Receipt, Signer};
use serde_json::{json, Value};

// RFC 8032 Ed25519 test-vector seed — a PUBLIC, well-known key (synthetic evidence, never a real
// signing identity; identical to the crate's other parity fixtures).
const SEED_HEX: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";

fn build_binary() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--bin", "kriya-mcp"])
        .status()
        .expect("cargo build should run");
    assert!(status.success(), "kriya-mcp must build");
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target/debug/kriya-mcp");
    assert!(path.is_file(), "expected binary at {}", path.display());
    path
}

/// Sign three receipts into `<dir>/claude-code.jsonl`, then tamper the middle line's params so it no
/// longer verifies. Returns the audit dir.
fn seed_audit_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kriya-evidence-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = dir.join("fixture.key");
    std::fs::write(&key, SEED_HEX).unwrap();
    let log = dir.join("claude-code.jsonl");
    let signer = Signer::with_identity(&key, log.clone()).unwrap();
    let actor = Actor::new("claude-code", "platform-eng");
    let r = |s: &str, a: &str, ts: u128, ok: bool, p: Value| {
        Receipt::new(s.into(), a.into(), p, ok, ts).with_actor(Some(actor.clone()))
    };
    signer.record(r("s1", "claude-code.deploy", 1000, true, json!({})));
    signer.record(r(
        "s2",
        "kriya.io.egress.http.allow",
        2000,
        true,
        json!({ "dest_host": "api.example.com" }),
    ));
    signer.record(r("s3", "claude-code.deploy", 3000, false, json!({})));

    // Tamper the egress line — flip the host — WITHOUT re-signing.
    let text = std::fs::read_to_string(&log).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[1] = lines[1].replace("api.example.com", "evil.example.com");
    std::fs::write(&log, lines.join("\n") + "\n").unwrap();
    dir
}

/// Send a batch of JSON-RPC requests to `kriya-mcp --evidence` over stdio and collect one parsed
/// response per non-notification line.
fn drive(bin: &PathBuf, audit_dir: &PathBuf, requests: &[Value]) -> Vec<Value> {
    let mut child = Command::new(bin)
        .args(["--evidence", "--audit"])
        .arg(audit_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn kriya-mcp --evidence");
    {
        let stdin = child.stdin.as_mut().unwrap();
        for req in requests {
            writeln!(stdin, "{}", serde_json::to_string(req).unwrap()).unwrap();
        }
        // Drop stdin (EOF) so the server loop exits after answering.
    }
    child.stdin.take();
    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line.unwrap();
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str::<Value>(&line).expect("valid JSON-RPC response line"));
    }
    let _ = child.wait();
    out
}

/// Unwrap a `tools/call` response's MCP text-content envelope back into the tool's JSON payload.
fn tool_payload(resp: &Value) -> Value {
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("tool result text");
    serde_json::from_str(text).expect("tool payload JSON")
}

fn call(name: &str, id: i64, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})
}

#[test]
fn evidence_reader_serves_verified_receipts_and_excludes_tampered() {
    let bin = build_binary();
    let dir = seed_audit_dir();

    let responses = drive(
        &bin,
        &dir,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
            call("receipts_search", 3, json!({ "action_prefix": "claude-code" })),
            call("receipts_search", 4, json!({ "action_prefix": "kriya.io" })),
            call("chain_verify", 5, json!({})),
            call("receipts_search", 6, json!({ "action_prefix": "kriya.evidence" })),
        ],
    );
    assert_eq!(responses.len(), 6, "one response per request: {responses:?}");

    // initialize
    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "kriya-mcp");
    // tools/list — the five read-only tools
    let names: Vec<&str> = responses[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "receipts_search",
            "receipt_get",
            "chain_verify",
            "session_tree",
            "spend_summary"
        ]
    );

    // receipts_search over claude-code.* — both deploys verify, the tampered egress line does not.
    let deploys = tool_payload(&responses[2]);
    assert_eq!(deploys["matched"], 2, "both deploy receipts verify");
    assert_eq!(deploys["unverifiable"], 1, "the tampered line is counted, not surfaced");

    // receipts_search over kriya.io.* — the ONLY io receipt was tampered, so nothing surfaces, and
    // neither the forged NOR the original host appears anywhere in the response.
    let io = tool_payload(&responses[3]);
    assert_eq!(io["matched"], 0, "the tampered egress receipt must not appear");
    let io_str = io.to_string();
    assert!(
        !io_str.contains("evil.example.com") && !io_str.contains("api.example.com"),
        "a tampered receipt must never leak its contents through search"
    );

    // chain_verify — the tampered claude-code.jsonl breaks; the evidence-mcp.jsonl chain is intact.
    let chain = tool_payload(&responses[4]);
    assert_eq!(chain["verified"], false, "the tampered log breaks the chain");
    let broke = chain["logs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["path"].as_str().unwrap().ends_with("claude-code.jsonl"))
        .unwrap();
    assert_eq!(broke["first_break"], 1, "break at the tampered middle line");

    // The reader is itself in evidence: its boot receipt is queryable through its own tools.
    let boot = tool_payload(&responses[5]);
    assert_eq!(boot["matched"], 1, "kriya.evidence.mcp.start is queryable");
    assert_eq!(
        boot["receipts"][0]["action_id"], "kriya.evidence.mcp.start",
        "the reader signed its own start receipt"
    );
    assert_eq!(boot["receipts"][0]["params"]["scope"], "read-only");

    let _ = std::fs::remove_dir_all(&dir);
}
