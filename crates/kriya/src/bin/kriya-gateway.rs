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
//!   --audit-log <path>    signed-receipt JSONL log. Default: a per-front file under the standard
//!                         ~/.kriya/audit/ dir (the Console auto-discovers + tails it — R27). Override
//!                         for an ad-hoc log somewhere else.
//!   --signing-key <path>  persist the Ed25519 identity here (0600) for a STABLE trust anchor
//!                         across runs (R20). Default: ephemeral per-process key
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
//! ## Standard on-device audit-log location (R27 / D-018)
//! When no `--audit-log` is given, the gateway writes its signed-receipt log to a **stable per-front
//! file under `~/.kriya/audit/`** — the standard, OS-appropriate directory the **kriya control-plane
//! Console auto-discovers and tails** (open the app, see your governance; no file-hunting, no manual
//! import). The filename names the front so re-runs continue the same hash-chained log instead of
//! scattering one file per run: a proxied server → `<server>.jsonl`, `reach-in --app Numbers` →
//! `reach-in-numbers.jsonl`, `computer-use` → `computer_use.jsonl`, `router` → `router.jsonl`. Pass
//! `--audit-log <path>` to override (ad-hoc inspection, or a custom location). The directory is
//! created on first write; the convention is shared with the Console via [`default_audit_dir`].
//!
//! Banner + per-call decisions go to **stderr**; stdout is reserved for the MCP JSON-RPC stream.

use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, Mutex};

use kriya::audit::{default_audit_dir, now_ms, Actor, Receipt, Signer, ATTESTATION_ON_DEVICE};
#[cfg(target_os = "macos")]
use kriya::mcp::GuiApproval;
use kriya::mcp::{
    ApprovalGate, AutoApprove, DenyApproval, EgressControl, EgressTarget, Governor, HashScheme,
    IngressControl, IoDecision, IoDirection, IoKind, IoRecord, McpClient, McpProxyExecutor,
    ProxyServer, TtyApproval,
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
// Router v2 (router feature) — ONE endpoint over many fronts under ONE governor. macOS-only build of
// the real `run_router` (it assembles the macOS reach-in + computer-use backends).
use kriya::mcp::jsonrpc::Tool;
#[cfg(all(feature = "router", target_os = "macos"))]
use kriya::mcp::{Front, RouterServer};
use kriya::permissions::{
    default_broker_policy, default_proxy_policy, url_host, ConnectorRegistryPolicy, EgressDecision,
    Policy,
};
// Broker (W2): the router core multiplexes N proxied MCP upstreams under one governor. Available
// under `mcp-client` (no OS deps) — same gate as the proxy this binary already requires. Guarded to
// the negation of the import above so building with BOTH `router` and `mcp-client` on macOS (the
// Console's sidecar build) doesn't double-import `Front`/`RouterServer`.
#[cfg(not(all(feature = "router", target_os = "macos")))]
use kriya::mcp::{Front, RouterServer};
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
         kriya-gateway broker --config broker.yaml [same governance flags]   (ONE endpoint over N MCP upstreams, one governor)\n       \
         kriya-gateway reach-in --app \"<App Name>\" [same governance flags]\n       \
         kriya-gateway computer-use [same governance flags]   (system-wide pixel floor, all apps)\n       \
         kriya-gateway router [--reach-in \"App1,App2\"] [same governance flags]   (computer-use floor + named reach-in apps, ONE governor)\n       \
         kriya-gateway doctor [--app \"<App Name>\"]   (macOS preflight: Accessibility, bundle, snippet)\n       \
         kriya-gateway run [--policy <p.yaml with an egress: section>] [same governance flags] -- <agent-cmd> [args...]\n       \
         \x20  (EG-C containment, macOS only: forces the launched agent's egress through the governed lane —\n       \
         \x20  see the honest ceiling in kriya::mcp::contain's module doc before selling this)\n       \
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
        // Broker (W2) — ONE endpoint, N MCP upstreams, one governor/signer/log. The single wiring
        // point for a client with no hook (Claude Desktop) or many servers (Cursor). Config-driven.
        "broker" => run_broker(args),
        // Containment (EG-C, doc 24 §11 B14) — force a LAUNCHED agent's egress through the
        // governed lane via a macOS Seatbelt profile + recording CONNECT proxy. Read
        // kriya::mcp::contain's module doc before selling this: it is network-only (the spike
        // found unified-log exec/file fidelity too weak to claim more) and macOS-only in v1.
        "run" => run_contained(args),
        // Front 2 — govern an app that has NO MCP server / NO API via its accessibility tree.
        "reach-in" => run_reachin(args),
        // Front 3 — governed computer-use (system-wide pixels): the universal reach floor alone.
        "computer-use" => run_computer_use(args),
        // Router v2 — ONE endpoint multiplexing the computer-use floor (any app) PLUS each
        // `--reach-in` app's named controls, all under ONE governor (one policy/audit/actor).
        "router" => run_router(args),
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

/// Build the signer for an already-resolved audit-log path, validating the signing-key path up front
/// so a bad path is a clean startup error rather than a hard failure mid-session (service-architecture
/// §7). The `audit_log` is always concrete here — [`resolve_audit_log`] defaults it under the standard
/// `~/.kriya/audit/` directory (R27) when the operator passes no `--audit-log`, so the gateway always
/// has a stable, Console-discoverable log. A persisted `--signing-key` gives a stable trust anchor
/// across runs (R20); without it the signer mints an ephemeral per-process key but still appends to
/// the same log file.
fn build_signer(audit_log: &Path, signing_key: &Option<PathBuf>) -> Arc<Signer> {
    match signing_key {
        Some(key) => match Signer::with_identity(key, audit_log.to_path_buf()) {
            Ok(s) => Arc::new(s),
            Err(e) => usage_and_exit(&format!("cannot use --signing-key {key:?}: {e}")),
        },
        None => Arc::new(Signer::with_log_path(audit_log.to_path_buf())),
    }
}

/// Resolve the audit-log path for a front (R27 / D-018). An explicit `--audit-log` (or the config
/// file's `audit_log`) always wins. Otherwise default to a **stable file under the standard
/// `~/.kriya/audit/` directory**, named for this front (`<label>.jsonl`), so the control-plane
/// Console auto-discovers and tails it and re-runs of the same front *continue the same
/// hash-chained log* instead of scattering a new file per run. `label` is a human-meaningful, stable
/// identifier for the front (the downstream server name, the reach-in app, `computer-use`, `router`).
fn resolve_audit_log(explicit: Option<PathBuf>, label: &str) -> PathBuf {
    explicit
        .unwrap_or_else(|| default_audit_dir().join(format!("{}.jsonl", slugify(label, "gateway"))))
}

/// A stable, human-meaningful label for a proxied downstream server, used to name its default
/// audit-log file (R27). Prefer the first script-like argument's file stem (e.g.
/// `node actual-mcp-server.js` → `actual-mcp-server`); else the program's own basename
/// (`uvx some-server` → `uvx`). Falls back to `proxy`.
fn downstream_label(downstream: &[String]) -> String {
    let pick = downstream
        .iter()
        .find(|a| !a.starts_with('-') && Path::new(a.as_str()).extension().is_some())
        .or_else(|| downstream.first())
        .map(|s| s.as_str())
        .unwrap_or("proxy");
    Path::new(pick)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "proxy".to_string())
}

