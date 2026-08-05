//! `kriya-run` — the **governed launcher** (kriya-console F-5, doc 31 §3.5 / doc 33 §4-B).
//!
//! `kriya run --pack <p> [--cwd <dir>] [--lane egress] [--lane contain] [--shift] -- <agent-cmd…>`
//!
//! One command starts any agent pre-wired to a policy pack: it signs ONE `kriya.run.launched`
//! receipt (the launch attestation — *this agent was started under this pack + these lanes*), then
//! execs the agent command. The signed launch lands in the Console's Session Timeline with its pack
//! chip; per-call governance then flows through the agent's own hook (Claude Code) or the in-process
//! govern+sign middleware (`kriya-govern`, the doc-21 engine) — kriya-run is the composer + launch
//! record, **not a second enforcement path** (the same law kriya-console's F-2 gate engine keeps:
//! policy authoring + receipt vocabulary, never a parallel enforcer).
//!
//! **The one-Signer law (design law 3).** This binary reimplements no trust primitive: it reuses the
//! runtime `Signer` (Ed25519 + hash chain) exactly as `kriya-govern` / `kriya-hook` / `kriya-mcp` do.
//! The launch receipt is byte-identical to every other kriya receipt — the Console re-verifies it.
//!
//! **Honest v1 (copy-first, the recorded v3 decision).** kriya-run records the launch + execs; it
//! passes the pack + audit-log path to the child via `KRIYA_PACK` / `KRIYA_AUDIT_LOG` env so
//! downstream wiring can read them, but it does NOT itself intercept the launched agent's tool calls
//! — that is the hook's / gateway's / containment lane's job. The `--lane` choices are RECORDED
//! intent in this version (the Console surfaces them); wiring the containment lane end-to-end is the
//! feature-gated `kriya-gateway run` path, not this launcher.
//!
//! Content-free by construction: the receipt carries `{v, agent, pack, lanes, shift}` — never the
//! agent command's argv (which could carry content).
//!
//! Usage:
//!   kriya-run --pack <id> [--cwd <dir>] [--lane egress|contain]… [--shift] [--agent <name>]
//!             [--actor <agent>] [--user <user>] [--audit-log <path>] [--dry-run] -- <cmd> [args…]
//!
//!   --pack       the policy pack the launched agent runs under (recorded on the launch receipt).
//!   --cwd        working directory for the launched command (default: inherit).
//!   --lane       a governed lane to record: `egress` and/or `contain` (repeatable).
//!   --shift      the run reports to a signed shift (F-6) — FORCED on for unattended/cron runs.
//!   --agent      the agent identity (e.g. `claude-code`) — recorded as `agent` + the receipt actor.
//!   --actor      override the receipt actor's agent string (default: `--agent` or `kriya-run`).
//!   --user       operator identity (default: $USER).
//!   --audit-log  signed-receipt JSONL path (default: $TMPDIR/kriya-audit.jsonl). Point it at
//!                ~/.kriya/audit/<name>.jsonl for the Console to tail + re-verify it.
//!   --dry-run    sign + print the plan, do NOT exec the agent command (composer/testing).

use std::path::PathBuf;
use std::process::{exit, Command};

use kriya::audit::{Actor, Receipt, Signer};
use serde_json::{json, Value};

/// The `kriya.run.launched` action id — the launch attestation vocabulary (OT-1: registered in the
/// Console's `src/lib/otel/registry.ts` + `semconv.ts`, parity-fixtured in `run_launched_fixture.rs`).
const ACTION_LAUNCHED: &str = "kriya.run.launched";

/// The two lanes the launcher records in v1 (the Console's LaunchView offers exactly these).
const KNOWN_LANES: [&str; 2] = ["egress", "contain"];

/// A fully-parsed launch plan. Kept as a plain struct (no I/O) so parsing is unit-testable.
#[derive(Debug, PartialEq)]
struct RunPlan {
    pack: String,
    cwd: Option<String>,
    lanes: Vec<String>,
    shift: bool,
    agent: Option<String>,
    actor: Option<String>,
    user: Option<String>,
    audit_log: Option<PathBuf>,
    dry_run: bool,
    /// The agent command after `--`. Empty is allowed only with `--dry-run` (compose-only).
    cmd: Vec<String>,
}

fn usage() -> &'static str {
    "usage: kriya-run --pack <id> [--cwd <dir>] [--lane egress|contain]… [--shift] \
     [--agent <name>] [--actor <agent>] [--user <user>] [--audit-log <path>] [--dry-run] \
     -- <cmd> [args…]"
}

