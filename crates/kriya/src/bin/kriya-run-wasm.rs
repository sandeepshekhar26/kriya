//! `kriya-run-wasm` (F4-wasm, doc 28 §F4) — record or re-verify a **Deterministic Execution** run
//! of a WASI-p2 component. See `kriya::wasmexec`'s module doc for the naming law (this is
//! re-EXECUTION, never "replay" — B1's console-side "Verified Replay" is a different claim) and the
//! full deterministic-configuration rationale.
//!
//! ## Usage
//! ```text
//! kriya-run-wasm run <module.wasm> [OPTIONS]
//!   --arg <value>          (repeatable) one guest argv entry
//!   --env <K=V>             (repeatable) one guest env var
//!   --stdin-file <path>     read stdin from this file (default: empty stdin)
//!   --seed <u64>            seed for the virtualized WASI random interfaces (default 0)
//!   --epoch-ms <u64>        the fixed instant wasi:clocks/wall-clock reports (default: now)
//!   --fuel <u64>            fuel budget (default 500_000_000)
//!   --out <bundle.json>     where to write the record bundle (default: <module>.kriya-exec-bundle.json)
//!   --audit-log <path>      signed-receipt JSONL log (default: ~/.kriya/audit/run-wasm.jsonl — R27)
//!   --signing-key <path>    persist the Ed25519 identity here (0600) for a stable trust anchor
//!   --actor <agent>         agent identity stamped into the receipt. Omit → unattributed
//!   --user <user>           operator the run acts for (default: $USER)
//!   --tool-name <name>      cosmetic label carried into the receipt's params.tool_name
//!
//! kriya-run-wasm --verify <bundle.json> [--module <path>]
//!   Re-executes the recorded run and hash-compares stdout/stderr/fuel. Exit 0 on match, 1 on any
//!   mismatch (module tamper, bundle tamper, or a genuinely divergent re-execution), 2 on usage error.
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::exit;

use kriya::audit::{default_audit_dir, now_ms, Actor, Receipt, Signer};
use kriya::wasmexec::{build_and_record, verify_bundle_file, ExecutionInputs, ACTION_ID};

fn usage_and_exit(msg: &str) -> ! {
    eprintln!("kriya-run-wasm: {msg}");
    eprintln!(
        "usage: kriya-run-wasm run <module.wasm> [--arg v]... [--env K=V]... [--stdin-file path] \
         [--seed n] [--epoch-ms n] [--fuel n] [--out bundle.json] [--audit-log path] \
         [--signing-key path] [--actor a] [--user u] [--tool-name name]\n       \
         kriya-run-wasm --verify <bundle.json> [--module path]"
    );
    exit(2);
}

struct RunArgs {
    module: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    stdin_file: Option<PathBuf>,
    seed: u64,
    epoch_ms: u64,
    fuel: u64,
    out: Option<PathBuf>,
    audit_log: Option<PathBuf>,
    signing_key: Option<PathBuf>,
    actor: Option<String>,
    user: Option<String>,
    tool_name: Option<String>,
}

fn parse_run_args(module: PathBuf, mut it: impl Iterator<Item = String>) -> RunArgs {
    let mut a = RunArgs {
        module,
        args: Vec::new(),
        env: BTreeMap::new(),
        stdin_file: None,
        seed: 0,
        epoch_ms: now_ms() as u64,
        fuel: 500_000_000,
        out: None,
        audit_log: None,
        signing_key: None,
        actor: None,
        user: None,
        tool_name: None,
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--arg" => a.args.push(it.next().unwrap_or_else(|| usage_and_exit("--arg needs a value"))),
            "--env" => {
                let kv = it.next().unwrap_or_else(|| usage_and_exit("--env needs a value"));
                match kv.split_once('=') {
                    Some((k, v)) => {
                        a.env.insert(k.to_string(), v.to_string());
                    }
                    None => usage_and_exit("--env must be K=V"),
                }
            }
            "--stdin-file" => {
                a.stdin_file = Some(PathBuf::from(it.next().unwrap_or_else(|| usage_and_exit("--stdin-file needs a value"))))
            }
            "--seed" => {
                a.seed = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage_and_exit("--seed needs a u64"))
            }
            "--epoch-ms" => {
                a.epoch_ms = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage_and_exit("--epoch-ms needs a u64"))
            }
            "--fuel" => {
                a.fuel = it
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage_and_exit("--fuel needs a u64"))
            }
            "--out" => a.out = Some(PathBuf::from(it.next().unwrap_or_else(|| usage_and_exit("--out needs a value")))),
            "--audit-log" => {
                a.audit_log = Some(PathBuf::from(it.next().unwrap_or_else(|| usage_and_exit("--audit-log needs a value"))))
            }
            "--signing-key" => {
                a.signing_key = Some(PathBuf::from(it.next().unwrap_or_else(|| usage_and_exit("--signing-key needs a value"))))
            }
            "--actor" => a.actor = Some(it.next().unwrap_or_else(|| usage_and_exit("--actor needs a value"))),
            "--user" => a.user = Some(it.next().unwrap_or_else(|| usage_and_exit("--user needs a value"))),
            "--tool-name" => a.tool_name = Some(it.next().unwrap_or_else(|| usage_and_exit("--tool-name needs a value"))),
            other => usage_and_exit(&format!("unknown flag: {other}")),
        }
    }
    a
}