/// Slugify an arbitrary string into a stable, filesystem- and MCP-safe token (`[a-z0-9_]`):
/// lowercase, runs of non-alphanumerics collapse to a single `_`, leading/trailing `_` trimmed.
/// Returns `fallback` for an all-punctuation / empty input. Used for default audit-log filenames
/// (R27) and router namespaces (it never emits `__`, the router's namespace separator).
fn slugify(s: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = true; // start true so a leading separator adds no leading underscore
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
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

    // R27: default the audit log to a stable per-server file under ~/.kriya/audit/ (named for the
    // downstream server) so the Console auto-discovers it; --audit-log still overrides.
    let audit_log = resolve_audit_log(args.audit_log.clone(), &downstream_label(&args.downstream));
    let signer = build_signer(&audit_log, &args.signing_key);
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

/// One upstream MCP server the broker multiplexes: a stable `name` (its namespace in the served
/// union — tools appear as `<name>__<tool>`) and EITHER a local stdio server to spawn (`command` +
/// `args`) OR a remote server to connect to (`url` + optional `headers`, W2-2). Exactly one of
/// `command`/`url` must be set.
#[derive(Debug, Deserialize)]
struct BrokerUpstream {
    /// Stable namespace for this upstream (slugified). Tools are served as `<name>__<tool>` and
    /// policy matches that namespaced name (`<name>__*` gates the whole upstream).
    name: String,
    /// Local stdio upstream: the server program to spawn. Mutually exclusive with `url`.
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    /// Remote upstream (W2-2): the MCP server's Streamable-HTTP endpoint. Mutually exclusive with
    /// `command` — reaches hosted servers a client adds directly (no local process to spawn).
    #[serde(default)]
    url: Option<String>,
    /// Extra request headers for a remote `url` upstream — e.g. `Authorization: "Bearer <token>"`.
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
}

/// The broker config file (`--config broker.yaml`). The `upstreams:` list is the only broker-
/// specific field; the rest mirror the gateway governance knobs so a project pins its whole posture
/// in one file. Unknown fields are rejected so a typo is a clean error, not a silent no-op.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerConfig {
    #[serde(default)]
    upstreams: Vec<BrokerUpstream>,
    policy: Option<PathBuf>,
    approval: Option<String>,
    audit_log: Option<PathBuf>,
    signing_key: Option<PathBuf>,
    name: Option<String>,
    actor: Option<String>,
    user: Option<String>,
}

/// Connect a remote (HTTP/SSE) broker upstream (W2-2). Gated on `mcp-http`: a build without it that
/// hits a `url:` upstream is a clean startup error telling the operator to rebuild, not a mystery.
#[cfg(feature = "mcp-http")]
fn connect_remote_upstream(
    name: &str,
    url: &str,
    headers: &std::collections::BTreeMap<String, String>,
    ssrf_guard: bool,
    secrets: Option<kriya::secrets::SecretsPolicy>,
) -> Arc<Mutex<McpClient>> {
    let hdrs: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    match McpClient::connect_http(url, hdrs, ssrf_guard, secrets) {
        Ok(c) => Arc::new(Mutex::new(c)),
        Err(e) => usage_and_exit(&format!(
            "broker: failed to connect remote upstream '{name}' ({url}): {e}"
        )),
    }
}

#[cfg(not(feature = "mcp-http"))]
fn connect_remote_upstream(
    name: &str,
    _url: &str,
    _headers: &std::collections::BTreeMap<String, String>,
    _ssrf_guard: bool,
    _secrets: Option<kriya::secrets::SecretsPolicy>,
) -> Arc<Mutex<McpClient>> {
    usage_and_exit(&format!(
        "broker: upstream '{name}' is remote (`url:`), but this build has no HTTP support — \
         rebuild with `--features mcp-http`"
    ))
}

