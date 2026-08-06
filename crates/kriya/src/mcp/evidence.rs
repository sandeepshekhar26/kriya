//! O-10 — a **read-only evidence** MCP server over the verified receipt store (doc 33 §5.8).
//!
//! This is deliberately NOT the governed action gateway ([`super::server::Server`], which routes
//! `tools/call` through policy → approval → budget → audit into an app). It is the inverse: a
//! local, zero-network, stdio MCP server whose tools let a governed agent *query its own signed
//! history* — "what did I deploy yesterday", "show my denied actions", "is the chain intact". The
//! non-egress posture is the pitch, so there is no executor, no policy, and no writes.
//!
//! **The honesty axiom of this server:** every answer is computed only from receipts that PASS
//! verification against their own embedded public key ([`crate::audit::verify_signed_line`]). A
//! line that does not verify is COUNTED and reported (`unverifiable`), never silently surfaced as
//! if it were proven. `receipt_get` on a tampered line reports `verified: false` rather than
//! returning its contents as fact. `chain_verify` reports the first break. Receipts are content-free
//! by construction (hashes stay hashes) — this server never adds content the receipts don't carry.
//!
//! The reader is itself in evidence: the binary that starts this server signs one
//! `kriya.evidence.mcp.start` receipt before serving (see `kriya-mcp --evidence`).

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::audit::{self, SignedReceipt};

use super::jsonrpc::{
    error_code, CallToolParams, CallToolResult, InitializeResult, ListToolsResult, Request,
    Response, Tool,
};

/// One audit log the server reads: its path and the exact raw lines on disk (each the bytes that
/// were signed+chained, without the trailing newline). Kept as raw strings so the hash-chain check
/// hashes precisely what was written.
struct LogFile {
    path: PathBuf,
    raw_lines: Vec<String>,
}

/// A verified receipt paired with the raw line it came from (for callers that want the signed
/// bytes) and the log it lives in.
struct Verified {
    signed: SignedReceipt,
    raw: String,
    log: PathBuf,
}

/// The names of the five read-only tools, in a stable order — also the `tools` list attested in the
/// `kriya.evidence.mcp.start` receipt.
pub const EVIDENCE_TOOL_NAMES: &[&str] = &[
    "receipts_search",
    "receipt_get",
    "chain_verify",
    "session_tree",
    "spend_summary",
];

/// A read-only MCP server over one or more audit-log **sources** (files or directories). Re-expands
/// the sources and re-reads + re-verifies every log on every tool call, so answers are always
/// current — a log that appears mid-session (e.g. this reader's own boot receipt) is picked up, and
/// no stale (or tampered-after-load) view is ever cached.
pub struct EvidenceServer {
    name: String,
    version: String,
    sources: Vec<PathBuf>,
    tools: Vec<Tool>,
}

