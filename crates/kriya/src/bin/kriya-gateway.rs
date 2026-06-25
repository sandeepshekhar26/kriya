//! `kriya-gateway` — the shippable governance product (D-016). A binary an MCP client (Claude
//! Desktop, Cursor, Claude Code) launches **instead of** the real MCP server. It spawns the real
//! ("downstream") server as a subprocess, governs every `tools/call` through the same
//! policy → approval → budget → Ed25519-signed-audit core the in-process host enforces, and is
//! otherwise transparent — **zero changes to the downstream server or the app.**
//!
//! Drops straight into a client's MCP config with no integration code:
//! ```jsonc
//! { "mcpServers": { "actual-budget": {
//!     "command": "kriya-gateway",
//!     "args": ["proxy", "--", "node", "actual-mcp-server.js"]
//! } } }
//! ```
//!
//! ## Usage
//! ```text
//! kriya-gateway proxy [OPTIONS] -- <downstream-cmd> [args...]
//!
//!   --config <p.yaml>     load gateway settings from a `.kriya.yaml` config file (see below)
//!   --policy <p.yaml>     permission policy (default: built-in deny-by-default — reads allow,
//!                         destructive/spend names require approval, everything else denied)
//!   --approval <mode>     how guarded calls are decided: deny (default) | tty | gui | auto
//!   --actor <agent>       agent identity stamped into every signed receipt (R8). Omit → unattributed
//!   --user <user>         operator the run acts for (default: $USER). Only used with --actor
//!   --audit-log <path>    signed-receipt JSONL log (default: $TMPDIR/kriya-audit.jsonl)
//!   --signing-key <path>  persist the Ed25519 identity here (0600) for a STABLE trust anchor
//!                         across runs (R20). Requires --audit-log. Default: ephemeral per-process key
//!   --name <n>            server name reported to the client in `initialize` (default: kriya-gateway)
//!   -- <downstream-cmd>   EVERYTHING after `--` is the downstream MCP server command + its args
//! ```
//! `kriya-gateway serve ...` delegates to the existing `kriya-mcp` bolt-on (point the client at the
//! `kriya-mcp` binary directly for that mode).
//!
//! ## Config file discovery (R24)
//! Before falling back to built-in defaults the gateway looks for a config file: `--config <path>`
//! if given, else `./.kriya.yaml`, else `./kriya.yaml` in the current working directory. It is a
//! small YAML map with all-optional keys — `policy`, `approval`, `audit_log`, `signing_key`,
//! `name`, `actor`, `user` — letting a project pin its governance posture without a long command
//! line. **Precedence: an explicit CLI flag wins; the config file fills anything not passed on the
//! command line; the built-in defaults apply to anything neither sets.** A missing config file is
//! silently ignored (defaults apply); a malformed one is a clean startup error.
//!
//! ## On-startup on-device attestation (R24 / R13)
//! When the gateway runs with a **persisted** signing key (`--signing-key` or the config's
//! `signing_key`) — the durable-identity case — it writes a signed on-device attestation receipt as
//! the FIRST line of the audit log before serving, using the same `kriya.attestation.on_device`
//! mechanism the in-process host emits (R13). The log therefore opens with an offline-verifiable
//! statement that this gateway session ran on-device under a pinned key, ahead of any action
//! receipt. With an ephemeral per-process key the attestation is skipped (it would not be
//! meaningfully verifiable across runs); a one-line stderr note records the decision either way.
//!
//! Banner + per-call decisions go to **stderr**; stdout is reserved for the MCP JSON-RPC stream.

use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, Mutex};

use kriya::audit::{now_ms, Actor, Receipt, Signer, ATTESTATION_ON_DEVICE};
#[cfg(target_os = "macos")]
use kriya::mcp::GuiApproval;
use kriya::mcp::{
    ApprovalGate, AutoApprove, DenyApproval, Governor, McpClient, McpProxyExecutor, ProxyServer,
    TtyApproval,
};
// Front 2 (reach-in) types — only when built with `--features reach-in`. The real driver is macOS.
#[cfg(all(feature = "reach-in", target_os = "macos"))]
use kriya::mcp::MacAxBackend;
#[cfg(feature = "reach-in")]
use kriya::mcp::{AxBackend, AxExecutor, ReachInServer};
// Front 3 (computer-use) types — only when built with `--features computer-use`. macOS backend.
#[cfg(all(feature = "computer-use", target_os = "macos"))]
use kriya::mcp::MacDesktopBackend;
#[cfg(feature = "computer-use")]
use kriya::mcp::{ComputerUseExecutor, ComputerUseServer, DesktopBackend};
use kriya::permissions::{default_proxy_policy, Policy};
use serde::Deserialize;