/// `kriya-gateway broker --config broker.yaml [OPTIONS]` — the **aggregator** (W2): ONE MCP
/// endpoint multiplexing N upstream MCP servers under ONE governor. A client with no hook seam
/// (Claude Desktop) or many servers (Cursor) points at this single entry; every upstream's tools
/// are served namespaced (`<upstream>__<tool>`) and every `tools/call` routes through the same
/// policy → approval → budget → signed audit into one `broker.jsonl` chain. This reuses the
/// router's `RouterServer` (each upstream is a `Front` whose executor is an `McpProxyExecutor`
/// over that upstream's spawned client), so one governor, one signer, one actor cover them all —
/// exactly the router's "one Governor, not one-per-front" property, applied to proxied MCP.
///
/// The `upstreams:` list lives in the config file (there's no clean single-command-line form for N
/// servers). Governance flags (--policy/--approval/--actor/--user/--audit-log/--signing-key/--name)
/// still work and take precedence over the config's same-named fields. With no `--policy`, the
/// default is a **per-namespace** deny-by-default (reads allow, destructive/spend approve, else
/// deny) — the flat proxy default would never match the namespaced names.
/// Sign a `kriya.io.egress.mcp.deny` receipt for a remote upstream refused at boot by the egress
/// allowlist (doc 24 §7.3 / L10). Written at the decision point — a denied upstream never connects,
/// so without this the `deny` rows would never exist.
fn record_egress_deny(
    signer: &Signer,
    actor: &Option<Actor>,
    host: &str,
    server: &str,
    rule: Option<String>,
    reason: String,
) {
    let io = IoRecord {
        direction: IoDirection::Egress,
        dest_host: Some(host.to_string()),
        dest_kind: IoKind::Mcp,
        method: None,
        bytes_out: None,
        bytes_in: None,
        bytes_in_is_partial: false,
        content_sha256: None,
        hash_scheme: HashScheme::WireBytes,
        decision: IoDecision::Deny,
        policy_rule: rule,
        approved_by: None,
        reason: Some(reason),
        server: Some(server.to_string()),
        flags: Vec::new(),
    };
    signer.record(
        Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            io.action_id(),
            io.params(None),
            false,
            now_ms(),
        )
        .with_actor(actor.clone()),
    );
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Canonical key-sorted JSON serialization — matches `kriya::audit`'s param canonicalization (and
/// `kriya-hook`'s own private copy) so a hash taken here is stable regardless of struct field
/// declaration order.
fn canonical_json_string(v: &serde_json::Value) -> String {
    serde_json::to_string(&canonical_value(v)).unwrap_or_default()
}

fn canonical_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonical_value(&map[k]));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonical_value).collect())
        }
        other => other.clone(),
    }
}

/// SHA-256 hex of a tool's canonical (key-sorted) JSON description — the B10 connector-registry
/// drift signal (doc 24 §11 B10). Computed identically wherever a tool needs comparing: here at
/// discovery time, and by whatever authors an `approved:` entry (the Console's "approve connector"
/// flow) — both sides must hash `name` + `description` + `input_schema` the same way for a match to
/// mean anything.
fn connector_tool_hash(tool: &Tool) -> String {
    let v = serde_json::to_value(tool).unwrap_or_default();
    sha256_hex(canonical_json_string(&v).as_bytes())
}

/// Sign a `kriya.io.egress.mcp.deny` receipt for a connector tool the registry disabled at discovery
/// time (doc 24 §11 B10) — either never approved, or approved but its description has drifted from
/// the hash it was approved under (the tool-poisoning signal). No `dest_host`: this is about a
/// specific TOOL, not a destination (doc 24 §4.3's "not claimable" case) — `server` carries the
/// connector namespace instead.
fn record_connector_disabled(
    signer: &Signer,
    actor: &Option<Actor>,
    namespace: &str,
    tool: &str,
    reason: String,
) {
    let io = IoRecord {
        direction: IoDirection::Egress,
        dest_host: None,
        dest_kind: IoKind::Mcp,
        method: None,
        bytes_out: None,
        bytes_in: None,
        bytes_in_is_partial: false,
        content_sha256: None,
        hash_scheme: HashScheme::WireBytes,
        decision: IoDecision::Deny,
        policy_rule: None,
        approved_by: None,
        reason: Some(format!("{reason} (tool={namespace}__{tool})")),
        server: Some(namespace.to_string()),
        flags: Vec::new(),
    };
    signer.record(
        Receipt::new(
            uuid::Uuid::new_v4().to_string(),
            io.action_id(),
            io.params(None),
            false,
            now_ms(),
        )
        .with_actor(actor.clone()),
    );
}

/// B10: filter `tools` down to those the connector registry allows — approved, with a matching
/// live description hash. An absent/disabled registry passes every tool through unchanged (opt-in,
/// never silently restrictive by default). A tool that's new (never approved) or drifted (approved
/// once, description since changed) is dropped from what the router even registers — "disabled"
/// means structurally absent (never listed, never callable), not merely deny-on-call — and this IS
/// the decision point, so it's receipted here (a dropped tool is never seen again this session).
fn filter_connector_registry(
    tools: Vec<Tool>,
    namespace: &str,
    registry: Option<&ConnectorRegistryPolicy>,
    signer: &Signer,
    actor: &Option<Actor>,
) -> Vec<Tool> {
    let Some(reg) = registry.filter(|r| r.enabled) else {
        return tools;
    };
    tools
        .into_iter()
        .filter(|tool| {
            match reg
                .approved
                .iter()
                .find(|a| a.upstream == namespace && a.tool == tool.name)
            {
                None => {
                    record_connector_disabled(
                        signer,
                        actor,
                        namespace,
                        &tool.name,
                        "connector tool not in the approved registry — disabled until approved (B10)"
                            .to_string(),
                    );
                    false
                }
                Some(approved) => {
                    if connector_tool_hash(tool) == approved.description_hash {
                        true
                    } else {
                        record_connector_disabled(
                            signer,
                            actor,
                            namespace,
                            &tool.name,
                            "connector tool description drifted from its approved hash — disabled \
                             until re-approved (B10)"
                                .to_string(),
                        );
                        false
                    }
                }
            }
        })
        .collect()
}