impl EvidenceServer {
    /// Build a server over the given audit-log sources — each a `*.jsonl` file or a directory whose
    /// `*.jsonl` children are read. Sources are expanded fresh on every call (see [`expand_logs`]).
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        sources: Vec<PathBuf>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            sources,
            tools: tool_catalog(),
        }
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Read JSON-RPC lines from `reader`, write responses to `writer`. Blocks until EOF. Mirrors
    /// [`super::server::Server::serve`] — stdout is the protocol channel, one JSON object per line.
    pub fn serve<R: BufRead, W: Write>(&self, reader: R, writer: &mut W) -> std::io::Result<()> {
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => self.handle(req),
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

    /// Dispatch one parsed request. `None` for notifications (which must not be answered). Pure and
    /// read-only — directly unit-testable.
    pub fn handle(&self, req: Request) -> Option<Response> {
        if req.is_notification() {
            return None;
        }
        let id = req.id.clone().unwrap_or(Value::Null);
        let resp = match req.method.as_str() {
            "initialize" => Response::success(
                id,
                ok_value(InitializeResult::new(&self.name, &self.version)),
            ),
            "tools/list" => Response::success(
                id,
                ok_value(ListToolsResult {
                    tools: self.tools.clone(),
                }),
            ),
            "tools/call" => self.handle_call(id, req.params),
            "ping" => Response::success(id, json!({})),
            other => Response::error(
                id,
                error_code::METHOD_NOT_FOUND,
                format!("unknown method: {other}"),
            ),
        };
        Some(resp)
    }

    fn handle_call(&self, id: Value, params: Option<Value>) -> Response {
        let Some(params) = params else {
            return Response::error(id, error_code::INVALID_PARAMS, "tools/call requires params");
        };
        let call: CallToolParams = match serde_json::from_value(params) {
            Ok(c) => c,
            Err(e) => {
                return Response::error(
                    id,
                    error_code::INVALID_PARAMS,
                    format!("bad tools/call params: {e}"),
                )
            }
        };
        // Load + verify the store fresh for this call.
        let store = self.load();
        let args = &call.arguments;
        let result = match call.name.as_str() {
            "receipts_search" => ok_result(tool_receipts_search(&store, args)),
            "receipt_get" => ok_result(tool_receipt_get(&store, args)),
            "chain_verify" => ok_result(tool_chain_verify(&store)),
            "session_tree" => ok_result(tool_session_tree(&store, args)),
            "spend_summary" => ok_result(tool_spend_summary(&store, args)),
            other => CallToolResult::err(format!("unknown tool: {other}")),
        };
        Response::success(id, ok_value(result))
    }

    /// Read every configured log, verify each line, and split into verified receipts vs a count of
    /// unverifiable lines. This is where the honesty axiom lives — a line that fails verification
    /// never enters `verified`.
    fn load(&self) -> Store {
        let mut files: Vec<LogFile> = Vec::new();
        let mut verified: Vec<Verified> = Vec::new();
        let mut unverifiable = 0usize;
        // Expand sources (files or dirs) fresh, so a log written after construction is included.
        let mut paths: Vec<PathBuf> = self.sources.iter().flat_map(|s| expand_logs(s)).collect();
        paths.dedup();
        for path in &paths {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let raw_lines: Vec<String> = text.lines().map(str::to_string).collect();
            for raw in &raw_lines {
                if raw.trim().is_empty() {
                    continue;
                }
                let v = audit::verify_signed_line(raw);
                match v.signed {
                    Some(signed) if v.verified => verified.push(Verified {
                        signed,
                        raw: raw.clone(),
                        log: path.clone(),
                    }),
                    _ => unverifiable += 1,
                }
            }
            files.push(LogFile {
                path: path.clone(),
                raw_lines,
            });
        }
        Store {
            files,
            verified,
            unverifiable,
        }
    }
}

/// The verified view of the store for one tool call.
struct Store {
    files: Vec<LogFile>,
    verified: Vec<Verified>,
    /// Count of non-blank lines that failed verification, across all logs.
    unverifiable: usize,
}

// ---- Tool implementations (deterministic, read-only) ----

/// `receipts_search` — filter VERIFIED receipts by time window, action prefix, agent, and success.
fn tool_receipts_search(store: &Store, args: &Value) -> Value {
    let since = num_u128(args, "since");
    let until = num_u128(args, "until");
    let action_prefix = str_arg(args, "action_prefix");
    let agent = str_arg(args, "agent");
    let success = args.get("success").and_then(Value::as_bool);
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map(|n| n.min(1000) as usize)
        .unwrap_or(100);

    let mut matches: Vec<&Verified> = store
        .verified
        .iter()
        .filter(|v| {
            let r = &v.signed.receipt;
            if let Some(s) = since {
                if r.ts_ms < s {
                    return false;
                }
            }
            if let Some(u) = until {
                if r.ts_ms > u {
                    return false;
                }
            }
            if let Some(p) = &action_prefix {
                if !r.action_id.starts_with(p.as_str()) {
                    return false;
                }
            }
            if let Some(a) = &agent {
                if r.actor.as_ref().map(|ac| ac.agent.as_str()) != Some(a.as_str()) {
                    return false;
                }
            }
            if let Some(su) = success {
                if r.success != su {
                    return false;
                }
            }
            true
        })
        .collect();
    // Newest first — the natural "what happened recently" reading.
    matches.sort_by(|a, b| b.signed.receipt.ts_ms.cmp(&a.signed.receipt.ts_ms));
    let matched = matches.len();
    let returned: Vec<Value> = matches
        .iter()
        .take(limit)
        .map(|v| receipt_row(v))
        .collect();
    json!({
        "matched": matched,
        "returned": returned.len(),
        "unverifiable": store.unverifiable,
        "receipts": returned,
    })
}

