//! How a governed `tools/call` actually runs the action, once the gates have cleared.
//!
//! The MCP server doesn't know *how* an app executes an action — a Tauri app dispatches it
//! in the webview, a sidecar (roadmap R3) forwards it to the app's main process, a CLI app
//! shells out. So execution is a trait. The governor owns the policy/approval/budget/audit
//! decision; the executor owns only the mechanics of running the cleared action and
//! reporting back what happened.

use serde_json::Value;

/// What running an action produced. `data` is whatever the handler returned (typically the
/// refreshed app state) and is surfaced to the calling agent; `error` is set on failure.
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub success: bool,
    pub data: Value,
    pub error: Option<String>,
}

impl ActionOutcome {
    pub fn ok(data: Value) -> Self {
        Self {
            success: true,
            data,
            error: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            success: false,
            data: Value::Null,
            error: Some(error.into()),
        }
    }
}

/// Runs an action that has already passed every governance gate. Implementations must not
/// re-check policy — that decision was already made and audited by the [`super::governor`].
pub trait ActionExecutor: Send {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome;
}

/// Adapts a closure into an [`ActionExecutor`]. Handy for tests and for embedders that
/// already hold a dispatch function.
pub struct FnExecutor<F>(pub F);

impl<F> ActionExecutor for FnExecutor<F>
where
    F: FnMut(&str, &Value) -> ActionOutcome + Send,
{
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        (self.0)(action_id, params)
    }
}

/// Runs each cleared action by shelling out to an external command — the simplest way to
/// bolt governed MCP onto an app that isn't Tauri. For every call the executor spawns the
/// command, writes one line of `{"action","params"}` JSON to its stdin, and reads one line
/// of `{"success","data","error"}` JSON back from its stdout. The app supplies that handler.
///
/// Per-call spawn keeps this dependency-free and stateless — fine for cheap, stateless
/// handlers. For a handler that holds an expensive connection, use
/// [`PersistentProcessExecutor`], which keeps it alive across calls (same line contract).
pub struct ProcessExecutor {
    program: String,
    args: Vec<String>,
}

impl ProcessExecutor {
    /// Build from a shell-style command string, e.g. `"node handle-action.js"`. Split on
    /// whitespace — the first token is the program, the rest are fixed leading arguments.
    pub fn new(command: &str) -> Self {
        let mut parts = command.split_whitespace().map(String::from);
        let program = parts.next().unwrap_or_default();
        let args = parts.collect();
        Self { program, args }
    }

    fn run(&self, action_id: &str, params: &Value) -> Result<ActionOutcome, String> {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};

        let mut child = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("failed to spawn '{}': {e}", self.program))?;

        {
            let stdin = child.stdin.as_mut().ok_or("child stdin unavailable")?;
            writeln!(stdin, "{}", request_line(action_id, params))
                .map_err(|e| format!("write to child failed: {e}"))?;
        } // drop stdin → EOF, so handlers that read-to-end terminate.

        let stdout = child.stdout.take().ok_or("child stdout unavailable")?;
        let mut last_line = String::new();
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| format!("read from child failed: {e}"))?;
            if !line.trim().is_empty() {
                last_line = line;
            }
        }
        let _ = child.wait();

        if last_line.trim().is_empty() {
            return Err("handler produced no JSON response line".into());
        }
        parse_reply(&last_line)
    }
}

impl ActionExecutor for ProcessExecutor {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        // Any plumbing failure becomes a failed outcome the agent can read — never a panic
        // that would take down the whole MCP session.
        self.run(action_id, params)
            .unwrap_or_else(ActionOutcome::failed)
    }
}

/// The handler-reply contract, shared by every process executor: one line of
/// `{ success: bool, data?: any, error?: string }`.
fn request_line(action_id: &str, params: &Value) -> String {
    serde_json::json!({ "action": action_id, "params": params }).to_string()
}

fn parse_reply(line: &str) -> Result<ActionOutcome, String> {
    let reply: Value =
        serde_json::from_str(line).map_err(|e| format!("handler response was not JSON: {e}"))?;
    let success = reply
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if success {
        Ok(ActionOutcome::ok(
            reply.get("data").cloned().unwrap_or(Value::Null),
        ))
    } else {
        let err = reply
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("handler reported failure")
            .to_string();
        Ok(ActionOutcome::failed(err))
    }
}

