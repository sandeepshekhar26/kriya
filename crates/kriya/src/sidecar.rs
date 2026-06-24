//! Sidecar host (roadmap **R3**): run the agent loop as a standalone process an app's main
//! process talks to over **stdio**, so Electron and plain Node apps can host the runtime, not
//! just Tauri. This is the cross-shell decoupling that lets kriya bolt onto an existing app
//! whatever its shell — and it keeps governance (policy/approval/budget/audit) in a process
//! the renderer can't tamper with.
//!
//! ## Wire protocol (newline-delimited JSON, both directions)
//!
//! The app writes these to the sidecar's **stdin**:
//! - `{"type":"start","data":<AgentStartRequest>}` — begin a run
//! - `{"type":"action_result","data":<AgentActionResult>}` — result of a dispatched action
//! - `{"type":"approval_response","data":<AgentApprovalResponse>}` — human approval decision
//! - `{"type":"step_advance","data":<AgentStepAdvance>}` — step-mode advance/stop
//!
//! The sidecar writes these to its **stdout** (one JSON object per line):
//! - `{"type":"action","data":<AgentActionRequest>}` — run this action, reply with a result
//! - `{"type":"approval","data":<AgentApprovalRequest>}` — needs a human
//! - `{"type":"await_step","data":<AgentAwaitStep>}` — paused in step-mode
//! - `{"type":"done","data":<AgentDone>}` — run finished
//! - `{"type":"log","data":<AgentLog>}` — telemetry
//!
//! These mirror the Tauri command/event names exactly, so the same [`HostSink`] loop drives
//! both shells. The `kriya-sidecar` package is the Node/TS client for this protocol.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::json;

use crate::audit::Signer;
use crate::permissions::Policy;
use crate::protocol::{
    AgentActionResult, AgentApprovalResponse, AgentDone, AgentLog, AgentStartRequest,
    AgentStepAdvance,
};
use crate::{run_task, ApprovalMap, HostSink, Inference, PendingMap, StepAdvanceMap};

/// A thread-safe stdout (or any writer) the sidecar serializes events to.
pub type SharedWriter = Arc<Mutex<dyn Write + Send>>;

/// A [`HostSink`] that serializes each event as one line of `{"type","data"}` JSON to a
/// shared writer. The shared lock serializes concurrent runs' output so lines never interleave.
pub struct StdioSink {
    out: SharedWriter,
}

impl StdioSink {
    pub fn new(out: SharedWriter) -> Self {
        Self { out }
    }

    fn send<T: serde::Serialize>(&self, kind: &str, data: &T) {
        let line = json!({ "type": kind, "data": data });
        if let Ok(mut w) = self.out.lock() {
            let _ = writeln!(w, "{line}");
            let _ = w.flush();
        }
    }
}

impl HostSink for StdioSink {
    fn emit_action(&self, req: &crate::protocol::AgentActionRequest) {
        self.send("action", req);
    }
    fn emit_approval(&self, req: &crate::protocol::AgentApprovalRequest) {
        self.send("approval", req);
    }
    fn emit_await_step(&self, ev: &crate::protocol::AgentAwaitStep) {
        self.send("await_step", ev);
    }
    fn emit_done(&self, done: &AgentDone) {
        self.send("done", done);
    }
    fn emit_log(&self, entry: &AgentLog) {
        self.send("log", entry);
    }
}

/// app -> sidecar: ask for a window of recent episodic memory. Answered out-of-band with a
/// `memory` line (request/response, not part of any run) — the sidecar mirror of the Tauri
/// `agent_memory_recent` command, so an Electron app can power the inspector's MemoryPanel.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryRecentRequest {
    /// Echoed back on the response so a client can correlate concurrent queries.
    #[serde(default)]
    request_id: Option<String>,
    /// How many newest episodes to return (default 20).
    #[serde(default)]
    limit: Option<u32>,
}

/// Messages the app sends to the sidecar over stdin.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Inbound {
    Start { data: AgentStartRequest },
    ActionResult { data: AgentActionResult },
    ApprovalResponse { data: AgentApprovalResponse },
    StepAdvance { data: AgentStepAdvance },
    MemoryRecent { data: MemoryRecentRequest },
}

/// Read the newest `limit` episodes from the same durable store the host loop writes to
/// (`<temp>/kriya-memory.db`, matching `agent::host` and the Tauri `agent_memory_recent`
/// command). A fresh connection per query — cheap, and it tolerates a run writing from another
/// thread. If no run has ever recorded anything the store opens empty and this returns `[]`.
fn read_recent_memory(limit: u32) -> Result<Vec<crate::memory::Episode>, String> {
    let path = std::env::temp_dir().join("kriya-memory.db");
    let mem = crate::memory::AgentMemory::open(&path)?;
    mem.recent(limit)
}