fn run_broker(mut it: impl Iterator<Item = String>) -> std::io::Result<()> {
    let mut config: Option<PathBuf> = None;
    let mut policy: Option<PathBuf> = None;
    let mut approval: Option<String> = None;
    let mut name: Option<String> = None;
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
            "--approval" => approval = Some(take("--approval")),
            "--name" => name = Some(take("--name")),
            "--actor" => actor = Some(take("--actor")),
            "--user" => user = Some(take("--user")),
            "--audit-log" => audit_log = Some(PathBuf::from(take("--audit-log"))),
            "--signing-key" => signing_key = Some(PathBuf::from(take("--signing-key"))),
            "-h" | "--help" => usage_and_exit("help"),
            other => usage_and_exit(&format!("broker: unknown argument: {other}")),
        }
    }

    // The broker needs its `upstreams:` list, which only lives in a config file. Auto-discover the
    // same broker.yaml/.kriya-broker.yaml the way proxy discovers .kriya.yaml, but require SOME
    // config with upstreams — a broker with no upstreams is a no-op the operator didn't intend.
    let cfg_path = config.clone().or_else(|| {
        [".kriya-broker.yaml", "broker.yaml"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
    });
    let cfg_path = cfg_path.unwrap_or_else(|| {
        usage_and_exit(
            "broker needs a config file with `upstreams:` — pass --config broker.yaml \
             (see: kriya-gateway broker --help)",
        )
    });
    let text = std::fs::read_to_string(&cfg_path).unwrap_or_else(|e| {
        usage_and_exit(&format!("cannot read broker config {cfg_path:?}: {e}"))
    });
    let cfg: BrokerConfig = serde_yaml::from_str(&text).unwrap_or_else(|e| {
        usage_and_exit(&format!("broker config {cfg_path:?} is malformed: {e}"))
    });
    eprintln!(
        "[kriya-gateway] loaded broker config: {}",
        cfg_path.display()
    );

    if cfg.upstreams.is_empty() {
        usage_and_exit(&format!(
            "broker config {cfg_path:?} declares no `upstreams:` — nothing to multiplex"
        ));
    }

    // Precedence CLI > config > default, matching the proxy path.
    let policy_path = policy.or(cfg.policy);
    let approval = approval.or(cfg.approval).unwrap_or_else(|| "deny".into());
    let actor_name = actor.or(cfg.actor);
    let user = user.or(cfg.user);
    let name = name
        .or(cfg.name)
        .unwrap_or_else(|| "kriya-gateway (broker)".into());

    // Namespaces are slugified upstream names; reject collisions up front (two upstreams sharing a
    // namespace would silently merge tool sets and cross-route — a correctness bug, not a warning).
    let mut namespaces: Vec<String> = Vec::new();
    for up in &cfg.upstreams {
        let ns = slugify(&up.name, "upstream");
        if namespaces.contains(&ns) {
            usage_and_exit(&format!(
                "broker: two upstreams slugify to the same namespace '{ns}' — rename one \
                 (namespaces must be unique; they prefix every tool and every policy rule)"
            ));
        }
        namespaces.push(ns);
    }

    let policy = Arc::new(match &policy_path {
        Some(p) => Policy::load_or_default(p),
        None => default_broker_policy(&namespaces),
    });
    for w in policy.warnings() {
        eprintln!("[kriya-gateway] policy warning: {w}");
    }

    // R27: one broker.jsonl chain for the whole aggregator (all upstreams, one governor).
    let audit_log = resolve_audit_log(audit_log.or(cfg.audit_log), "broker");
    let signing_key = signing_key.or(cfg.signing_key);
    let signer = build_signer(&audit_log, &signing_key);
    let approval_gate = build_approval(&approval);
    let actor = build_actor(actor_name, user);
    write_startup_attestation(&signer, signing_key.is_some(), &actor);

    // Egress governance (doc 24 §7.3): if the policy carries an `egress:` tier, remote upstreams are
    // allowlisted by host. The per-call tier + byte-budget + `kriya.io.*` receipt is enforced by the
    // governor via a resolver over this namespace→host map; the STARTUP check below additionally
    // refuses to connect a deny-tier host at all (closing landmine L3 for denied hosts).
    let egress_policy = policy.egress().cloned();
    let mut egress_targets: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();

    // Spawn each upstream, handshake it, cache its tools, and wrap it as a Front whose executor is
    // an McpProxyExecutor over that upstream's own client. One RouterServer then governs them all.
    let mut fronts: Vec<Front> = Vec::new();
    let mut summaries: Vec<String> = Vec::new();
    for (up, ns) in cfg.upstreams.iter().zip(&namespaces) {
        // Startup allowlist check (L10): with a deny-tier egress destination, a remote upstream
        // refuses to CONNECT at boot — receipted as kriya.io.egress.mcp.deny at the decision point,
        // BEFORE the un-ledgered handshake below ever reaches it. stdio upstreams have no host to
        // allowlist and are unaffected.
        if let (Some(url), Some(ep)) = (&up.url, &egress_policy) {
            let host = url_host(url);
            if let EgressDecision::Deny { rule, reason } = ep.evaluate(&host) {
                record_egress_deny(&signer, &actor, &host, &up.name, rule, reason);
                eprintln!(
                    "[kriya-gateway] broker: upstream '{}' ({host}) DENIED by egress allowlist — not connected",
                    up.name
                );
                summaries.push(format!("{ns} ('{}', DENIED by egress)", up.name));
                continue;
            }
        }

        // A local stdio upstream (`command`) or a remote one (`url`) — exactly one. The connect
        // path differs; everything after (handshake → tools → Front) is identical, which is the
        // whole point of the McpClient backend enum (W2-2).
        let client = match (&up.command, &up.url) {
            (Some(cmd), None) => match McpClient::spawn(cmd, &up.args) {
                Ok(c) => Arc::new(Mutex::new(c)),
                Err(e) => usage_and_exit(&format!(
                    "broker: failed to spawn upstream '{}' ({cmd}): {e}",
                    up.name
                )),
            },
            (None, Some(url)) => {
                let ssrf_guard = policy
                    .detection()
                    .and_then(|d| d.ssrf_guard)
                    .is_some_and(|g| g.enabled);
                connect_remote_upstream(
                    &up.name,
                    url,
                    &up.headers,
                    ssrf_guard,
                    policy.secrets().cloned(),
                )
            }
            (Some(_), Some(_)) => usage_and_exit(&format!(
                "broker: upstream '{}' sets both `command` and `url` — pick one",
                up.name
            )),
            (None, None) => usage_and_exit(&format!(
                "broker: upstream '{}' sets neither `command` (stdio) nor `url` (remote)",
                up.name
            )),
        };
        {
            // The initialize/tools/list handshake here is PRE-GOVERNOR egress (landmine L3). For a
            // remote upstream it is un-ledgered protocol overhead — MCP session setup, not an agent
            // tool call — and it only runs for a host the egress tier already ALLOWED (the deny
            // check above gates connection). Every subsequent governed `tools/call` IS receipted as
            // kriya.io.egress.mcp.* by the governor.
            let mut guard = client.lock().unwrap();
            if let Err(e) = guard.initialize() {
                usage_and_exit(&format!(
                    "broker: upstream '{}' handshake failed: {e}",
                    up.name
                ));
            }
        }
        let tools = {
            let mut guard = client.lock().unwrap();
            guard.list_tools().unwrap_or_else(|e| {
                usage_and_exit(&format!(
                    "broker: upstream '{}' tools/list failed: {e}",
                    up.name
                ))
            })
        };
        // B10 connector registry (doc 24 §11): drop any tool that's new or drifted BEFORE it's ever
        // registered with the router — disabled means the router never learns it exists.
        let tools = filter_connector_registry(
            tools,
            &up.name,
            policy
                .detection()
                .and_then(|d| d.connector_registry.as_ref()),
            &signer,
            &actor,
        );
        let count = tools.len();
        let executor = Box::new(McpProxyExecutor::new(client));
        fronts.push(Front::new(ns.clone(), tools, executor));
        let kind = if up.url.is_some() { "remote" } else { "stdio" };
        summaries.push(format!("{ns} ('{}', {kind}, {count} tools)", up.name));
        // Record the resolver target for a connected remote upstream so the governor can map a
        // `<ns>__<tool>` call to its host for the per-call egress tier + io receipt.
        if let Some(url) = &up.url {
            egress_targets.insert(ns.clone(), (url_host(url), up.name.clone()));
        }
    }

    // Wrap the namespace→host map + tier into the governor's egress control (doc 24 §7.3). Absent
    // egress policy → None → the broker governs exactly as before.
    let egress_control = egress_policy.map(|ep| {
        EgressControl::new(ep, move |action_id: &str, _params: &serde_json::Value| {
            let ns = action_id
                .split_once("__")
                .map(|(ns, _)| ns)
                .unwrap_or(action_id);
            egress_targets.get(ns).map(|(host, server)| EgressTarget {
                host: host.clone(),
                kind: IoKind::Mcp,
                server: Some(server.clone()),
            })
        })
    });

    // Wrap the per-server trust classes into the governor's ingress control (doc 24 §11 B12). The
    // resolver maps a namespaced `<upstream>__<tool>` action back to its upstream name — the same
    // convention `egress_control`'s resolver above uses to go the other way (name → host). Absent
    // policy → None → the broker governs exactly as before.
    let ingress_control = policy
        .detection()
        .and_then(|d| d.mcp_response.clone())
        .map(|mrp| {
            IngressControl::new(mrp, |action_id: &str| {
                action_id.split_once("__").map(|(ns, _)| ns.to_string())
            })
        });

    let mut server = RouterServer::from_parts_with_egress_and_ingress(
        name,
        fronts,
        policy,
        signer.clone(),
        approval_gate,
        actor.clone(),
        egress_control,
        ingress_control,
    );

    let actor_desc = match &actor {
        Some(a) => format!("{}/{}", a.agent, a.user),
        None => "<unattributed>".to_string(),
    };
    eprintln!(
        "[kriya-gateway] broker · {} upstream(s): {} · {} tool(s) ({} visible after policy) · \
         approval={} · actor={} · audit log={}",
        summaries.len(),
        summaries.join(", "),
        server.tool_count(),
        server.visible_tool_count(),
        approval,
        actor_desc,
        signer.log_path().display()
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    server.serve(stdin.lock(), &mut out)
}

/// `kriya-gateway run [OPTIONS] -- <agent-cmd> [args...]` — containment (EG-C, doc 24 §11 B14):
/// force a LAUNCHED agent's network egress through the governed lane, closing the `curl`/subprocess
/// bypass the governed-lane receipts honestly disclose. macOS only in this release (Seatbelt);
/// **read `kriya::mcp::contain`'s module doc for the honest ceiling before this is sold as
/// anything beyond "network egress from agents kriya launches."**
///
/// Requires a `--policy` with an `egress:` section — `run` with no egress policy would sandbox the
/// child for nothing (every destination would fall through the default `unlisted` posture with no
/// operator-authored intent behind it), so it's a clean startup error rather than a silent no-op.
#[cfg(feature = "contain")]
fn run_contained(it: impl Iterator<Item = String>) -> std::io::Result<()> {
    let args = parse_proxy_args(it);
    let policy = Arc::new(match &args.policy {
        Some(p) => Policy::load_or_default(p),
        None => usage_and_exit(
            "run requires --policy <p.yaml> with an `egress:` section — containment with no \
             egress policy would sandbox the child for no operator-authored reason",
        ),
    });
    if policy.egress().is_none() {
        usage_and_exit(
            "run requires the policy to have an `egress:` section — see docs/gtm/samples or \
             doc 24 §7.3 for the shape",
        );
    }
    for w in policy.warnings() {
        eprintln!("[kriya-gateway] policy warning: {w}");
    }

    let audit_log = resolve_audit_log(args.audit_log.clone(), "run");
    let signer = build_signer(&audit_log, &args.signing_key);
    let actor = build_actor(args.actor.clone(), args.user.clone());
    write_startup_attestation(&signer, args.signing_key.is_some(), &actor);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&policy, &signer, &actor); // keep bindings used on the error path
        usage_and_exit(
            "kriya-gateway run is macOS-only in this release (Seatbelt containment) — Linux \
             containment rides the W3 Tetragon watcher (kriyawatch) when it ships; see doc 24 \
             §11.3's documented v1 choice",
        );
    }

    #[cfg(target_os = "macos")]
    {
        use kriya::mcp::{seatbelt_profile, ConnectProxy, RUN_EXIT, RUN_START};

        let scope_token = uuid::Uuid::new_v4().to_string();
        let egress_policy = policy.egress().cloned().expect("checked above");
        let proxy = match ConnectProxy::spawn(
            egress_policy,
            signer.clone(),
            actor.clone(),
            scope_token.clone(),
        ) {
            Ok(p) => p,
            Err(e) => usage_and_exit(&format!("run: failed to start the local egress proxy: {e}")),
        };

        let profile_text = seatbelt_profile(proxy.port());
        let profile_sha256 = kriya::mcp::contain_sha256_hex(profile_text.as_bytes());
        let profile_path = std::env::temp_dir().join(format!("kriya-run-{scope_token}.sb"));
        if let Err(e) = std::fs::write(&profile_path, &profile_text) {
            usage_and_exit(&format!(
                "run: failed to write the Seatbelt profile {profile_path:?}: {e}"
            ));
        }

        signer.record(
            Receipt::new(
                uuid::Uuid::new_v4().to_string(),
                RUN_START.to_string(),
                serde_json::json!({
                    "scope_token": scope_token,
                    "seatbelt_profile_sha256": profile_sha256,
                    "proxy_port": proxy.port(),
                    "downstream": args.downstream,
                }),
                true,
                now_ms(),
            )
            .with_actor(actor.clone()),
        );

        eprintln!(
            "[kriya-gateway] run: containing '{}' under Seatbelt profile {profile_path:?} \
             (sha256={profile_sha256}) · egress proxy on 127.0.0.1:{} · scope_token={scope_token} \
             · audit log={}",
            args.downstream.join(" "),
            proxy.port(),
            signer.log_path().display()
        );
        eprintln!(
            "[kriya-gateway] honest ceiling: network-only containment for THIS process and its \
             children — no file/exec visibility (the spike found log-stream fidelity too weak to \
             claim it); a raw socket that ignores the proxy is blocked by the Seatbelt profile \
             itself, not by agent cooperation."
        );

        let proxy_url = format!("http://127.0.0.1:{}", proxy.port());
        let (program, down_args) = args
            .downstream
            .split_first()
            .expect("non-empty, checked in parse_proxy_args");
        let start = std::time::Instant::now();
        let status = std::process::Command::new("sandbox-exec")
            .arg("-f")
            .arg(&profile_path)
            .arg(program)
            .args(down_args)
            .env("HTTP_PROXY", &proxy_url)
            .env("HTTPS_PROXY", &proxy_url)
            .env("ALL_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env("https_proxy", &proxy_url)
            .env("all_proxy", &proxy_url)
            .status();
        let duration_ms = start.elapsed().as_millis() as u64;

        let (exit_code, spawn_error) = match &status {
            Ok(s) => (s.code().unwrap_or(-1), None),
            Err(e) => (-1, Some(e.to_string())),
        };
        signer.record(
            Receipt::new(
                uuid::Uuid::new_v4().to_string(),
                RUN_EXIT.to_string(),
                serde_json::json!({
                    "scope_token": scope_token,
                    "exit_code": exit_code,
                    "duration_ms": duration_ms,
                    "spawn_error": spawn_error,
                }),
                spawn_error.is_none(),
                now_ms(),
            )
            .with_actor(actor.clone()),
        );
        let _ = std::fs::remove_file(&profile_path);
        drop(proxy);

        match status {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(e) => usage_and_exit(&format!(
                "run: failed to spawn '{program}' under sandbox-exec: {e}"
            )),
        }
    }
}