/// Like [`ProcessExecutor`], but keeps the handler process **alive across calls** — spawned
/// once, then one request line and one response line per action over its stdio. This is what a
/// real bolt-on needs: a handler that holds an expensive in-process connection (e.g. Actual
/// Budget's `@actual-app/api`, which loads a local SQLite budget on `init()`) pays that cost
/// once, not on every `tools/call`. Same line contract as `ProcessExecutor`, so the same
/// handler script works in either mode.
///
/// The child is spawned lazily on the first call and reused thereafter; if it dies, the next
/// call surfaces the failure as a failed `ActionOutcome` (never a panic).
pub struct PersistentProcessExecutor {
    program: String,
    args: Vec<String>,
    proc: Option<Handles>,
}

struct Handles {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl PersistentProcessExecutor {
    pub fn new(command: &str) -> Self {
        let mut parts = command.split_whitespace().map(String::from);
        let program = parts.next().unwrap_or_default();
        let args = parts.collect();
        Self {
            program,
            args,
            proc: None,
        }
    }

    fn ensure(&mut self) -> Result<&mut Handles, String> {
        if self.proc.is_none() {
            use std::process::{Command, Stdio};
            let mut child = Command::new(&self.program)
                .args(&self.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| format!("failed to spawn '{}': {e}", self.program))?;
            let stdin = child.stdin.take().ok_or("child stdin unavailable")?;
            let stdout = child.stdout.take().ok_or("child stdout unavailable")?;
            self.proc = Some(Handles {
                child,
                stdin,
                stdout: std::io::BufReader::new(stdout),
            });
        }
        Ok(self.proc.as_mut().expect("just set"))
    }

    fn run(&mut self, action_id: &str, params: &Value) -> Result<ActionOutcome, String> {
        let h = self.ensure()?;
        match exchange(&mut h.stdin, &mut h.stdout, action_id, params) {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                // A broken pipe / dead handler means this child is unusable — drop it so the
                // next call respawns a fresh one.
                let _ = h.child.kill();
                self.proc = None;
                Err(e)
            }
        }
    }
}

/// One persistent-handler round-trip: write a request line, read exactly one non-blank
/// response line. Generic over the streams so the protocol is unit-testable without a real
/// process.
fn exchange<W: std::io::Write, R: std::io::BufRead>(
    stdin: &mut W,
    stdout: &mut R,
    action_id: &str,
    params: &Value,
) -> Result<ActionOutcome, String> {
    writeln!(stdin, "{}", request_line(action_id, params))
        .map_err(|e| format!("write to handler failed: {e}"))?;
    stdin
        .flush()
        .map_err(|e| format!("flush to handler failed: {e}"))?;

    let mut line = String::new();
    loop {
        line.clear();
        let n = stdout
            .read_line(&mut line)
            .map_err(|e| format!("read from handler failed: {e}"))?;
        if n == 0 {
            return Err("handler closed its output before replying".into());
        }
        if !line.trim().is_empty() {
            return parse_reply(line.trim());
        }
    }
}

impl ActionExecutor for PersistentProcessExecutor {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        self.run(action_id, params)
            .unwrap_or_else(ActionOutcome::failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_reply_maps_success_and_failure() {
        let ok = parse_reply(r#"{"success":true,"data":{"id":1}}"#).unwrap();
        assert!(ok.success);
        assert_eq!(ok.data, json!({"id": 1}));

        let bad = parse_reply(r#"{"success":false,"error":"nope"}"#).unwrap();
        assert!(!bad.success);
        assert_eq!(bad.error.as_deref(), Some("nope"));

        assert!(parse_reply("not json").is_err());
    }

    #[test]
    fn exchange_writes_request_and_reads_one_reply() {
        let mut sent: Vec<u8> = Vec::new();
        // Handler "output": a blank line (must be skipped) then the real reply.
        let mut out = std::io::Cursor::new("\n{\"success\":true,\"data\":42}\n");

        let outcome = exchange(&mut sent, &mut out, "categorize_txn", &json!({"id": 7})).unwrap();
        assert!(outcome.success);
        assert_eq!(outcome.data, json!(42));

        // The request line we wrote carries the action + params.
        let request = String::from_utf8(sent).unwrap();
        let v: serde_json::Value = serde_json::from_str(request.trim()).unwrap();
        assert_eq!(v["action"], "categorize_txn");
        assert_eq!(v["params"]["id"], 7);
    }

    #[test]
    fn exchange_errors_when_handler_closes_without_replying() {
        let mut sent: Vec<u8> = Vec::new();
        let mut out = std::io::Cursor::new(""); // immediate EOF
        let err = exchange(&mut sent, &mut out, "a", &json!({})).unwrap_err();
        assert!(err.contains("closed"), "got: {err}");
    }
}