/// `receipt_get` — fetch one receipt by `step_id`. Reports honestly when the step exists but does
/// not verify (a tampered line): `found: true, verified: false` rather than returning it as fact.
fn tool_receipt_get(store: &Store, args: &Value) -> Value {
    let Some(step_id) = str_arg(args, "step_id") else {
        return json!({ "error": "receipt_get requires a step_id" });
    };
    if let Some(v) = store
        .verified
        .iter()
        .find(|v| v.signed.receipt.step_id == step_id)
    {
        return json!({ "found": true, "verified": true, "receipt": receipt_row(v) });
    }
    // Not among the verified set — is there an unverifiable line carrying this step_id? Report it as
    // present-but-unproven so a query never silently "loses" a tampered receipt.
    for file in &store.files {
        for raw in &file.raw_lines {
            if raw.trim().is_empty() {
                continue;
            }
            if let Some(signed) = audit::parse_signed_line(raw) {
                if signed.receipt.step_id == step_id {
                    return json!({
                        "found": true,
                        "verified": false,
                        "reason": "signature does not verify — not surfaced as fact",
                        "step_id": step_id,
                    });
                }
            }
        }
    }
    json!({ "found": false, "step_id": step_id })
}

/// `chain_verify` — the R20 hash-chain integrity of each log: whether every line verifies and links
/// to the SHA-256 of the previous line, plus the index of the first break.
fn tool_chain_verify(store: &Store) -> Value {
    let logs: Vec<Value> = store
        .files
        .iter()
        .map(|f| {
            let verdict = audit::verify_chain(&f.raw_lines);
            json!({
                "path": f.path.display().to_string(),
                "lines": verdict.lines,
                "verified": verdict.verified,
                "first_break": verdict.first_break,
            })
        })
        .collect();
    let all_ok = store
        .files
        .iter()
        .all(|f| audit::verify_chain(&f.raw_lines).verified);
    json!({ "verified": all_ok, "logs": logs })
}

/// `session_tree` — group VERIFIED receipts by `kriya.corr.run_id`. With `run_id` set, return that
/// run's actions in time order; without it, a summary of every run seen.
fn tool_session_tree(store: &Store, args: &Value) -> Value {
    let want = str_arg(args, "run_id");
    // Preserve first-seen order of runs for determinism.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<&Verified>> =
        std::collections::HashMap::new();
    for v in &store.verified {
        let corr = crate::corr::from_json(v.signed.receipt.params.get(crate::corr::RESERVED_KEY));
        let key = corr.run_id.unwrap_or_else(|| "(unattributed)".to_string());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(v);
    }
    if let Some(run) = want {
        let mut rows: Vec<&Verified> = groups.remove(&run).unwrap_or_default();
        rows.sort_by_key(|v| v.signed.receipt.ts_ms);
        let actions: Vec<Value> = rows.iter().map(|v| receipt_row(v)).collect();
        return json!({ "run_id": run, "actions": actions, "count": actions.len() });
    }
    let sessions: Vec<Value> = order
        .iter()
        .map(|k| {
            let rows = &groups[k];
            let first = rows.iter().map(|v| v.signed.receipt.ts_ms).min().unwrap_or(0);
            let last = rows.iter().map(|v| v.signed.receipt.ts_ms).max().unwrap_or(0);
            let mut action_ids: Vec<String> = rows
                .iter()
                .map(|v| v.signed.receipt.action_id.clone())
                .collect();
            action_ids.sort();
            action_ids.dedup();
            json!({
                "run_id": k,
                "count": rows.len(),
                "first_ts_ms": u128_json(first),
                "last_ts_ms": u128_json(last),
                "action_ids": action_ids,
            })
        })
        .collect();
    json!({ "sessions": sessions, "count": sessions.len() })
}