/// Built WITHOUT the `contain` feature: tell the operator how to get it.
#[cfg(not(feature = "contain"))]
fn run_contained(_it: impl Iterator<Item = String>) -> std::io::Result<()> {
    usage_and_exit("this gateway build has no containment support — rebuild with `--features mcp-client,contain`")
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
    // R27: default to a stable reach-in-<app>.jsonl under ~/.kriya/audit/; --audit-log overrides.
    let audit_log = resolve_audit_log(pargs.audit_log.clone(), &format!("reach-in-{app}"));
    let signer = build_signer(&audit_log, &pargs.signing_key);
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
    // R27: default to a stable computer-use.jsonl under ~/.kriya/audit/; --audit-log overrides.
    let audit_log = resolve_audit_log(pargs.audit_log.clone(), "computer-use");
    let signer = build_signer(&audit_log, &pargs.signing_key);
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

/// `kriya-gateway router [--reach-in "App1,App2"] [OPTIONS]` — **router v2** (service-architecture):
/// ONE MCP endpoint that multiplexes multiple governed fronts under ONE `Governor`. A single Claude
/// Desktop entry then governs the **computer-use floor (any app)** PLUS the **named reach-in
/// controls** of one or more specific apps at once — every `tools/call` routed to the right front by
/// a `<namespace>__<tool>` prefix, all through the same policy → approval → budget → signed audit.
///
/// Fronts assembled here:
/// - always the **computer-use floor** under namespace `cu` (its fixed catalog +
///   `ComputerUseExecutor` over `MacDesktopBackend`) — the universal "support every app" floor;
/// - for each app in `--reach-in "Numbers,Keynote"`, a reach-in front under a slug of the app name
///   (`numbers`, `keynote`): `MacAxBackend::for_app(app)` → `snapshot()` → synthesized tools +
///   `AxExecutor` over the same backend.
///
/// **Policy matches the namespaced name** — gate `cu__computer_click`, `numbers__*`, etc. Same
/// governance flags as `proxy`/`reach-in`/`computer-use` (--policy/--approval/--actor/--user/
/// --audit-log/--signing-key/--config/--name) PLUS `--reach-in` (comma-separated app names,
/// optional; omit it for the floor alone). macOS-only; needs Accessibility (+ Screen Recording for
/// the floor's screenshot tool).
#[cfg(all(feature = "router", target_os = "macos"))]
fn run_router(mut it: impl Iterator<Item = String>) -> std::io::Result<()> {
    let mut config: Option<PathBuf> = None;
    let mut policy: Option<PathBuf> = None;
    let mut approval = "deny".to_string();
    let mut approval_from_cli = false;
    let mut name = "kriya-gateway".to_string();
    let mut actor: Option<String> = None;
    let mut user: Option<String> = None;
    let mut audit_log: Option<PathBuf> = None;
    let mut signing_key: Option<PathBuf> = None;
    // The reach-in apps to ALSO govern alongside the computer-use floor (comma-separated). Optional.
    let mut reach_in: Vec<String> = Vec::new();

    while let Some(flag) = it.next() {
        let mut take = |label: &str| -> String {
            it.next()
                .unwrap_or_else(|| usage_and_exit(&format!("{label} needs a value")))
        };
        match flag.as_str() {
            "--reach-in" => {
                reach_in = take("--reach-in")
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
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

    // Reuse the proxy arg/config plumbing (no downstream), fold in `.kriya.yaml` for unset flags.
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
    // R27: default to a stable router.jsonl under ~/.kriya/audit/ (one log for the whole router,
    // all fronts under one governor); --audit-log overrides.
    let audit_log = resolve_audit_log(pargs.audit_log.clone(), "router");
    let signer = build_signer(&audit_log, &pargs.signing_key);
    let approval = build_approval(&pargs.approval);
    let actor = build_actor(pargs.actor.clone(), pargs.user.clone());
    write_startup_attestation(&signer, pargs.signing_key.is_some(), &actor);

    // Build the fronts. ALWAYS the computer-use floor (ns "cu") — the universal reach. Each
    // executor is plain `Box<dyn ActionExecutor>`; the RouterServer wraps them all in ONE governor.
    let mut fronts: Vec<Front> = Vec::new();
    let mut front_summaries: Vec<String> = Vec::new();

    let desktop: Arc<dyn DesktopBackend> = Arc::new(MacDesktopBackend::new());
    let cu_tools = kriya::mcp::computeruse::tool_list();
    let cu_count = cu_tools.len();
    fronts.push(Front::new(
        "cu",
        cu_tools,
        Box::new(ComputerUseExecutor::new(desktop)),
    ));
    front_summaries.push(format!("cu (computer-use floor, {cu_count} tools)"));

    // Then one reach-in front per --reach-in app: snapshot its AX tree, synthesize tools + executor.
    for app in &reach_in {
        let backend: Arc<dyn AxBackend> = Arc::new(
            MacAxBackend::for_app(app)
                .unwrap_or_else(|e| usage_and_exit(&format!("router: reach-in '{app}': {e}"))),
        );
        let nodes = backend.snapshot().unwrap_or_else(|e| {
            usage_and_exit(&format!(
                "router: cannot read '{app}' accessibility tree: {e}"
            ))
        });
        let tools = kriya::mcp::reachin::synth::synthesize_tools(&nodes);
        let count = tools.len();
        let ns = slug_namespace(app);
        let executor = Box::new(AxExecutor::new(backend, nodes));
        fronts.push(Front::new(ns.clone(), tools, executor));
        front_summaries.push(format!("{ns} (reach-in '{app}', {count} tools)"));
    }

    let server_name = if pargs.name == "kriya-gateway" {
        "kriya-gateway (router)".to_string()
    } else {
        pargs.name.clone()
    };
    let mut server = RouterServer::from_parts(
        server_name,
        fronts,
        policy,
        signer.clone(),
        approval,
        actor.clone(),
    );

    let actor_desc = match &actor {
        Some(a) => format!("{}/{}", a.agent, a.user),
        None => "<unattributed>".to_string(),
    };
    eprintln!(
        "[kriya-gateway] router · {} front(s): {} · {} tool(s) ({} visible after policy) · \
         approval={} · actor={} · audit log={}",
        front_summaries.len(),
        front_summaries.join(", "),
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

/// Slugify an app name into a stable, MCP-safe namespace token (`[a-z0-9_]`): `"Apple Numbers"` →
/// `"apple_numbers"`. Must not contain `"__"` (the router's namespace separator) — [`slugify`]
/// collapses runs of non-alphanumerics to a single `_`, which guarantees that. Falls back to `"app"`.
#[cfg(all(feature = "router", target_os = "macos"))]
fn slug_namespace(app: &str) -> String {
    slugify(app, "app")
}

/// Built with `router` but on a non-macOS host: the reach-in + computer-use backends are macOS-only.
#[cfg(all(feature = "router", not(target_os = "macos")))]
fn run_router(_it: impl Iterator<Item = String>) -> std::io::Result<()> {
    usage_and_exit(
        "router is currently macOS-only (it composes the macOS reach-in + computer-use fronts)",
    )
}

/// Built WITHOUT the `router` feature: tell the operator how to get it.
#[cfg(not(feature = "router"))]
fn run_router(_it: impl Iterator<Item = String>) -> std::io::Result<()> {
    usage_and_exit(
        "this gateway build has no router support — rebuild with `--features mcp-client,reach-in,computer-use,router`",
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

    // 5. Standard audit-log location (R27): where signed receipts land by default and the directory
    // the control-plane Console auto-discovers + tails. Surfaced so the operator knows where their
    // governance trail is recorded without passing --audit-log.
    println!("\n[5] Audit log location (signed receipts)");
    let audit_dir = default_audit_dir();
    println!("    default dir: {}", audit_dir.display());
    println!("    Each front writes a stable <front>.jsonl here — proxy → <server>.jsonl,");
    println!("    reach-in → reach-in-<app>.jsonl, computer-use → computer_use.jsonl, router → router.jsonl.");
    println!(
        "    The Kriya control-plane app auto-discovers + tails this directory (no manual import)."
    );
    println!("    Override per-run with  --audit-log <path>  for ad-hoc inspection.");

    println!("\nDone. (doctor is advisory — it never changes governance or your config.)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basics() {
        assert_eq!(slugify("Apple Numbers", "app"), "apple_numbers");
        assert_eq!(slugify("computer-use", "x"), "computer_use");
        assert_eq!(slugify("reach-in-Numbers", "x"), "reach_in_numbers");
        assert_eq!(slugify("  -- !! ", "fallback"), "fallback");
        // Never emits the router's "__" namespace separator (runs collapse to a single "_").
        assert!(!slugify("a___b", "x").contains("__"));
    }

    #[test]
    fn downstream_label_prefers_script_stem() {
        assert_eq!(
            downstream_label(&["node".into(), "actual-mcp-server.js".into()]),
            "actual-mcp-server"
        );
        // No script-like arg → fall back to the program basename.
        assert_eq!(
            downstream_label(&["uvx".into(), "some-server".into()]),
            "uvx"
        );
        // Flags are skipped when picking the script.
        assert_eq!(
            downstream_label(&["python".into(), "-m".into(), "server.py".into()]),
            "server"
        );
        assert_eq!(downstream_label(&[]), "proxy");
    }

    #[test]
    fn resolve_audit_log_respects_explicit_override() {
        let explicit = PathBuf::from("/tmp/custom-audit.jsonl");
        assert_eq!(
            resolve_audit_log(Some(explicit.clone()), "router"),
            explicit
        );
    }

    #[test]
    fn resolve_audit_log_defaults_to_named_file_under_standard_dir() {
        let p = resolve_audit_log(None, "computer-use");
        assert_eq!(
            p.file_name().unwrap().to_string_lossy(),
            "computer_use.jsonl"
        );
        assert_eq!(p.parent().unwrap(), default_audit_dir());
    }

    // ---- B10: connector registry + drift detection (doc 24 §11 / EG-P) ----------------------

    fn mk_tool(name: &str, description: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    fn signer_with_log() -> (Signer, std::path::PathBuf) {
        let log = std::env::temp_dir().join(format!("kriya-b10-{}.jsonl", uuid::Uuid::new_v4()));
        (Signer::with_log_path(log.clone()), log)
    }

    fn io_lines(log: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|v| {
                v["action_id"]
                    .as_str()
                    .map(|a| a.starts_with("kriya.io."))
                    .unwrap_or(false)
            })
            .collect()
    }

    #[test]
    fn connector_tool_hash_changes_when_the_description_drifts() {
        let a = connector_tool_hash(&mk_tool("delete_all", "deletes everything in scope"));
        let b = connector_tool_hash(&mk_tool(
            "delete_all",
            "deletes everything in scope, now with more",
        ));
        assert_ne!(
            a, b,
            "a changed description must change the hash (that's the whole point)"
        );
        // Deterministic: hashing the SAME tool twice gives the SAME hash.
        let a2 = connector_tool_hash(&mk_tool("delete_all", "deletes everything in scope"));
        assert_eq!(a, a2);
    }

    #[test]
    fn b10_registry_allows_an_approved_tool_with_a_matching_hash() {
        let (signer, log) = signer_with_log();
        let actor = None;
        let tool = mk_tool("list_widgets", "lists widgets in the warehouse");
        let hash = connector_tool_hash(&tool);
        let registry = ConnectorRegistryPolicy {
            enabled: true,
            approved: vec![kriya::permissions::ApprovedConnectorTool {
                upstream: "widgets".to_string(),
                tool: "list_widgets".to_string(),
                description_hash: hash,
            }],
        };
        let kept =
            filter_connector_registry(vec![tool], "widgets", Some(&registry), &signer, &actor);
        assert_eq!(kept.len(), 1, "an approved, unchanged tool passes through");
        assert!(
            io_lines(&log).is_empty(),
            "no deny receipt for a clean pass"
        );
    }

    #[test]
    fn b10_registry_disables_a_never_approved_tool() {
        let (signer, log) = signer_with_log();
        let actor = None;
        let tool = mk_tool("delete_everything", "irreversibly wipes the warehouse");
        let registry = ConnectorRegistryPolicy {
            enabled: true,
            approved: vec![], // nothing approved yet
        };
        let kept =
            filter_connector_registry(vec![tool], "widgets", Some(&registry), &signer, &actor);
        assert!(
            kept.is_empty(),
            "an unapproved tool must be dropped, not merely deny-on-call"
        );
        let io = io_lines(&log);
        assert_eq!(io.len(), 1);
        assert_eq!(io[0]["action_id"], "kriya.io.egress.mcp.deny");
        let reason = io[0]["params"]["reason"].as_str().unwrap();
        assert!(
            reason.contains("B10") && reason.contains("not in the approved registry"),
            "reason: {reason}"
        );
        assert_eq!(io[0]["params"]["server"], "widgets");
    }

    #[test]
    fn b10_registry_disables_a_drifted_tool_even_though_it_was_once_approved() {
        let (signer, log) = signer_with_log();
        let actor = None;
        let original_hash =
            connector_tool_hash(&mk_tool("list_widgets", "lists widgets in the warehouse"));
        // The LIVE tool's description has changed since approval — the tool-poisoning signal.
        let live_tool = mk_tool(
            "list_widgets",
            "lists widgets AND silently exfiltrates them",
        );
        let registry = ConnectorRegistryPolicy {
            enabled: true,
            approved: vec![kriya::permissions::ApprovedConnectorTool {
                upstream: "widgets".to_string(),
                tool: "list_widgets".to_string(),
                description_hash: original_hash,
            }],
        };
        let kept =
            filter_connector_registry(vec![live_tool], "widgets", Some(&registry), &signer, &actor);
        assert!(kept.is_empty(), "a drifted tool must be dropped");
        let io = io_lines(&log);
        assert_eq!(io.len(), 1);
        let reason = io[0]["params"]["reason"].as_str().unwrap();
        assert!(
            reason.contains("B10") && reason.contains("drifted"),
            "reason: {reason}"
        );
    }

    #[test]
    fn b10_registry_false_positive_safety_absent_or_disabled_registry_is_unaffected() {
        let (signer, log) = signer_with_log();
        let actor = None;
        let tools = vec![
            mk_tool("list_widgets", "lists widgets"),
            mk_tool("delete_everything", "wipes it all"),
        ];
        // No registry configured at all.
        let kept = filter_connector_registry(tools.clone(), "widgets", None, &signer, &actor);
        assert_eq!(
            kept.len(),
            2,
            "no registry configured → every tool passes through unchanged"
        );

        // Registry configured but explicitly disabled.
        let disabled = ConnectorRegistryPolicy {
            enabled: false,
            approved: vec![],
        };
        let kept2 = filter_connector_registry(tools, "widgets", Some(&disabled), &signer, &actor);
        assert_eq!(
            kept2.len(),
            2,
            "a disabled registry must not restrict anything"
        );
        assert!(
            io_lines(&log).is_empty(),
            "no receipts when the registry never engaged"
        );
    }
}
