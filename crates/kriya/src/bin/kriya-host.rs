//! `kriya-host` — run the agent loop as a **standalone sidecar process** an app's main process
//! talks to over stdio (roadmap R3). This is what lets Electron and plain Node apps host the
//! kriya runtime — and the safety layer (policy/approval/budget/signed-audit) — in a process
//! the renderer can't tamper with, instead of only inside a Tauri backend.
//!
//! The wire protocol (newline-delimited JSON in both directions) is documented in
//! `kriya::sidecar`; the `kriya-sidecar` npm package is the Node client.
//!
//! Usage:
//!   kriya-host [--policy <policy.yaml>] [--script <script.json>] [--audit-log <path>]
//!
//!   --policy    YAML permission policy (default: safe built-in)
//!   --script    a JSON array of decisions to replay deterministically (no LLM, no API key) —
//!               the zero-config default backend for demos/CI. Without it, the backend is
//!               selected from AGENT_BACKEND (claude-cli | ollama | anthropic), defaulting to
//!               claude-cli. An explicit AGENT_BACKEND always wins over --script.
//!   --audit-log path for the signed-receipt JSONL log (default: $TMPDIR/kriya-audit.jsonl)

use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, Mutex};

use kriya::agent::inference::{claude_cli::ClaudeCli, scripted::ScriptedPlanner};
use kriya::audit::Signer;
use kriya::permissions::Policy;
use kriya::{run_sidecar, select_backend_with_default, Inference, SharedWriter};

struct Args {
    policy: Option<PathBuf>,
    script: Option<PathBuf>,
    audit_log: Option<PathBuf>,
}

fn usage_and_exit(msg: &str) -> ! {
    eprintln!("kriya-host: {msg}");
    eprintln!(
        "usage: kriya-host [--policy <policy.yaml>] [--script <script.json>] [--audit-log <path>]"
    );
    exit(2);
}

fn parse_args() -> Args {
    let mut policy = None;
    let mut script = None;
    let mut audit_log = None;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut take = |label: &str| -> String {
            it.next().unwrap_or_else(|| usage_and_exit(&format!("{label} needs a value")))
        };
        match flag.as_str() {
            "--policy" => policy = Some(PathBuf::from(take("--policy"))),
            "--script" => script = Some(PathBuf::from(take("--script"))),
            "--audit-log" => audit_log = Some(PathBuf::from(take("--audit-log"))),
            "-h" | "--help" => usage_and_exit("help"),
            other => usage_and_exit(&format!("unknown argument: {other}")),
        }
    }
    Args { policy, script, audit_log }
}

fn main() -> std::io::Result<()> {
    let args = parse_args();

    let policy = match &args.policy {
        Some(p) => Policy::load_or_default(p),
        None => Policy::default(),
    };
    let signer = Arc::new(match &args.audit_log {
        Some(p) => Signer::with_log_path(p.clone()),
        None => Signer::new(),
    });

    // Validate the script once up front so a typo fails loudly at startup, not mid-run.
    if let Some(path) = &args.script {
        if let Err(e) = ScriptedPlanner::from_file(path) {
            usage_and_exit(&format!("{e}"));
        }
    }

    let backend_mode = match (&args.script, std::env::var("AGENT_BACKEND").ok()) {
        (_, Some(b)) if !b.is_empty() => format!("env AGENT_BACKEND={b}"),
        (Some(p), _) => format!("scripted ({})", p.display()),
        (None, _) => "claude-cli (default)".to_string(),
    };
    eprintln!(
        "[kriya-host] sidecar ready · backend={} · audit log={}",
        backend_mode,
        signer.log_path().display()
    );

    // Fresh backend per run. An explicit AGENT_BACKEND wins; otherwise --script, else claude-cli.
    let script = args.script.clone();
    let make_backend = move || -> Box<dyn Inference> {
        let default: Box<dyn Inference> = match &script {
            Some(path) => match ScriptedPlanner::from_file(Path::new(path)) {
                Ok(p) => Box::new(p),
                Err(e) => {
                    eprintln!("[kriya-host] script reload failed: {e}");
                    Box::new(ClaudeCli::new())
                }
            },
            None => Box::new(ClaudeCli::new()),
        };
        select_backend_with_default(default)
    };

    let out: SharedWriter = Arc::new(Mutex::new(std::io::stdout()));
    let stdin = std::io::stdin();
    run_sidecar(stdin.lock(), out, Arc::new(policy), signer, make_backend)
}