/// `spend_summary` — a summary computed ONLY from verified `kriya.pay.*` receipts: purchases folded
/// by `pay_id`, counted by outcome, with known amounts summed per currency. Honest by construction:
/// a purchase with `amount_known: false` is counted under `unknown_amount`, never given a guessed
/// figure; this is NOT a re-priced token/model spend (that is the console's job) — it summarizes the
/// signed purchase chain. `period` ∈ `today|7d|30d|all` (default `all`) filters by the intent's ts.
fn tool_spend_summary(store: &Store, args: &Value) -> Value {
    let period = str_arg(args, "period").unwrap_or_else(|| "all".to_string());
    let since = period_since(&period, now_ms_safe());

    // Fold pay receipts by pay_id.
    #[derive(Default)]
    struct Chain {
        amount_minor: Option<i64>,
        currency: Option<String>,
        amount_known: bool,
        result: Option<String>,
        decision: Option<String>,
        intent_ts: Option<u128>,
    }
    let mut chains: std::collections::HashMap<String, Chain> = std::collections::HashMap::new();
    for v in &store.verified {
        let r = &v.signed.receipt;
        let p = &r.params;
        let Some(pay_id) = p.get("pay_id").and_then(Value::as_str) else {
            continue;
        };
        let c = chains.entry(pay_id.to_string()).or_default();
        match r.action_id.as_str() {
            "kriya.pay.intent" => {
                c.intent_ts = Some(r.ts_ms);
                c.amount_known = p.get("amount_known").and_then(Value::as_bool).unwrap_or(false);
                c.amount_minor = p.get("amount_minor").and_then(Value::as_i64);
                c.currency = p.get("currency").and_then(Value::as_str).map(str::to_string);
            }
            "kriya.pay.decision" => {
                c.decision = p.get("decision").and_then(Value::as_str).map(str::to_string);
            }
            "kriya.pay.outcome" => {
                c.result = p.get("result").and_then(Value::as_str).map(str::to_string);
            }
            _ => {}
        }
    }

    let mut purchases = 0usize;
    let mut executed = 0usize;
    let mut denied = 0usize;
    let mut held = 0usize;
    let mut unknown_amount = 0usize;
    let mut totals: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for c in chains.values() {
        // Period filter on the intent timestamp; a chain with no intent ts is included under "all".
        if let (Some(s), Some(ts)) = (since, c.intent_ts) {
            if ts < s {
                continue;
            }
        }
        purchases += 1;
        match c.result.as_deref() {
            Some("executed") => executed += 1,
            Some("denied") => denied += 1,
            _ => {
                // No outcome: a held decision is the honest 2/3 shape.
                if c.decision.as_deref() == Some("held") {
                    held += 1;
                }
            }
        }
        // Only count a known amount toward the currency total when the purchase actually executed.
        if c.result.as_deref() == Some("executed") {
            match (c.amount_known, c.amount_minor, &c.currency) {
                (true, Some(minor), Some(cur)) => {
                    *totals.entry(cur.clone()).or_insert(0) += minor;
                }
                _ => unknown_amount += 1,
            }
        }
    }
    let totals_json: Value = Value::Object(
        totals
            .into_iter()
            .map(|(cur, minor)| (cur, json!(minor)))
            .collect(),
    );
    json!({
        "period": period,
        "purchases": purchases,
        "executed": executed,
        "held": held,
        "denied": denied,
        "unknown_amount": unknown_amount,
        "executed_totals_minor": totals_json,
        "note": "verified kriya.pay.* purchase chains only; amounts in minor units; not a re-priced token spend",
        "unverifiable": store.unverifiable,
    })
}