/// Parse the launcher's own flags up to `--`, then everything after `--` is the agent command.
/// Returns a human error string on any misuse (no process exit here — the caller decides).
fn parse_run_args(args: Vec<String>) -> Result<RunPlan, String> {
    let mut pack: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut lanes: Vec<String> = Vec::new();
    let mut shift = false;
    let mut agent: Option<String> = None;
    let mut actor: Option<String> = None;
    let mut user: Option<String> = None;
    let mut audit_log: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut cmd: Vec<String> = Vec::new();

    let mut it = args.into_iter();
    let mut saw_dashdash = false;
    while let Some(flag) = it.next() {
        if saw_dashdash {
            cmd.push(flag);
            continue;
        }
        let mut take = |label: &str| -> Result<String, String> {
            it.next().ok_or_else(|| format!("{label} needs a value"))
        };
        match flag.as_str() {
            "--" => saw_dashdash = true,
            "--pack" => pack = Some(take("--pack")?),
            "--cwd" => cwd = Some(take("--cwd")?),
            "--lane" => {
                let lane = take("--lane")?;
                if !KNOWN_LANES.contains(&lane.as_str()) {
                    return Err(format!("--lane must be egress|contain, got '{lane}'"));
                }
                if !lanes.contains(&lane) {
                    lanes.push(lane);
                }
            }
            "--shift" => shift = true,
            "--agent" => agent = Some(take("--agent")?),
            "--actor" => actor = Some(take("--actor")?),
            "--user" => user = Some(take("--user")?),
            "--audit-log" => audit_log = Some(PathBuf::from(take("--audit-log")?)),
            "--dry-run" => dry_run = true,
            "-h" | "--help" => return Err("help".to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let pack = pack.ok_or_else(|| "--pack is required".to_string())?;
    if pack.trim().is_empty() {
        return Err("--pack must not be empty".to_string());
    }
    if cmd.is_empty() && !dry_run {
        return Err("no agent command — put it after `--` (or use --dry-run to compose only)".to_string());
    }
    // Canonical lane order (egress before contain) so the receipt is deterministic regardless of
    // the flag order the composer emitted.
    lanes.sort_by_key(|l| KNOWN_LANES.iter().position(|k| k == l).unwrap_or(usize::MAX));

    Ok(RunPlan {
        pack,
        cwd,
        lanes,
        shift,
        agent,
        actor,
        user,
        audit_log,
        dry_run,
        cmd,
    })
}

/// The content-free `kriya.run.launched` receipt params: `{v, agent, pack, lanes, shift}`.
/// Never the agent command argv (F-4/C1 zero-content discipline).
fn launch_params(plan: &RunPlan) -> Value {
    json!({
        "v": 1,
        "agent": plan.agent.clone().unwrap_or_else(|| "unknown".to_string()),
        "pack": plan.pack,
        "lanes": plan.lanes,
        "shift": plan.shift,
    })
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Build the receipt actor (R8) — `--actor` overrides `--agent`; `None` for neither leaves the
/// launch attributed to `kriya-run` so the launch is never unattributed (a launch has an operator).
fn build_actor(plan: &RunPlan) -> Actor {
    let agent = plan
        .actor
        .clone()
        .or_else(|| plan.agent.clone())
        .unwrap_or_else(|| "kriya-run".to_string());
    let user = plan
        .user
        .clone()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "local".to_string());
    Actor::new(agent, user)
}

fn main() {
    let plan = match parse_run_args(std::env::args().skip(1).collect()) {
        Ok(p) => p,
        Err(msg) => {
            if msg == "help" {
                eprintln!("{}", usage());
                exit(0);
            }
            eprintln!("kriya-run: {msg}");
            eprintln!("{}", usage());
            exit(2);
        }
    };

    // Sign the launch attestation (the one-Signer path — byte-identical to every other receipt).
    let signer = match &plan.audit_log {
        Some(p) => Signer::with_log_path(p.clone()),
        None => Signer::new(),
    };
    let actor = build_actor(&plan);
    let step_id = uuid::Uuid::new_v4().to_string();
    let signed = signer.record(
        Receipt::new(
            step_id,
            ACTION_LAUNCHED.to_string(),
            launch_params(&plan),
            true,
            now_ms(),
        )
        .with_actor(Some(actor.clone())),
    );

    let lanes_desc = if plan.lanes.is_empty() {
        "none".to_string()
    } else {
        plan.lanes.join("+")
    };
    eprintln!(
        "[kriya-run] launch signed · pack={} · lanes={} · shift={} · actor={}/{} · audit log={}",
        plan.pack,
        lanes_desc,
        plan.shift,
        actor.agent,
        actor.user,
        signer.log_path().display()
    );

    if plan.dry_run || plan.cmd.is_empty() {
        // Compose-only: print the signed line so a caller can inspect it, then stop.
        match serde_json::to_string(&signed) {
            Ok(line) => println!("{line}"),
            Err(e) => {
                eprintln!("kriya-run: could not serialize launch receipt: {e}");
                exit(1);
            }
        }
        return;
    }

    // Exec the agent command, passing the pack + audit-log path through the environment so
    // downstream wiring (the agent's hook) can pick them up. kriya-run itself does not intercept
    // the launched agent's calls — honest v1 (stated in the module doc + the Console UI).
    let program = plan.cmd[0].clone();
    let mut command = Command::new(&program);
    command.args(&plan.cmd[1..]);
    command.env("KRIYA_PACK", &plan.pack);
    command.env("KRIYA_AUDIT_LOG", signer.log_path());
    if plan.shift {
        command.env("KRIYA_SHIFT", "1");
    }
    if let Some(dir) = &plan.cwd {
        command.current_dir(dir);
    }
    match command.status() {
        Ok(status) => exit(status.code().unwrap_or(0)),
        Err(e) => {
            eprintln!("kriya-run: could not exec '{program}': {e}");
            exit(127);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Result<RunPlan, String> {
        parse_run_args(argv.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn minimal_claude_launch_parses() {
        let p = parse(&["--pack", "developer", "--", "claude"]).unwrap();
        assert_eq!(p.pack, "developer");
        assert_eq!(p.cmd, vec!["claude"]);
        assert!(p.lanes.is_empty());
        assert!(!p.shift);
    }

    #[test]
    fn lanes_are_deduped_and_canonically_ordered() {
        // contain given before egress, and egress twice — canonical order is egress, contain, once.
        let p = parse(&[
            "--pack", "developer", "--lane", "contain", "--lane", "egress", "--lane", "egress",
            "--", "claude",
        ])
        .unwrap();
        assert_eq!(p.lanes, vec!["egress", "contain"]);
    }

    #[test]
    fn unknown_lane_is_rejected() {
        let err = parse(&["--pack", "developer", "--lane", "wormhole", "--", "claude"]).unwrap_err();
        assert!(err.contains("egress|contain"), "{err}");
    }

    #[test]
    fn command_after_dashdash_is_captured_verbatim_including_flags() {
        // Flags AFTER `--` belong to the agent, not the launcher.
        let p = parse(&[
            "--pack", "analyst", "--", "python", "agent.py", "--pack", "not-mine",
        ])
        .unwrap();
        assert_eq!(p.cmd, vec!["python", "agent.py", "--pack", "not-mine"]);
        assert_eq!(p.pack, "analyst", "the launcher's --pack is only read before --");
    }

    #[test]
    fn pack_is_required() {
        assert!(parse(&["--", "claude"]).unwrap_err().contains("--pack is required"));
        assert!(parse(&["--pack", "  ", "--", "claude"]).unwrap_err().contains("must not be empty"));
    }

    #[test]
    fn missing_command_is_an_error_unless_dry_run() {
        assert!(parse(&["--pack", "developer"]).unwrap_err().contains("no agent command"));
        // --dry-run allows an empty command (compose-only).
        let p = parse(&["--pack", "developer", "--dry-run"]).unwrap();
        assert!(p.dry_run && p.cmd.is_empty());
    }

    #[test]
    fn cron_style_forces_shift_and_records_it() {
        let p = parse(&["--pack", "planner", "--shift", "--agent", "cron", "--", "claude"]).unwrap();
        assert!(p.shift);
        let params = launch_params(&p);
        assert_eq!(params["shift"], true);
        assert_eq!(params["agent"], "cron");
    }

    #[test]
    fn launch_params_are_content_free_shape() {
        let p = parse(&[
            "--pack", "developer", "--lane", "egress", "--agent", "claude-code",
            "--cwd", "/secret/project", "--", "claude", "--dangerous",
        ])
        .unwrap();
        let params = launch_params(&p);
        assert_eq!(params["v"], 1);
        assert_eq!(params["agent"], "claude-code");
        assert_eq!(params["pack"], "developer");
        assert_eq!(params["lanes"], json!(["egress"]));
        assert_eq!(params["shift"], false);
        // The cwd path and the agent argv are NOT in the receipt (content-free by construction).
        let s = params.to_string();
        assert!(!s.contains("/secret/project"), "cwd must not leak into the receipt");
        assert!(!s.contains("dangerous"), "agent argv must not leak into the receipt");
    }

    #[test]
    fn actor_defaults_to_agent_then_to_kriya_run() {
        let p1 = parse(&["--pack", "developer", "--agent", "claude-code", "--", "claude"]).unwrap();
        assert_eq!(build_actor(&p1).agent, "claude-code");
        let p2 = parse(&["--pack", "developer", "--", "claude"]).unwrap();
        assert_eq!(build_actor(&p2).agent, "kriya-run");
        let p3 = parse(&["--pack", "developer", "--agent", "cron", "--actor", "ops-bot", "--", "claude"]).unwrap();
        assert_eq!(build_actor(&p3).agent, "ops-bot", "--actor overrides --agent");
    }
}