/// Run the sidecar loop: read inbound JSON lines from `reader`, drive runs, and write events
/// to `out`. Each `start` spawns the agent loop on its own thread (so the reader keeps
/// accepting results concurrently — exactly how Tauri commands run off the loop thread).
///
/// `make_backend` is called once per run to mint a fresh inference backend. On EOF (the app
/// closed stdin) the loop joins in-flight runs before returning, so a shutdown doesn't sever
/// a run mid-flight — bounded by the loop's own result timeout if a reply never comes.
pub fn run_sidecar<R, F>(
    reader: R,
    out: SharedWriter,
    policy: Arc<Policy>,
    signer: Arc<Signer>,
    make_backend: F,
) -> std::io::Result<()>
where
    R: BufRead,
    F: Fn() -> Box<dyn Inference>,
{
    let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
    let approvals: ApprovalMap = Arc::new(Mutex::new(HashMap::new()));
    let advances: StepAdvanceMap = Arc::new(Mutex::new(HashMap::new()));

    let mut runs = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Inbound>(&line) {
            Ok(Inbound::Start { data }) => {
                let sink: Arc<dyn HostSink> = Arc::new(StdioSink::new(out.clone()));
                let err_sink = sink.clone();
                let backend = make_backend();
                let (p, a, ad) = (pending.clone(), approvals.clone(), advances.clone());
                let (pol, sig) = (policy.clone(), signer.clone());
                runs.push(std::thread::spawn(move || {
                    if let Err(e) = run_task(sink, p, a, ad, pol, sig, backend, data) {
                        // Surface the failure to the app the same way the Tauri glue does.
                        err_sink.emit_log(&AgentLog {
                            step_id: None,
                            level: "error".into(),
                            message: e.clone(),
                            detail: None,
                        });
                        err_sink.emit_done(&AgentDone {
                            summary: format!("Failed: {e}"),
                            steps: 0,
                        });
                    }
                }));
            }
            Ok(Inbound::ActionResult { data }) => {
                if let Some(tx) = pending.lock().unwrap().remove(&data.step_id) {
                    let _ = tx.send(data);
                }
            }
            Ok(Inbound::ApprovalResponse { data }) => {
                if let Some(tx) = approvals.lock().unwrap().remove(&data.step_id) {
                    let _ = tx.send(data.approved);
                }
            }
            Ok(Inbound::StepAdvance { data }) => {
                if let Some(tx) = advances.lock().unwrap().remove(&data.gate_id) {
                    let _ = tx.send(data.proceed);
                }
            }
            Ok(Inbound::MemoryRecent { data }) => {
                // A quick SQLite read answered inline; reply on the same stream the events use.
                // Always answer (with `error` set on failure) so the client never hangs on a
                // pending query.
                let (episodes, error) = match read_recent_memory(data.limit.unwrap_or(20)) {
                    Ok(eps) => (eps, None),
                    Err(e) => (Vec::new(), Some(e)),
                };
                let line = json!({
                    "type": "memory",
                    "data": { "requestId": data.request_id, "episodes": episodes, "error": error }
                });
                if let Ok(mut w) = out.lock() {
                    let _ = writeln!(w, "{line}");
                    let _ = w.flush();
                }
            }
            Err(e) => {
                // Malformed inbound: tell the app over the same channel rather than crashing.
                if let Ok(mut w) = out.lock() {
                    let msg = json!({
                        "type": "log",
                        "data": { "level": "error", "message": format!("bad inbound message: {e}") }
                    });
                    let _ = writeln!(w, "{msg}");
                    let _ = w.flush();
                }
            }
        }
    }

    for run in runs {
        let _ = run.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::inference::scripted::ScriptedPlanner;
    use crate::protocol::AgentActionRequest;
    use crate::StepDecision;
    use serde_json::Value;

    /// A `Write` that appends to a shared buffer the test can inspect afterward.
    struct VecWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for VecWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn shared() -> (Arc<Mutex<Vec<u8>>>, SharedWriter) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let out: SharedWriter = Arc::new(Mutex::new(VecWriter(buf.clone())));
        (buf, out)
    }

    fn text(buf: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn stdio_sink_emits_tagged_lines() {
        let (buf, out) = shared();
        let sink = StdioSink::new(out);
        sink.emit_action(&AgentActionRequest {
            step_id: "s1".into(),
            action_id: "create_note".into(),
            params: json!({"title": "hi"}),
            reasoning: "r".into(),
        });
        let line = text(&buf);
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "action");
        assert_eq!(v["data"]["actionId"], "create_note");
    }

    #[test]
    fn start_drives_a_scripted_run_to_done() {
        let (buf, out) = shared();
        // Done-only script: the run needs no action round-trip, so this is deterministic.
        let start = json!({
            "type": "start",
            "data": { "goal": "noop", "state": {}, "tools": [] }
        })
        .to_string();
        let reader = std::io::Cursor::new(format!("{start}\n"));

        run_sidecar(
            reader,
            out,
            Arc::new(Policy::default()),
            Arc::new(Signer::new()),
            || {
                Box::new(ScriptedPlanner::from_decisions(vec![StepDecision::Done {
                    summary: "nothing to do".into(),
                }]))
            },
        )
        .unwrap();

        // run_sidecar joins the run thread before returning, so the done line is present.
        let out_text = text(&buf);
        assert!(out_text.contains("\"type\":\"done\""), "got: {out_text}");
    }

    #[test]
    fn memory_recent_request_gets_a_correlated_memory_response() {
        let (buf, out) = shared();
        // No `start`, so the only output is the memory reply — deterministic to assert.
        let req = json!({ "type": "memory_recent", "data": { "requestId": "q1", "limit": 5 } })
            .to_string();
        let reader = std::io::Cursor::new(format!("{req}\n"));

        run_sidecar(
            reader,
            out,
            Arc::new(Policy::default()),
            Arc::new(Signer::new()),
            || Box::new(ScriptedPlanner::from_decisions(vec![])),
        )
        .unwrap();

        let line = text(&buf);
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["type"], "memory");
        assert_eq!(v["data"]["requestId"], "q1"); // echoed for correlation
        assert!(
            v["data"]["episodes"].is_array(),
            "episodes present even when empty"
        );
    }

    #[test]
    fn malformed_inbound_reports_an_error_line() {
        let (buf, out) = shared();
        let reader = std::io::Cursor::new("not json at all\n");
        run_sidecar(
            reader,
            out,
            Arc::new(Policy::default()),
            Arc::new(Signer::new()),
            || Box::new(ScriptedPlanner::from_decisions(vec![])),
        )
        .unwrap();
        assert!(
            text(&buf).contains("bad inbound message"),
            "got: {}",
            text(&buf)
        );
    }
}
