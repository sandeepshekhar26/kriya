//! `kriya-mcp` — expose a kriya app's actions as a **governed** MCP server over stdio.
//!
//! An external agent (Claude Desktop, Cursor, …) speaks MCP to this binary; every
//! `tools/call` is routed through the app's policy → approval → budget → signed-audit
//! gates before — and only if — it reaches the app. This is the bolt-on that turns kriya
//! from a rewrite into an add-on: point it at a tool-schema file and an action handler and
//! the app gains a governed agent surface without touching its own code.
//!
//! Usage:
//!   kriya-mcp --tools <schemas.json> [--policy <policy.yaml>] [--exec "<cmd>"]
//!            [--approval deny|tty|gui|auto] [--name <name>] [--actor <agent>] [--user <user>]
//!
//!   --tools     JSON array from the SDK's getToolSchemas() (required)
//!   --policy    YAML permission policy (default: safe built-in — create/edit allow,
//!               delete requires approval, everything else denied)
//!   --exec      command run per cleared action; it reads {"action","params"} on stdin and
//!               writes {"success","data","error"} on stdout. Omit for discovery-only.
//!   --approval  how guarded actions are decided: deny (default), tty (prompt a human on
//!               the terminal), gui (native macOS dialog — works under a TUI host like Claude
//!               Code), or auto (approve — trusted/testing only)
//!   --name      server name reported in `initialize` (default: kriya-mcp)
//!   --actor     identity of the agent/client driving the server — stamped into every signed
//!               receipt's `actor` field (R8). Omit to leave receipts unattributed.
//!   --user      operator identity the run acts for (default: $USER). Only used with --actor.
//!   --audit-log path for the signed-receipt JSONL log (default: $TMPDIR/kriya-audit.jsonl)

use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;

use kriya::audit::{Actor, Signer};
#[cfg(target_os = "macos")]
use kriya::mcp::GuiApproval;
use kriya::mcp::{
    ActionExecutor, ActionOutcome, ApprovalGate, AutoApprove, DenyApproval, FnExecutor, Governor,
    PersistentProcessExecutor, ProcessExecutor, Server, TtyApproval,
};
use kriya::permissions::Policy;
use kriya::protocol::ToolSchema;

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Args {
    tools: PathBuf,
    policy: Option<PathBuf>,
    exec: Option<String>,
    approval: String,
    name: String,
    persistent: bool,
    actor: Option<String>,
    user: Option<String>,
    audit_log: Option<PathBuf>,
}

fn usage_and_exit(msg: &str) -> ! {
    eprintln!("kriya-mcp: {msg}");
    eprintln!(
        "usage: kriya-mcp --tools <schemas.json> [--policy <policy.yaml>] [--exec \"<cmd>\"] \
         [--persistent] [--approval deny|tty|gui|auto] [--name <name>] \
         [--actor <agent>] [--user <user>] [--audit-log <path>]"
    );
    exit(2);
}

fn parse_args() -> Args {
    let mut tools: Option<PathBuf> = None;
    let mut policy: Option<PathBuf> = None;
    let mut exec: Option<String> = None;
    let mut approval = "deny".to_string();
    let mut name = "kriya-mcp".to_string();
    let mut persistent = false;
    let mut actor: Option<String> = None;
    let mut user: Option<String> = None;
    let mut audit_log: Option<PathBuf> = None;

    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        // Each flag takes one value; a missing value is a usage error.
        let mut take = |label: &str| -> String {
            it.next()
                .unwrap_or_else(|| usage_and_exit(&format!("{label} needs a value")))
        };
        match flag.as_str() {
            "--tools" => tools = Some(PathBuf::from(take("--tools"))),
            "--policy" => policy = Some(PathBuf::from(take("--policy"))),
            "--exec" => exec = Some(take("--exec")),
            "--approval" => approval = take("--approval"),
            "--name" => name = take("--name"),
            "--persistent" => persistent = true,
            "--actor" => actor = Some(take("--actor")),
            "--user" => user = Some(take("--user")),
            "--audit-log" => audit_log = Some(PathBuf::from(take("--audit-log"))),
            "-h" | "--help" => usage_and_exit("help"),
            other => usage_and_exit(&format!("unknown argument: {other}")),
        }
    }

    let Some(tools) = tools else {
        usage_and_exit("--tools <schemas.json> is required");
    };
    Args {
        tools,
        policy,
        exec,
        approval,
        name,
        persistent,
        actor,
        user,
        audit_log,
    }
}

