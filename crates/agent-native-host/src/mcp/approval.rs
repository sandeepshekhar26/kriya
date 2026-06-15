//! Human-in-the-loop approval for actions a policy marks `RequiresApproval`, in MCP mode.
//!
//! The in-process host routes approval to a Tauri modal (a human at the app). An external
//! agent driving over stdio has no such UI, so approval is a trait with a few built-ins.
//! Default posture is **deny** — a guarded action with no one to approve it must not slip
//! through just because the requester is a remote agent.

use serde_json::Value;

/// Decides whether a policy-guarded action may proceed. Called only for actions the policy
/// returned `RequiresApproval` for; `Allow`/`Deny` never reach here.
pub trait ApprovalGate: Send {
    fn request(&self, action_id: &str, params: &Value) -> bool;
}

/// Safe default: deny everything that needs approval. With no interactive operator, a
/// guarded action is held rather than waved through.
pub struct DenyApproval;

impl ApprovalGate for DenyApproval {
    fn request(&self, _action_id: &str, _params: &Value) -> bool {
        false
    }
}

/// Approve everything that needs approval. For tests and explicitly-trusted deployments
/// only — using this in production defeats the approval gate.
pub struct AutoApprove;

impl ApprovalGate for AutoApprove {
    fn request(&self, _action_id: &str, _params: &Value) -> bool {
        true
    }
}

/// Prompt a human on the controlling terminal and wait for a y/n. Opens `/dev/tty`
/// directly rather than reading stdin, because in stdio transport stdin carries the
/// JSON-RPC stream — so the operator answers out-of-band from the agent's traffic.
/// Any failure to reach a tty (no terminal, EOF, non-unix) is treated as a denial.
pub struct TtyApproval;

impl ApprovalGate for TtyApproval {
    fn request(&self, action_id: &str, params: &Value) -> bool {
        #[cfg(unix)]
        {
            prompt_on_tty(action_id, params).unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = (action_id, params);
            false
        }
    }
}

#[cfg(unix)]
fn prompt_on_tty(action_id: &str, params: &Value) -> std::io::Result<bool> {
    use std::fs::OpenOptions;
    use std::io::{BufRead, BufReader, Write};

    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let mut writer = tty.try_clone()?;
    write!(
        writer,
        "\n[verb] APPROVAL REQUIRED — an external agent wants to run a guarded action:\n  action: {action_id}\n  params: {params}\n[verb] approve? [y/N]: "
    )?;
    writer.flush()?;

    let mut line = String::new();
    BufReader::new(tty).read_line(&mut line)?;
    let answer = line.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}