// ---- Shared helpers ----

/// A compact, content-free summary row for a verified receipt: identity, action, actor, success,
/// time, chain pointer, the receipt's own params (already content-free), and the raw signed line.
fn receipt_row(v: &Verified) -> Value {
    let r = &v.signed.receipt;
    json!({
        "step_id": r.step_id,
        "action_id": r.action_id,
        "agent": r.actor.as_ref().map(|a| a.agent.clone()),
        "user": r.actor.as_ref().map(|a| a.user.clone()),
        "success": r.success,
        "ts_ms": u128_json(r.ts_ms),
        "prev_hash": r.prev_hash,
        "params": r.params,
        "verified": true,
        "log": v.log.display().to_string(),
        "raw": v.raw,
    })
}

fn tool_catalog() -> Vec<Tool> {
    let obj = |props: Value| json!({ "type": "object", "properties": props });
    vec![
        Tool {
            name: "receipts_search".into(),
            description: "Search VERIFIED signed receipts by time window, action_id prefix, agent, and success. Returns only receipts that pass signature verification; unverifiable lines are counted separately, never surfaced.".into(),
            input_schema: obj(json!({
                "since": {"type":"integer","description":"lower bound, unix ms (inclusive)"},
                "until": {"type":"integer","description":"upper bound, unix ms (inclusive)"},
                "action_prefix": {"type":"string","description":"match action_id by prefix, e.g. 'kriya.io' or 'claude-code'"},
                "agent": {"type":"string","description":"exact actor.agent match"},
                "success": {"type":"boolean"},
                "limit": {"type":"integer","description":"max rows (default 100, cap 1000)"}
            })),
        },
        Tool {
            name: "receipt_get".into(),
            description: "Fetch one receipt by step_id. Reports verified:false when the line exists but its signature does not verify (never returns tampered contents as fact).".into(),
            input_schema: obj(json!({ "step_id": {"type":"string"} })),
        },
        Tool {
            name: "chain_verify".into(),
            description: "Verify the R20 hash-chain of each audit log: whether every line's signature verifies and links to the SHA-256 of the previous line, plus the index of the first break.".into(),
            input_schema: obj(json!({})),
        },
        Tool {
            name: "session_tree".into(),
            description: "Group verified receipts by run/session (kriya.corr.run_id). With run_id, returns that run's actions in time order; without it, a summary of every run.".into(),
            input_schema: obj(json!({ "run_id": {"type":"string","description":"omit for a summary of all runs"} })),
        },
        Tool {
            name: "spend_summary".into(),
            description: "Summarize verified kriya.pay.* purchase chains: purchases folded by pay_id, counted by outcome, known amounts summed per currency (minor units). Not a re-priced token spend.".into(),
            input_schema: obj(json!({ "period": {"type":"string","enum":["today","7d","30d","all"],"description":"default 'all'"} })),
        },
    ]
}