/// Build the receipt actor (R8) from the binary's identity flags. Returns `None` when
/// no `--actor` was given, leaving receipts unattributed (backward compatible). When an
/// agent id is given, the operator defaults to `$USER`, then `"local"`.
fn build_actor(agent: Option<String>, user: Option<String>) -> Option<Actor> {
    let agent = agent?;
    let user = user
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("USER").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "local".to_string());
    Some(Actor::new(agent, user))
}

fn load_tools(path: &PathBuf) -> Vec<ToolSchema> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| usage_and_exit(&format!("cannot read --tools file {path:?}: {e}")));
    serde_json::from_str(&text).unwrap_or_else(|e| {
        usage_and_exit(&format!("--tools file is not a tool-schema array: {e}"))
    })
}

fn build_approval(kind: &str) -> Box<dyn ApprovalGate> {
    match kind {
        "deny" => Box::new(DenyApproval),
        "tty" => Box::new(TtyApproval),
        "auto" => Box::new(AutoApprove),
        // Native macOS approval dialog — works even when the server is a child of a TUI host
        // (e.g. Claude Code) that owns the controlling terminal.
        #[cfg(target_os = "macos")]
        "gui" => Box::new(GuiApproval),
        #[cfg(not(target_os = "macos"))]
        "gui" => usage_and_exit("--approval gui is only available on macOS"),
        other => usage_and_exit(&format!(
            "--approval must be deny|tty|gui|auto, got '{other}'"
        )),
    }
}

fn build_executor(exec: Option<String>, persistent: bool) -> Box<dyn ActionExecutor> {
    match exec {
        // Persistent: spawn the handler once and reuse it (for handlers holding an expensive
        // connection, e.g. Actual Budget). Per-call: spawn fresh each time (cheap/stateless).
        Some(cmd) if persistent => Box::new(PersistentProcessExecutor::new(&cmd)),
        Some(cmd) => Box::new(ProcessExecutor::new(&cmd)),
        // Discovery-only: tools/list works, but any call fails with a clear message.
        None => Box::new(FnExecutor(|_id: &str, _p: &serde_json::Value| {
            ActionOutcome::failed(
                "kriya-mcp started without --exec — discovery only, cannot run actions",
            )
        })),
    }
}

fn main() -> std::io::Result<()> {
    let args = parse_args();

    let schemas = load_tools(&args.tools);
    let policy = match &args.policy {
        Some(p) => Policy::load_or_default(p),
        None => Policy::default(),
    };
    let signer = Arc::new(match &args.audit_log {
        Some(p) => Signer::with_log_path(p.clone()),
        None => Signer::new(),
    });
    let approval = build_approval(&args.approval);
    let executor = build_executor(args.exec.clone(), args.persistent);

    let actor = build_actor(args.actor.clone(), args.user.clone());
    let governor = Governor::new(Arc::new(policy), signer.clone(), approval, executor)
        .with_actor(actor.clone());
    let mut server = Server::new(&args.name, SERVER_VERSION, schemas, governor);

    // Banner to stderr — stdout is reserved for the JSON-RPC stream.
    let exec_desc = match (&args.exec, args.persistent) {
        (Some(cmd), true) => format!("{cmd} (persistent)"),
        (Some(cmd), false) => format!("{cmd} (per-call)"),
        (None, _) => "<none: discovery only>".to_string(),
    };
    let actor_desc = match &actor {
        Some(a) => format!("{}/{}", a.agent, a.user),
        None => "<unattributed>".to_string(),
    };
    eprintln!(
        "[kriya-mcp] serving {} governed tool(s) · approval={} · actor={} · exec={} · audit log={}",
        server.tool_count(),
        args.approval,
        actor_desc,
        exec_desc,
        signer.log_path().display()
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    server.serve(stdin.lock(), &mut out)
}