fn run_record(a: RunArgs) -> std::process::ExitCode {
    let stdin = match &a.stdin_file {
        Some(p) => match std::fs::read(p) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("kriya-run-wasm: reading --stdin-file {}: {e}", p.display());
                return std::process::ExitCode::from(2);
            }
        },
        None => Vec::new(),
    };

    // WASI's `wasi:cli/environment.get-arguments()` carries no implicit argv[0] (unlike a native
    // process) — it's whatever the embedder chooses to hand the guest. This CLI follows Wasmtime's
    // own `wasmtime run <module> <args...>` convention: argv[0] is the module's file name, so a
    // guest written the normal Rust way (`std::env::args().nth(1)`/`.skip(1)`) sees exactly what it
    // expects. The synthesized argv[0] is recorded as part of `RunBundle.args` (and therefore
    // covered by `args_hash`) — a `--verify` re-run replays the SAME argv, not a re-derived one.
    let module_name = a
        .module
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("module")
        .to_string();
    let mut full_args = vec![module_name];
    full_args.extend(a.args);

    let inputs = ExecutionInputs {
        args: full_args,
        env: a.env,
        stdin,
        seed: a.seed,
        epoch_ms: a.epoch_ms,
        fuel_limit: a.fuel,
    };

    let bundle = match build_and_record(&a.module, inputs) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("kriya-run-wasm: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let out_path = a.out.clone().unwrap_or_else(|| {
        let mut p = a.module.clone();
        let name = format!(
            "{}.kriya-exec-bundle.json",
            p.file_stem().and_then(|s| s.to_str()).unwrap_or("run")
        );
        p.set_file_name(name);
        p
    });
    if let Err(e) = std::fs::write(&out_path, serde_json::to_string_pretty(&bundle).unwrap_or_default()) {
        eprintln!("kriya-run-wasm: writing bundle {}: {e}", out_path.display());
        return std::process::ExitCode::FAILURE;
    }

    // Emit the kriya.exec.deterministic receipt — binds ONLY the bundle's hash + summary fields
    // (module_sha256, fuel, output hashes), never any content (module doc: C1 discipline).
    let audit_log = a.audit_log.unwrap_or_else(|| default_audit_dir().join("run-wasm.jsonl"));
    let signer = match &a.signing_key {
        Some(key_path) => match Signer::with_identity(key_path, audit_log.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("kriya-run-wasm: failed to load signing identity {key_path:?}: {e}");
                return std::process::ExitCode::FAILURE;
            }
        },
        None => Signer::with_log_path(audit_log.clone()),
    };
    let actor = a.actor.map(|agent| {
        let user = a
            .user
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "unknown".to_string());
        Actor::new(agent, user)
    });
    let params = bundle.receipt_params(a.tool_name.as_deref());
    let signed = signer.record(
        Receipt::new(uuid::Uuid::new_v4().to_string(), ACTION_ID.to_string(), params, bundle.success, now_ms())
            .with_actor(actor),
    );

    println!(
        "[kriya-run-wasm] recorded {} (module {}, fuel {}, stdout {} bytes, stderr {} bytes) -> {}",
        if bundle.success { "OK" } else { "FAILED" },
        bundle.module_sha256,
        bundle.fuel_consumed,
        bundle.stdout_len,
        bundle.stderr_len,
        out_path.display(),
    );
    println!(
        "[kriya-run-wasm] receipt {} -> {}",
        signed.receipt.step_id,
        audit_log.display()
    );
    println!("[kriya-run-wasm] bundle_hash = {}", bundle.bundle_hash());
    if bundle.success {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

fn run_verify(bundle_path: PathBuf, module_override: Option<PathBuf>) -> std::process::ExitCode {
    let report = verify_bundle_file(&bundle_path, module_override.as_deref());
    for field in &report.fields {
        println!(
            "[kriya-run-wasm] {}: {} — {}",
            field.field,
            if field.ok { "OK" } else { "FAIL" },
            field.detail
        );
    }
    if let Some(reexec) = &report.reexecuted {
        println!(
            "[kriya-run-wasm] re-execution: fuel_consumed={} stdout_hash={} stderr_hash={}",
            reexec.fuel_consumed, reexec.stdout_hash, reexec.stderr_hash
        );
    }
    println!(
        "[kriya-run-wasm] {}: {}",
        bundle_path.display(),
        if report.ok { "OK — deterministic re-execution matched" } else { "FAIL" }
    );
    if report.ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        usage_and_exit("missing subcommand");
    };
    match first.as_str() {
        "run" => {
            let Some(module) = args.next() else {
                usage_and_exit("run needs a <module.wasm>");
            };
            if module.starts_with("--") {
                usage_and_exit("run needs a <module.wasm> before any flags");
            }
            run_record(parse_run_args(PathBuf::from(module), args))
        }
        "--verify" => {
            let Some(bundle) = args.next() else {
                usage_and_exit("--verify needs a <bundle.json>");
            };
            let mut module_override = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--module" => {
                        module_override = Some(PathBuf::from(args.next().unwrap_or_else(|| usage_and_exit("--module needs a value"))))
                    }
                    other => usage_and_exit(&format!("unknown flag: {other}")),
                }
            }
            run_verify(PathBuf::from(bundle), module_override)
        }
        other => usage_and_exit(&format!("unknown subcommand: {other}")),
    }
}