struct ProxyArgs {
    policy: Option<PathBuf>,
    approval: String,
    name: String,
    actor: Option<String>,
    user: Option<String>,
    audit_log: Option<PathBuf>,
    signing_key: Option<PathBuf>,
    /// The downstream command + args (everything after `--`). `[0]` is the program.
    downstream: Vec<String>,
}

/// Settings loaded from a `.kriya.yaml` config file (R24). Every field is **optional** — the file
/// only supplies values the operator didn't pass on the command line, and anything it omits keeps
/// the built-in default. Fields mirror the CLI flags so a project can pin its governance posture
/// once in a checked-in file instead of repeating a long command line in every MCP client config.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GatewayConfig {
    policy: Option<PathBuf>,
    approval: Option<String>,
    audit_log: Option<PathBuf>,
    signing_key: Option<PathBuf>,
    name: Option<String>,
    actor: Option<String>,
    user: Option<String>,
}

/// The config files the gateway auto-discovers in the current working directory, in order, when no
/// explicit `--config` is given. The first that exists wins; if none exist, built-in defaults apply.
const CONFIG_AUTODISCOVER: &[&str] = &[".kriya.yaml", "kriya.yaml"];

/// Resolve and parse the gateway config (R24): an explicit `--config <path>` if given, else the
/// first auto-discovered `.kriya.yaml`/`kriya.yaml` in the cwd. Returns the parsed config plus the
/// path it came from (for the stderr note), or `None` when no file applies. An explicit `--config`
/// that doesn't exist, or any file that fails to parse, is a clean startup error (non-zero exit) —
/// a config the operator asked for must never be silently ignored.
fn load_gateway_config(explicit: &Option<PathBuf>) -> Option<(GatewayConfig, PathBuf)> {
    let path = match explicit {
        Some(p) => {
            if !p.exists() {
                usage_and_exit(&format!("--config {p:?} does not exist"));
            }
            p.clone()
        }
        // No config file at all is the common case — `?` returns None, defaults apply, nothing to log.
        None => CONFIG_AUTODISCOVER
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())?,
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_yaml::from_str::<GatewayConfig>(&text) {
            Ok(cfg) => Some((cfg, path)),
            Err(e) => usage_and_exit(&format!("config file {path:?} is malformed: {e}")),
        },
        Err(e) => usage_and_exit(&format!("cannot read config file {path:?}: {e}")),
    }
}

/// Apply config-file values to anything the operator did NOT pass on the command line (R24).
/// Precedence is **CLI flag > config file > built-in default**: each `get_or_insert_with` only
/// fills a `None` (an unset flag), so an explicit CLI value always wins. `approval` defaults to a
/// sentinel set by the flag parser, so it is treated as "unset" only when still the default.
fn apply_config(args: &mut ProxyArgs, approval_from_cli: bool, cfg: GatewayConfig) {
    if args.policy.is_none() {
        args.policy = cfg.policy;
    }
    if !approval_from_cli {
        if let Some(a) = cfg.approval {
            args.approval = a;
        }
    }
    if args.audit_log.is_none() {
        args.audit_log = cfg.audit_log;
    }
    if args.signing_key.is_none() {
        args.signing_key = cfg.signing_key;
    }
    if args.actor.is_none() {
        args.actor = cfg.actor;
    }
    if args.user.is_none() {
        args.user = cfg.user;
    }
    // `name` has a built-in default ("kriya-gateway"); only override it if the operator didn't pass
    // --name. Tracked by comparing against that default since the flag parser seeds it eagerly.
    if args.name == "kriya-gateway" {
        if let Some(n) = cfg.name {
            args.name = n;
        }
    }
}

fn usage_and_exit(msg: &str) -> ! {
    eprintln!("kriya-gateway: {msg}");
    eprintln!(
        "usage: kriya-gateway proxy [--policy <p.yaml>] [--approval deny|tty|gui|auto] \
         [--actor <agent>] [--user <user>] [--audit-log <path>] [--signing-key <path>] \
         [--name <n>] -- <downstream-cmd> [args...]\n       \
         kriya-gateway reach-in --app \"<App Name>\" [same governance flags]\n       \
         kriya-gateway doctor [--app \"<App Name>\"]   (macOS preflight: Accessibility, bundle, snippet)\n       \
         kriya-gateway serve ...   (delegates to the kriya-mcp bolt-on)"
    );
    exit(2);
}

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let sub = args.next().unwrap_or_else(|| {
        usage_and_exit("a subcommand is required (proxy|reach-in|computer-use|router|serve|doctor)")
    });
    match sub.as_str() {
        "proxy" => run_proxy(parse_proxy_args(args)),
        // Front 2 — govern an app that has NO MCP server / NO API via its accessibility tree.
        "reach-in" => run_reachin(args),
        // Front 3 — governed computer-use (system-wide pixels). `router` is the unified entry: today
        // it serves the computer-use floor + the `list_apps` discovery tool (auto-tier per app is v2).
        "computer-use" | "router" => run_computer_use(args),
        // The existing in-process bolt-on lives in the `kriya-mcp` binary; keep one tool per
        // entry point rather than duplicating its wiring here.
        "serve" => usage_and_exit(
            "`serve` mode is the kriya-mcp bolt-on — run the `kriya-mcp` binary directly",
        ),
        // Operator preflight (R24): is Accessibility granted, are we in the .app bundle, what apps
        // can reach-in target, and the exact Claude Desktop config snippet. A human-run CLI tool,
        // so its output goes to STDOUT (no MCP stdio session to corrupt).
        "doctor" => run_doctor(args),
        "-h" | "--help" | "help" => usage_and_exit("help"),
        other => usage_and_exit(&format!(
            "unknown subcommand: {other} (expected proxy|reach-in|computer-use|router|serve|doctor)"
        )),
    }
}

