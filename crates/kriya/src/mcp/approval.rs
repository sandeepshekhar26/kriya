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
/// Any failure to reach a tty (no terminal, EOF, non-unix) is treated as a denial, and an
/// unanswered prompt times out — also a denial — after 300s (matches `GuiApproval`'s own bound).
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

/// Prompt a human via a native macOS dialog (`osascript`). Unlike {@link TtyApproval}, this
/// works even when the MCP server is a child of a TUI host (e.g. Claude Code) that owns the
/// controlling terminal — the dialog is drawn by the window server, out-of-band from any tty.
/// Any failure to show the dialog, a cancel, or a timeout is treated as a denial.
#[cfg(target_os = "macos")]
pub struct GuiApproval;

#[cfg(target_os = "macos")]
impl ApprovalGate for GuiApproval {
    fn request(&self, action_id: &str, params: &Value) -> bool {
        prompt_via_osascript(action_id, params).unwrap_or(false)
    }
}

#[cfg(target_os = "macos")]
fn prompt_via_osascript(action_id: &str, params: &Value) -> std::io::Result<bool> {
    use std::process::Command;

    let body = format!(
        "An external agent wants to run a guarded action:\n\naction: {action_id}\nparams: {params}"
    );
    // Deny is both default and cancel button, so Esc / dismiss also denies. `giving up after`
    // bounds the wait so an unattended host can't hang forever on a held action.
    let script = format!(
        "display dialog {body} with title \"kriya — approval required\" \
         buttons {{\"Deny\", \"Approve\"}} default button \"Deny\" cancel button \"Deny\" \
         with icon caution giving up after 300",
        body = applescript_string(&body),
    );

    let output = Command::new("osascript").arg("-e").arg(script).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // osascript echoes `button returned:Approve` only when Approve was clicked; a cancel exits
    // non-zero with empty stdout, and a time-out yields `gave up:true` — both deny.
    Ok(stdout.contains("button returned:Approve"))
}

/// Render a Rust string as an AppleScript string literal (quote it, escape `\`, `"`, newline).
/// osascript receives this as source, so only AppleScript escaping is needed — no shell quoting,
/// since {@link std::process::Command} passes the arg without a shell.
#[cfg(target_os = "macos")]
fn applescript_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Matches `prompt_via_osascript`'s own `giving up after 300` bound. A caller of `ApprovalGate`
/// may itself be under an external timeout shorter than an indefinite wait (e.g. Claude Code's
/// hook runner, which fails a killed/timed-out hook **open** — see `kriya-hook`'s module doc) —
/// self-bounding here means an unanswered prompt denies itself well inside that ceiling instead
/// of leaving the decision to whichever side gives up first.
#[cfg(unix)]
const TTY_APPROVAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[cfg(unix)]
fn prompt_on_tty(action_id: &str, params: &Value) -> std::io::Result<bool> {
    use std::fs::OpenOptions;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;

    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let mut writer = tty.try_clone()?;
    write!(
        writer,
        "\n[kriya] APPROVAL REQUIRED — an external agent wants to run a guarded action:\n  action: {action_id}\n  params: {params}\n[kriya] approve? [y/N] (times out after {}s): ",
        TTY_APPROVAL_TIMEOUT.as_secs()
    )?;
    writer.flush()?;

    // `read_line` has no native timeout, so read on a dedicated thread and race it against the
    // deadline. On timeout the reader thread is left blocked on the read — harmless, since the
    // process exits shortly after this function returns either way (the `pre` hook has nothing
    // left to do once the decision is made).
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(tty).read_line(&mut line).map(|_| line);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(TTY_APPROVAL_TIMEOUT) {
        Ok(Ok(line)) => {
            let answer = line.trim();
            Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
        }
        Ok(Err(e)) => Err(e),
        // A timeout is a denial, not an IO error — the same outcome `prompt_via_osascript`
        // produces on its own `giving up after 300` (`gave up:true` → deny), so both interactive
        // gates fail the same direction on an unanswered prompt.
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(false),
        Err(mpsc::RecvTimeoutError::Disconnected) => Ok(false),
    }
}