fn ok_result(value: Value) -> CallToolResult {
    CallToolResult::ok(serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".into()))
}

fn ok_value<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn num_u128(args: &Value, key: &str) -> Option<u128> {
    args.get(key).and_then(Value::as_u64).map(|n| n as u128)
}

/// JSON numbers cap at u64; audit ts_ms fits comfortably, so render u128 as a JSON number via u64.
fn u128_json(ts: u128) -> Value {
    json!(ts as u64)
}

fn now_ms_safe() -> u128 {
    audit::now_ms()
}

/// Resolve a period label into a lower-bound ms, given "now". `all` ⇒ `None` (no lower bound).
fn period_since(period: &str, now: u128) -> Option<u128> {
    let day = 86_400_000u128;
    match period {
        "today" => Some(now.saturating_sub(day)),
        "7d" => Some(now.saturating_sub(7 * day)),
        "30d" => Some(now.saturating_sub(30 * day)),
        _ => None,
    }
}

/// Expand a `--audit` path into concrete log files: a `*.jsonl` file is itself; a directory
/// contributes its `*.jsonl` children (sorted for determinism). A missing path yields nothing.
pub fn expand_logs(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        let mut out: Vec<PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        out.sort();
        out
    } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{Actor, Receipt, Signer};
    use crate::corr::{self, Correlation};

    /// A unique temp dir + a fresh log signed under the RFC 8032 test-vector key. The dir is
    /// returned so the caller keeps it alive; it is cleaned by the OS temp reaper (the crate carries
    /// no dev-deps, so no `tempdir` — this mirrors the audit/contain test pattern).
    fn write_log(receipts: Vec<Receipt>) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kriya-evidence-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("claude-code.jsonl");
        let key = dir.join("fixture.key");
        // RFC 8032 test-vector seed — a public well-known key (synthetic evidence).
        std::fs::write(&key, "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
            .unwrap();
        let signer = Signer::with_identity(&key, log.clone()).unwrap();
        for r in receipts {
            signer.record(r);
        }
        (log, dir)
    }

    fn r(step: &str, action: &str, ts: u128, success: bool, params: Value) -> Receipt {
        Receipt::new(step.into(), action.into(), params, success, ts)
            .with_actor(Some(Actor::new("claude-code", "op")))
    }

    fn call(server: &EvidenceServer, name: &str, args: Value) -> Value {
        let req: Request = serde_json::from_value(json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params": {"name": name, "arguments": args}
        }))
        .unwrap();
        let resp = server.handle(req).unwrap();
        let result = resp.result.unwrap();
        // Unwrap the MCP text-content envelope back into JSON.
        let text = result["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn initialize_and_list_expose_the_read_only_tools() {
        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![]);
        let init: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":"initialize"})).unwrap();
        let r = server.handle(init).unwrap().result.unwrap();
        assert_eq!(r["serverInfo"]["name"], "kriya-evidence");
        let list: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let tools = server.handle(list).unwrap().result.unwrap()["tools"].clone();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, EVIDENCE_TOOL_NAMES);
    }

    #[test]
    fn receipts_search_filters_and_excludes_tampered_lines() {
        let (log, _d) = write_log(vec![
            r("s1", "claude-code.deploy", 1000, true, json!({})),
            r("s2", "kriya.io.egress.http.allow", 2000, true, json!({"dest_host": "api.example.com"})),
            r("s3", "claude-code.deploy", 3000, false, json!({})),
        ]);
        // Tamper the middle line so it must not appear in results.
        let text = std::fs::read_to_string(&log).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        lines[1] = lines[1].replace("api.example.com", "evil.example.com");
        std::fs::write(&log, lines.join("\n") + "\n").unwrap();

        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![log]);
        // Prefix filter for deploys: both s1 (true) and s3 (false) verify; the tampered io line is out.
        let out = call(&server, "receipts_search", json!({ "action_prefix": "claude-code" }));
        assert_eq!(out["matched"], 2);
        assert_eq!(out["unverifiable"], 1, "the tampered line is counted, not surfaced");
        let ids: Vec<&str> = out["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["action_id"].as_str().unwrap())
            .collect();
        assert!(ids.iter().all(|a| a.starts_with("claude-code")));
        // The tampered egress host must appear NOWHERE in the response.
        assert!(
            !out.to_string().contains("evil.example.com") && !out.to_string().contains("api.example.com"),
            "a tampered receipt must never appear in search results"
        );

        // success filter
        let only_ok = call(&server, "receipts_search", json!({ "success": true, "action_prefix": "claude-code" }));
        assert_eq!(only_ok["matched"], 1);
    }

    #[test]
    fn receipt_get_reports_tampered_as_unverified() {
        let (log, _d) = write_log(vec![r("keep", "claude-code.deploy", 1000, true, json!({"k": "v"}))]);
        let text = std::fs::read_to_string(&log).unwrap();
        std::fs::write(&log, text.replace("\"v\"", "\"TAMPERED\"")).unwrap();
        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![log]);
        let out = call(&server, "receipt_get", json!({ "step_id": "keep" }));
        assert_eq!(out["found"], true);
        assert_eq!(out["verified"], false, "tampered receipt reported, not returned as fact");
    }

    #[test]
    fn receipt_get_returns_a_verified_receipt() {
        let (log, _d) = write_log(vec![r("keep", "claude-code.deploy", 1000, true, json!({"k": "v"}))]);
        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![log]);
        let out = call(&server, "receipt_get", json!({ "step_id": "keep" }));
        assert_eq!(out["found"], true);
        assert_eq!(out["verified"], true);
        assert_eq!(out["receipt"]["action_id"], "claude-code.deploy");
    }

    #[test]
    fn chain_verify_reports_break_after_deletion() {
        let (log, _d) = write_log(vec![
            r("s1", "a.one", 1, true, json!({})),
            r("s2", "a.two", 2, true, json!({})),
            r("s3", "a.three", 3, true, json!({})),
        ]);
        // Intact first.
        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![log.clone()]);
        let intact = call(&server, "chain_verify", json!({}));
        assert_eq!(intact["verified"], true);
        // Delete the middle line.
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&log, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        let broken = call(&server, "chain_verify", json!({}));
        assert_eq!(broken["verified"], false);
        assert_eq!(broken["logs"][0]["first_break"], 1);
    }

    #[test]
    fn session_tree_groups_by_run_id() {
        let corr_a = Correlation::run("run-A");
        let corr_b = Correlation::run("run-B");
        let (log, _d) = write_log(vec![
            r("s1", "a.one", 1, true, corr::attach(json!({}), &corr_a)),
            r("s2", "a.two", 2, true, corr::attach(json!({}), &corr_a)),
            r("s3", "b.one", 3, true, corr::attach(json!({}), &corr_b)),
        ]);
        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![log]);
        let summary = call(&server, "session_tree", json!({}));
        assert_eq!(summary["count"], 2);
        let run_a = call(&server, "session_tree", json!({ "run_id": "run-A" }));
        assert_eq!(run_a["count"], 2);
        assert_eq!(run_a["actions"][0]["action_id"], "a.one");
    }

    #[test]
    fn spend_summary_folds_pay_chains() {
        let (log, _d) = write_log(vec![
            // executed $42.00 usd
            r("p1i", "kriya.pay.intent", 1000, true, json!({"pay_id":"P1","amount_known":true,"amount_minor":4200,"currency":"usd"})),
            r("p1d", "kriya.pay.decision", 1001, true, json!({"pay_id":"P1","decision":"approved"})),
            r("p1o", "kriya.pay.outcome", 1002, true, json!({"pay_id":"P1","result":"executed"})),
            // held, unknown amount
            r("p2i", "kriya.pay.intent", 2000, true, json!({"pay_id":"P2","amount_known":false})),
            r("p2d", "kriya.pay.decision", 2001, true, json!({"pay_id":"P2","decision":"held"})),
        ]);
        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![log]);
        let out = call(&server, "spend_summary", json!({ "period": "all" }));
        assert_eq!(out["purchases"], 2);
        assert_eq!(out["executed"], 1);
        assert_eq!(out["held"], 1);
        assert_eq!(out["executed_totals_minor"]["usd"], 4200);
    }

    #[test]
    fn unknown_tool_is_an_error_result() {
        let server = EvidenceServer::new("kriya-evidence", "0.1.0", vec![]);
        let req: Request = serde_json::from_value(json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params": {"name": "nope", "arguments": {}}
        }))
        .unwrap();
        let resp = server.handle(req).unwrap();
        assert_eq!(resp.result.unwrap()["isError"], true);
    }
}