fn parse_proxy_args(mut it: impl Iterator<Item = String>) -> ProxyArgs {
    let mut config: Option<PathBuf> = None;
    let mut policy: Option<PathBuf> = None;
    let mut approval = "deny".to_string();
    // Whether --approval was passed explicitly. `approval` has a non-None default ("deny"), so we
    // can't use Option to detect "unset" the way the path flags do; track it separately so a config
    // file can supply approval only when the operator left it off the command line (R24 precedence).
    let mut approval_from_cli = false;
    let mut name = "kriya-gateway".to_string();
    let mut actor: Option<String> = None;
    let mut user: Option<String> = None;
    let mut audit_log: Option<PathBuf> = None;
    let mut signing_key: Option<PathBuf> = None;
    let mut downstream: Vec<String> = Vec::new();

    while let Some(flag) = it.next() {
        // `--` ends option parsing: everything after it is the downstream command + args, passed
        // through untouched (so the downstream can have its own `--flags`).
        if flag == "--" {
            downstream = it.by_ref().collect();
            break;
        }
        let mut take = |label: &str| -> String {
            it.next()
                .unwrap_or_else(|| usage_and_exit(&format!("{label} needs a value")))
        };
        match flag.as_str() {
            "--config" => config = Some(PathBuf::from(take("--config"))),
            "--policy" => policy = Some(PathBuf::from(take("--policy"))),
            "--approval" => {
                approval = take("--approval");
                approval_from_cli = true;
            }
            "--name" => name = take("--name"),
            "--actor" => actor = Some(take("--actor")),
            "--user" => user = Some(take("--user")),
            "--audit-log" => audit_log = Some(PathBuf::from(take("--audit-log"))),
            "--signing-key" => signing_key = Some(PathBuf::from(take("--signing-key"))),
            "-h" | "--help" => usage_and_exit("help"),
            other => usage_and_exit(&format!("unknown argument: {other}")),
        }
    }

    if downstream.is_empty() {
        usage_and_exit(
            "no downstream command — pass it after `--`, e.g. `proxy -- node server.js`",
        );
    }
    let mut args = ProxyArgs {
        policy,
        approval,
        name,
        actor,
        user,
        audit_log,
        signing_key,
        downstream,
    };
    // R24: fold in a `.kriya.yaml` (explicit --config, else auto-discovered) for anything the
    // command line didn't set. CLI flag > config file > built-in default.
    if let Some((cfg, path)) = load_gateway_config(&config) {
        eprintln!("[kriya-gateway] loaded config: {}", path.display());
        apply_config(&mut args, approval_from_cli, cfg);
    }
    args
}

/// Build the receipt actor (R8) from the identity flags — same logic as kriya-mcp. `None` leaves
/// receipts unattributed; with `--actor`, the operator defaults to `$USER`, then `"local"`.
fn build_actor(agent: Option<String>, user: Option<String>) -> Option<Actor> {
    let agent = agent?;
    let user = user
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("USER").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "local".to_string());
    Some(Actor::new(agent, user))
}

/// Select the approval gate — the SAME deny/tty/gui/auto selection kriya-mcp uses.
fn build_approval(kind: &str) -> Box<dyn ApprovalGate> {
    match kind {
        "deny" => Box::new(DenyApproval),
        "tty" => Box::new(TtyApproval),
        "auto" => Box::new(AutoApprove),
        #[cfg(target_os = "macos")]
        "gui" => Box::new(GuiApproval),
        #[cfg(not(target_os = "macos"))]
        "gui" => usage_and_exit("--approval gui is only available on macOS"),
        other => usage_and_exit(&format!(
            "--approval must be deny|tty|gui|auto, got '{other}'"
        )),
    }
}

/// Build the signer, validating audit-log / signing-key paths up front so a bad path is a clean
/// startup error rather than a hard failure mid-session (service-architecture §7). A persisted
/// `--signing-key` gives a stable trust anchor across runs (R20); it requires `--audit-log`.
fn build_signer(audit_log: &Option<PathBuf>, signing_key: &Option<PathBuf>) -> Arc<Signer> {
    match (signing_key, audit_log) {
        (Some(key), Some(log)) => match Signer::with_identity(key, log.clone()) {
            Ok(s) => Arc::new(s),
            Err(e) => usage_and_exit(&format!("cannot use --signing-key {key:?}: {e}")),
        },
        (Some(_), None) => {
            usage_and_exit("--signing-key requires --audit-log (the persisted log it anchors)")
        }
        (None, Some(log)) => Arc::new(Signer::with_log_path(log.clone())),
        (None, None) => Arc::new(Signer::new()),
    }
}

/// Record an on-startup on-device attestation receipt (R24), reusing the exact R13 mechanism the
/// in-process host emits: a signed [`Receipt`] carrying the reserved [`ATTESTATION_ON_DEVICE`]
/// action id. It makes the audit log OPEN with an offline-verifiable statement that this gateway
/// session ran on-device under a pinned key, before any action receipt — so an auditor reading the
/// log can see the trust anchor was asserted up front, not retrofitted.
///
/// Gated on a **persisted** signing key (`durable_key`): an attestation signed by an ephemeral
/// per-process key can't be checked against a pinned public key across runs, so it would be
/// security theater — we skip it (with a stderr note) rather than write a receipt no auditor can
/// rely on. The gateway proxies a downstream subprocess rather than driving an inference backend,
/// so there is no model egress to seal here; the receipt records `egress: false` with a
/// `gateway-proxy` network profile so a verifier can tell a gateway attestation from a host's.
fn write_startup_attestation(signer: &Signer, durable_key: bool, actor: &Option<Actor>) {
    if !durable_key {
        eprintln!(
            "[kriya-gateway] ephemeral signing key — skipping on-device attestation (not \
             verifiable across runs; pass --signing-key for a durable trust anchor)"
        );
        return;
    }
    let attestation = signer.record(
        Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            ATTESTATION_ON_DEVICE.to_string(),
            serde_json::json!({
                "component": "kriya-gateway",
                "network_profile": "gateway-proxy",
                "egress": false,
            }),
            true,
            now_ms(),
        )
        .with_actor(actor.clone()),
    );
    eprintln!(
        "[kriya-gateway] on-device attestation written (pinned key {}…) · sig={}…",
        &signer.public_key()[..signer.public_key().len().min(16)],
        &attestation.signature[..attestation.signature.len().min(16)]
    );
}

fn run_proxy(args: ProxyArgs) -> std::io::Result<()> {
    // Default to the zero-config deny-by-default policy when no file is given (the product path).
    let policy = Arc::new(match &args.policy {
        Some(p) => Policy::load_or_default(p),
        None => default_proxy_policy(),
    });
    // Surface obviously-dangerous policy configurations to the operator (stderr).
    for w in policy.warnings() {
        eprintln!("[kriya-gateway] policy warning: {w}");
    }

    let signer = build_signer(&args.audit_log, &args.signing_key);
    let approval = build_approval(&args.approval);
    let actor = build_actor(args.actor.clone(), args.user.clone());

    // R24 / R13: open the audit log with a signed on-device attestation when a durable signing key
    // is in use (the verifiable-across-runs case). A no-op with the ephemeral per-process key.
    write_startup_attestation(&signer, args.signing_key.is_some(), &actor);

    // Spawn the downstream MCP server (everything after `--`).
    let (program, down_args) = args
        .downstream
        .split_first()
        .expect("non-empty, checked in parse");
    let client = match McpClient::spawn(program, down_args) {
        Ok(c) => Arc::new(Mutex::new(c)),
        Err(e) => usage_and_exit(&format!("failed to spawn downstream '{program}': {e}")),
    };

    // The governor calls McpProxyExecutor on a cleared call; both share the one downstream client.
    let executor = Box::new(McpProxyExecutor::new(client.clone()));
    let governor =
        Governor::new(policy.clone(), signer.clone(), approval, executor).with_actor(actor.clone());

    let mut server = match ProxyServer::new(&args.name, governor, client, policy) {
        Ok(s) => s,
        Err(e) => usage_and_exit(&format!("downstream handshake failed: {e}")),
    };

    // Banner to stderr — stdout is the JSON-RPC channel.
    let actor_desc = match &actor {
        Some(a) => format!("{}/{}", a.agent, a.user),
        None => "<unattributed>".to_string(),
    };
    eprintln!(
        "[kriya-gateway] proxying '{}' · {} downstream tool(s) ({} visible after policy) · \
         approval={} · actor={} · audit log={}",
        args.downstream.join(" "),
        server.tool_count(),
        server.visible_tool_count(),
        args.approval,
        actor_desc,
        signer.log_path().display()
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    server.serve(stdin.lock(), &mut out)
}

/// `kriya-gateway reach-in --app "<App Name>" [OPTIONS]` — Front 2 (service-architecture §5):
/// govern an app that exposes **no MCP server and no API** by synthesizing tools from its macOS
/// accessibility tree and routing each cleared `tools/call` through the same governance core as
/// `proxy`. macOS-only; requires Accessibility permission granted to this process. Coverage is
/// bounded (degrades on custom-drawn / Electron / web UIs) — prefer `proxy` when the app speaks MCP.
/// Accepts the same governance flags as `proxy` (--policy/--approval/--actor/--user/--audit-log/
/// --signing-key/--config/--name); `--app` replaces the downstream command.
#[cfg(all(feature = "reach-in", target_os = "macos"))]
fn run_reachin(mut it: impl Iterator<Item = String>) -> std::io::Result<()> {
    let mut app: Option<String> = None;
    let mut config: Option<PathBuf> = None;
    let mut policy: Option<PathBuf> = None;
    let mut approval = "deny".to_string();
    let mut approval_from_cli = false;
    let mut name = "kriya-gateway".to_string();
    let mut actor: Option<String> = None;
    let mut user: Option<String> = None;
    let mut audit_log: Option<PathBuf> = None;
    let mut signing_key: Option<PathBuf> = None;

    while let Some(flag) = it.next() {
        let mut take = |label: &str| -> String {
            it.next()
                .unwrap_or_else(|| usage_and_exit(&format!("{label} needs a value")))
        };
        match flag.as_str() {
            "--app" => app = Some(take("--app")),
            "--config" => config = Some(PathBuf::from(take("--config"))),
            "--policy" => policy = Some(PathBuf::from(take("--policy"))),
            "--approval" => {
                approval = take("--approval");
                approval_from_cli = true;
            }
            "--name" => name = take("--name"),
            "--actor" => actor = Some(take("--actor")),
            "--user" => user = Some(take("--user")),
            "--audit-log" => audit_log = Some(PathBuf::from(take("--audit-log"))),
            "--signing-key" => signing_key = Some(PathBuf::from(take("--signing-key"))),
            "-h" | "--help" => usage_and_exit("help"),
            other => usage_and_exit(&format!("unknown argument: {other}")),
        }
    }
    let app = app.unwrap_or_else(|| usage_and_exit("reach-in requires --app \"<App Name>\""));

    // Reuse the proxy arg/config plumbing: a ProxyArgs with no downstream, then fold in `.kriya.yaml`
    // (CLI flag > config file > built-in default), exactly as `proxy` does.
    let mut pargs = ProxyArgs {
        policy,
        approval,
        name,
        actor,
        user,
        audit_log,
        signing_key,
        downstream: Vec::new(),
    };
    if let Some((cfg, path)) = load_gateway_config(&config) {
        eprintln!("[kriya-gateway] loaded config: {}", path.display());
        apply_config(&mut pargs, approval_from_cli, cfg);
    }

    let policy = Arc::new(match &pargs.policy {
        Some(p) => Policy::load_or_default(p),
        None => default_proxy_policy(),
    });
    for w in policy.warnings() {
        eprintln!("[kriya-gateway] policy warning: {w}");
    }
    let signer = build_signer(&pargs.audit_log, &pargs.signing_key);
    let approval = build_approval(&pargs.approval);
    let actor = build_actor(pargs.actor.clone(), pargs.user.clone());
    write_startup_attestation(&signer, pargs.signing_key.is_some(), &actor);

    // Build the macOS accessibility backend for the target app and snapshot once, sharing it between
    // the executor (which performs a cleared action) and the server (which synthesizes the tools).
    let backend: Arc<dyn AxBackend> = Arc::new(
        MacAxBackend::for_app(&app).unwrap_or_else(|e| usage_and_exit(&format!("reach-in: {e}"))),
    );
    let nodes = backend.snapshot().unwrap_or_else(|e| {
        usage_and_exit(&format!(
            "reach-in: cannot read '{app}' accessibility tree: {e}"
        ))
    });
    let executor = Box::new(AxExecutor::new(backend.clone(), nodes));
    let governor =
        Governor::new(policy.clone(), signer.clone(), approval, executor).with_actor(actor.clone());

    let server_name = if pargs.name == "kriya-gateway" {
        format!("kriya-gateway (reach-in: {app})")
    } else {
        pargs.name.clone()
    };
    let mut server = ReachInServer::with_backend(server_name, backend, governor, policy)
        .unwrap_or_else(|e| usage_and_exit(&format!("reach-in: {e}")));

    let actor_desc = match &actor {
        Some(a) => format!("{}/{}", a.agent, a.user),
        None => "<unattributed>".to_string(),
    };
    eprintln!(
        "[kriya-gateway] reach-in '{}' · {} tool(s) ({} visible after policy) · approval={} · \
         actor={} · audit log={}",
        app,
        server.tool_count(),
        server.visible_tool_count(),
        pargs.approval,
        actor_desc,
        signer.log_path().display()
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    server.serve(stdin.lock(), &mut out)
}

/// Built with `reach-in` but on a non-macOS host: the accessibility driver is macOS-only today.
#[cfg(all(feature = "reach-in", not(target_os = "macos")))]
fn run_reachin(_it: impl Iterator<Item = String>) -> std::io::Result<()> {
    usage_and_exit("reach-in is currently macOS-only (it drives the macOS accessibility API)")
}

/// Built WITHOUT the `reach-in` feature: tell the operator how to get it.
#[cfg(not(feature = "reach-in"))]
fn run_reachin(_it: impl Iterator<Item = String>) -> std::io::Result<()> {
    usage_and_exit(
        "this gateway build has no reach-in support — rebuild with `--features mcp-client,reach-in`",
    )
}

/// `kriya-gateway computer-use [OPTIONS]` (and `router`) — Front 3 (service-architecture §6, D-017):
/// **governed computer-use**, the universal reach floor. A fixed, system-wide tool set
/// (screenshot/click/move/scroll/type/key + `list_apps`) drives ANY app via pixels, every call routed
/// through the same `Governor` (policy → approval → budget → signed audit) as the other fronts. No
/// `--app` — it governs the whole desktop. Same governance flags as `proxy`/`reach-in`. macOS-only;
/// needs Accessibility (for input) + Screen Recording (for `computer_screenshot`).
#[cfg(all(feature = "computer-use", target_os = "macos"))]
fn run_computer_use(mut it: impl Iterator<Item = String>) -> std::io::Result<()> {
    let mut config: Option<PathBuf> = None;
    let mut policy: Option<PathBuf> = None;
    let mut approval = "deny".to_string();
    let mut approval_from_cli = false;
    let mut name = "kriya-gateway".to_string();
    let mut actor: Option<String> = None;
    let mut user: Option<String> = None;
    let mut audit_log: Option<PathBuf> = None;
    let mut signing_key: Option<PathBuf> = None;

    while let Some(flag) = it.next() {
        let mut take = |label: &str| -> String {
            it.next()
                .unwrap_or_else(|| usage_and_exit(&format!("{label} needs a value")))
        };
        match flag.as_str() {
            "--config" => config = Some(PathBuf::from(take("--config"))),
            "--policy" => policy = Some(PathBuf::from(take("--policy"))),
            "--approval" => {
                approval = take("--approval");
                approval_from_cli = true;
            }
            "--name" => name = take("--name"),
            "--actor" => actor = Some(take("--actor")),
            "--user" => user = Some(take("--user")),
            "--audit-log" => audit_log = Some(PathBuf::from(take("--audit-log"))),
            "--signing-key" => signing_key = Some(PathBuf::from(take("--signing-key"))),
            "-h" | "--help" => usage_and_exit("help"),
            other => usage_and_exit(&format!("unknown argument: {other}")),
        }
    }

    // Reuse the proxy arg/config plumbing (no downstream, no --app), fold in `.kriya.yaml`.
    let mut pargs = ProxyArgs {
        policy,
        approval,
        name,
        actor,
        user,
        audit_log,
        signing_key,
        downstream: Vec::new(),
    };
    if let Some((cfg, path)) = load_gateway_config(&config) {
        eprintln!("[kriya-gateway] loaded config: {}", path.display());
        apply_config(&mut pargs, approval_from_cli, cfg);
    }

    let policy = Arc::new(match &pargs.policy {
        Some(p) => Policy::load_or_default(p),
        None => default_proxy_policy(),
    });
    for w in policy.warnings() {
        eprintln!("[kriya-gateway] policy warning: {w}");
    }
    let signer = build_signer(&pargs.audit_log, &pargs.signing_key);
    let approval = build_approval(&pargs.approval);
    let actor = build_actor(pargs.actor.clone(), pargs.user.clone());
    write_startup_attestation(&signer, pargs.signing_key.is_some(), &actor);

    // System-wide governed desktop control — no per-app snapshot; the fixed tool set drives the whole
    // screen via CGEvent + screencapture. This is the universal "support every app" floor (D-017).
    let backend: Arc<dyn DesktopBackend> = Arc::new(MacDesktopBackend::new());
    let executor = Box::new(ComputerUseExecutor::new(backend));
    let governor =
        Governor::new(policy.clone(), signer.clone(), approval, executor).with_actor(actor.clone());
    let mut server = ComputerUseServer::new(governor, policy);

    let actor_desc = match &actor {
        Some(a) => format!("{}/{}", a.agent, a.user),
        None => "<unattributed>".to_string(),
    };
    eprintln!(
        "[kriya-gateway] computer-use (system-wide, all apps) · {} tool(s) ({} visible after policy) \
         · approval={} · actor={} · audit log={}",
        server.tool_count(),
        server.visible_tool_count(),
        pargs.approval,
        actor_desc,
        signer.log_path().display()
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    server.serve(stdin.lock(), &mut out)
}

/// Built with `computer-use` but on a non-macOS host: the desktop driver is macOS-only today.
#[cfg(all(feature = "computer-use", not(target_os = "macos")))]
fn run_computer_use(_it: impl Iterator<Item = String>) -> std::io::Result<()> {
    usage_and_exit("computer-use is currently macOS-only (CGEvent + screencapture)")
}

/// Built WITHOUT the `computer-use` feature: tell the operator how to get it.
#[cfg(not(feature = "computer-use"))]
fn run_computer_use(_it: impl Iterator<Item = String>) -> std::io::Result<()> {
    usage_and_exit(
        "this gateway build has no computer-use support — rebuild with `--features mcp-client,computer-use`",
    )
}

// ── doctor: operator preflight (R24) ────────────────────────────────────────────────────────────
//
// macOS reach-in only works when (a) the gateway runs from a signed `.app` bundle with a stable
// CFBundleIdentifier — a loose binary spawned by an Electron host (Claude Desktop) can't be granted
// Accessibility via TCC, the bundle can — and (b) that bundle has been added to System Settings →
// Privacy & Security → Accessibility. `doctor` checks both, lists the apps reach-in could target,
// and prints a ready-to-paste Claude Desktop config snippet. Output goes to STDOUT: a human runs
// this in a terminal, not inside an MCP stdio session, so there's no JSON-RPC stream to protect.

/// Whether this process is currently trusted for Accessibility (macOS TCC). Returns `None` on builds
/// that can't answer (no reach-in feature, or non-macOS). On macOS with `reach-in`, calls the system
/// `AXIsProcessTrusted()` — an `extern "C"` fn (hence `unsafe`) that is a pure read, no side effects.
#[cfg(all(feature = "reach-in", target_os = "macos"))]
fn accessibility_trusted() -> Option<bool> {
    // SAFETY: `AXIsProcessTrusted` takes no arguments, returns a `bool`, and only reads the current
    // process's TCC grant — there is nothing to misuse and no memory to manage.
    Some(unsafe { accessibility_sys::AXIsProcessTrusted() })
}

#[cfg(not(all(feature = "reach-in", target_os = "macos")))]
fn accessibility_trusted() -> Option<bool> {
    None
}

/// Best-effort list of user-facing (non-background) GUI apps — the reach-in *candidates*. Uses
/// `osascript`/System Events rather than any private API, so it needs Automation permission for
/// System Events; on failure (denied, not macOS, no osascript) returns `None` and the caller prints
/// a note instead of failing. This is discovery only — it does NOT auto-govern anything.
fn running_gui_apps() -> Option<Vec<String>> {
    let out = std::process::Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to get name of (processes where background only is false)",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let mut apps: Vec<String> = raw
        .trim()
        .split(", ")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    apps.sort();
    apps.dedup();
    if apps.is_empty() {
        None
    } else {
        Some(apps)
    }
}

/// Resolve the path to *this* executable and whether it sits inside a `.app` bundle's
/// `Contents/MacOS/` — the TCC-grantable location. A loose binary (the trap we hit live) can be
/// added to the Accessibility list but the grant won't stick to a stable identity, so reach-in
/// silently stays untrusted. Returns `(current_exe, in_bundle)`.
fn exe_bundle_state() -> (PathBuf, bool) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kriya-gateway"));
    let in_bundle = exe.to_string_lossy().contains(".app/Contents/MacOS/");
    (exe, in_bundle)
}

/// `kriya-gateway doctor [--app "<App Name>"]` — see the module note above. All checks are
/// non-destructive; exit code is 0 even when something needs fixing (it's advisory).
fn run_doctor(mut it: impl Iterator<Item = String>) -> std::io::Result<()> {
    // Tiny hand-rolled flag parse to match the rest of the binary's style.
    let mut app: Option<String> = None;
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--app" => {
                app = Some(
                    it.next()
                        .unwrap_or_else(|| usage_and_exit("--app needs a value")),
                )
            }
            "-h" | "--help" => usage_and_exit("help"),
            other => usage_and_exit(&format!("doctor: unknown argument: {other}")),
        }
    }

    println!("kriya-gateway doctor — macOS reach-in preflight");
    println!("================================================");

    // 1. Where are we running from? (the loose-binary TCC trap).
    let (exe, in_bundle) = exe_bundle_state();
    println!("\n[1] Executable");
    println!("    path: {}", exe.display());
    if in_bundle {
        println!("    OK   running from inside a .app bundle (Contents/MacOS/).");
    } else {
        println!("    WARN running as a LOOSE binary, not from a .app bundle.");
        println!(
            "         macOS TCC grants Accessibility to a stable bundle identity, not a bare path."
        );
        println!(
            "         Build the bundle (scripts/macos/build-gateway-app.sh) and point Claude Desktop's"
        );
        println!(
            "         `command` at  \"…/Kriya Gateway.app/Contents/MacOS/kriya-gateway\"  — not this binary,"
        );
        println!("         or the Accessibility grant will NOT stick.");
    }

    // 2. Accessibility (TCC) trust for THIS process.
    println!("\n[2] Accessibility permission (macOS Privacy & Security → Accessibility)");
    match accessibility_trusted() {
        Some(true) => {
            println!("    OK   this process is trusted for Accessibility — reach-in can read the AX tree.");
        }
        Some(false) => {
            println!("    FIX  this process is NOT trusted for Accessibility.");
            println!("         1. Open System Settings → Privacy & Security → Accessibility");
            println!(
                "         2. Add  \"Kriya Gateway.app\"  (the signed bundle) and toggle it ON"
            );
            println!("         3. Re-run this doctor from inside the bundle to confirm");
            println!("         (opening that pane for you now…)");
            // Best-effort: surface the exact settings pane. Ignore failure (headless / sandboxed).
            let _ = std::process::Command::new("open")
                .arg(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                )
                .status();
        }
        None => {
            #[cfg(not(feature = "reach-in"))]
            println!(
                "    n/a  built WITHOUT reach-in — rebuild with `--features mcp-client,reach-in` to \
                 enable the Accessibility check and reach-in mode."
            );
            #[cfg(all(feature = "reach-in", not(target_os = "macos")))]
            println!("    n/a  reach-in / Accessibility is macOS-only.");
        }
    }

    // 3. Reach-in candidates — running GUI apps (discovery only, no auto-governance).
    println!("\n[3] Reach-in candidates (running user-facing apps)");
    match running_gui_apps() {
        Some(apps) => {
            println!(
                "    Found {} app(s). reach-in can target any of these by name",
                apps.len()
            );
            println!(
                "    (coverage varies — native control/form UIs strong, Electron/Qt/web weak):"
            );
            for a in &apps {
                println!("      • {a}");
            }
        }
        None => {
            println!(
                "    note: couldn't list apps (System Events Automation may be denied, or not macOS)."
            );
            println!(
                "          Grant Automation for System Events, or just pass the app name to --app."
            );
        }
    }

    // 4. Ready-to-paste Claude Desktop config snippet for the named app.
    println!("\n[4] Claude Desktop config");
    // Prefer the real bundle path so the snippet is copy-paste correct; fall back to the loose exe.
    let command_path = if in_bundle {
        exe.to_string_lossy().to_string()
    } else {
        // Point at where the built bundle's binary lives so the snippet is right even when doctor
        // was (wrongly) run from a loose binary.
        "/Applications/Kriya Gateway.app/Contents/MacOS/kriya-gateway".to_string()
    };
    match &app {
        Some(name) => {
            println!(
                "    Paste into Claude Desktop's claude_desktop_config.json (\"mcpServers\" map),"
            );
            println!("    governing \"{name}\" via reach-in with the GUI approval modal:\n");
            // Build the snippet with serde_json so quoting/escaping is always correct.
            let server_key = format!(
                "kriya-{}",
                name.to_lowercase()
                    .replace(|c: char| !c.is_alphanumeric(), "-")
            );
            let snippet = serde_json::json!({
                "mcpServers": {
                    server_key: {
                        "command": command_path,
                        "args": ["reach-in", "--app", name, "--approval", "gui"]
                    }
                }
            });
            println!("{}", serde_json::to_string_pretty(&snippet).unwrap());
            if !in_bundle {
                println!(
                    "\n    (adjust `command` to wherever you installed the bundle; the path above assumes /Applications)"
                );
            }
        }
        None => {
            println!(
                "    pass  --app \"<App Name>\"  to print a ready-to-paste mcpServers snippet,"
            );
            println!("    e.g.  kriya-gateway doctor --app \"Numbers\"");
        }
    }

    println!("\nDone. (doctor is advisory — it never changes governance or your config.)");
    Ok(())
}
