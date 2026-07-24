//! `kriya-llm-proxy` (F1, doc 28 §F1) — a local governed reverse proxy in front of a local
//! OpenAI-compatible / Ollama-native inference server. Point any OpenAI client or Ollama client at
//! this proxy's port instead of the model server's own port, and every governed completion signs a
//! receipt (`kriya.model.identity` / `kriya.model.gate` / `kriya.model.serve`) via the same Ed25519
//! signer path every other kriya lane uses.
//!
//! ## Usage
//! ```text
//! kriya-llm-proxy serve [OPTIONS]
//!
//!   --listen <host:port>   where this proxy listens (default 127.0.0.1:11435)
//!   --upstream <url>       the real inference server (default http://localhost:11434 — Ollama's
//!                          default port)
//!   --policy <p.yaml>      the SAME agent-policy.yaml every other governed binary loads; reads its
//!                          optional `model:` section. Default: built-in ModelPolicy::default()
//!                          (unknown_model: warn — observation-first, nothing blocks)
//!   --actor <agent>        agent identity stamped into every signed receipt. Omit → unattributed
//!   --user <user>          operator the run acts for (default: $USER). Only used with --actor
//!   --audit-log <path>     signed-receipt JSONL log. Default: ~/.kriya/audit/llm-proxy.jsonl (the
//!                          standard dir the Console auto-discovers — R27)
//!   --signing-key <path>   persist the Ed25519 identity here (0600) for a stable trust anchor
//!                          across runs. Default: ephemeral per-process key
//!   --models-dir <path>    override the Ollama models directory (default ~/.ollama/models, or
//!                          $OLLAMA_MODELS)
//!   --cache-path <path>    the file-hash digest cache (default ~/.kriya/llm-proxy/digest-cache.json)
//!   --model-path <name>=<path>   (repeatable) map a served model NAME to a local file, for the
//!                          file-hash-cache resolution path (llama.cpp-style servers with no Ollama
//!                          manifest) — populate the cache first with `hash-model`
//!
//! kriya-llm-proxy hash-model <path> [--cache-path <path>]
//!   Compute (once, off the proxy's request path) and cache a local model file's sha256 digest, so
//!   `--model-path` lookups resolve without ever hashing on the hot path.
//! ```
//!
//! **Bypass honesty**: this proxy governs ONLY clients configured to point at it (`OPENAI_BASE_URL`,
//! `OLLAMA_HOST`, or your client's base-url setting). A process that connects straight to the
//! upstream inference server is invisible to it — see `kriya::llm`'s module doc and the runtime
//! README's wiring section before claiming broader coverage.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use kriya::audit::{default_audit_dir, Actor, Signer};
use kriya::llm::manifest;
use kriya::llm::proxy::{LlmProxy, ProxyOptions};
use kriya::permissions::{ModelPolicy, Policy};

const DEFAULT_LISTEN: &str = "127.0.0.1:11435";
const DEFAULT_UPSTREAM: &str = "http://localhost:11434";

fn usage_and_exit(msg: &str) -> ! {
    eprintln!("kriya-llm-proxy: {msg}");
    eprintln!(
        "usage: kriya-llm-proxy serve [--listen host:port] [--upstream url] [--policy p.yaml] \
         [--actor <agent>] [--user <user>] [--audit-log path] [--signing-key path] \
         [--models-dir path] [--cache-path path] [--model-path name=path]...\n       \
         kriya-llm-proxy hash-model <path> [--cache-path path]"
    );
    exit(2);
}

struct ServeArgs {
    listen: String,
    upstream: String,
    policy: Option<PathBuf>,
    actor: Option<String>,
    user: Option<String>,
    audit_log: Option<PathBuf>,
    signing_key: Option<PathBuf>,
    models_dir: Option<PathBuf>,
    cache_path: Option<PathBuf>,
    model_paths: HashMap<String, PathBuf>,
}

fn parse_serve_args(mut it: impl Iterator<Item = String>) -> ServeArgs {
    let mut args = ServeArgs {
        listen: DEFAULT_LISTEN.to_string(),
        upstream: DEFAULT_UPSTREAM.to_string(),
        policy: None,
        actor: None,
        user: None,
        audit_log: None,
        signing_key: None,
        models_dir: None,
        cache_path: None,
        model_paths: HashMap::new(),
    };
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--listen" => {
                args.listen = it
                    .next()
                    .unwrap_or_else(|| usage_and_exit("--listen needs a value"))
            }
            "--upstream" => {
                args.upstream = it
                    .next()
                    .unwrap_or_else(|| usage_and_exit("--upstream needs a value"))
            }
            "--policy" => {
                args.policy = Some(PathBuf::from(
                    it.next()
                        .unwrap_or_else(|| usage_and_exit("--policy needs a value")),
                ))
            }
            "--actor" => {
                args.actor = Some(
                    it.next()
                        .unwrap_or_else(|| usage_and_exit("--actor needs a value")),
                )
            }
            "--user" => {
                args.user = Some(
                    it.next()
                        .unwrap_or_else(|| usage_and_exit("--user needs a value")),
                )
            }
            "--audit-log" => {
                args.audit_log =
                    Some(PathBuf::from(it.next().unwrap_or_else(|| {
                        usage_and_exit("--audit-log needs a value")
                    })))
            }
            "--signing-key" => {
                args.signing_key =
                    Some(PathBuf::from(it.next().unwrap_or_else(|| {
                        usage_and_exit("--signing-key needs a value")
                    })))
            }
            "--models-dir" => {
                args.models_dir =
                    Some(PathBuf::from(it.next().unwrap_or_else(|| {
                        usage_and_exit("--models-dir needs a value")
                    })))
            }
            "--cache-path" => {
                args.cache_path =
                    Some(PathBuf::from(it.next().unwrap_or_else(|| {
                        usage_and_exit("--cache-path needs a value")
                    })))
            }
            "--model-path" => {
                let kv = it
                    .next()
                    .unwrap_or_else(|| usage_and_exit("--model-path needs a value"));
                match kv.split_once('=') {
                    Some((name, path)) => {
                        args.model_paths
                            .insert(name.to_string(), PathBuf::from(path));
                    }
                    None => usage_and_exit("--model-path must be name=path"),
                }
            }
            other => usage_and_exit(&format!("unknown flag: {other}")),
        }
    }
    args
}

fn run_serve(args: ServeArgs) -> std::io::Result<()> {
    let policy: Arc<ModelPolicy> = Arc::new(match &args.policy {
        Some(p) => Policy::load_or_default(p)
            .model()
            .cloned()
            .unwrap_or_default(),
        None => ModelPolicy::default(),
    });

    let audit_log = args
        .audit_log
        .unwrap_or_else(|| default_audit_dir().join("llm-proxy.jsonl"));
    let signer = Arc::new(match &args.signing_key {
        Some(key_path) => match Signer::with_identity(key_path, audit_log.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("kriya-llm-proxy: failed to load signing identity {key_path:?}: {e}");
                exit(1);
            }
        },
        None => Signer::with_log_path(audit_log.clone()),
    });

    let actor = args.actor.map(|agent| {
        let user = args
            .user
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_else(|| "unknown".to_string());
        Actor::new(agent, user)
    });

    let options = ProxyOptions {
        models_dir: args.models_dir.unwrap_or_else(manifest::ollama_models_dir),
        cache_path: args.cache_path.unwrap_or_else(manifest::default_cache_path),
        model_paths: args.model_paths,
    };

    let proxy = LlmProxy::spawn(
        &args.listen,
        args.upstream.clone(),
        options,
        signer.clone(),
        actor,
        policy,
    )?;

    eprintln!(
        "[kriya-llm-proxy] listening on 127.0.0.1:{} -> upstream {}",
        proxy.port(),
        args.upstream
    );
    eprintln!(
        "[kriya-llm-proxy] signed receipts -> {}",
        audit_log.display()
    );
    eprintln!(
        "[kriya-llm-proxy] governs ONLY clients pointed at this port — a direct connection to {} \
         is invisible to it (see kriya::llm's module doc)",
        args.upstream
    );

    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn run_hash_model(path: PathBuf, cache_path: Option<PathBuf>) -> std::io::Result<()> {
    let cache_path = cache_path.unwrap_or_else(manifest::default_cache_path);
    let digest = manifest::compute_and_cache_digest(&cache_path, &path)?;
    println!("{digest}  {}", path.display());
    eprintln!("[kriya-llm-proxy] cached in {}", cache_path.display());
    Ok(())
}

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(sub) = args.next() else {
        usage_and_exit("missing subcommand");
    };
    match sub.as_str() {
        "serve" => run_serve(parse_serve_args(args)),
        "hash-model" => {
            let Some(path) = args.next() else {
                usage_and_exit("hash-model needs a <path>");
            };
            if path.starts_with("--") {
                usage_and_exit("hash-model needs a <path> before any flags");
            }
            let mut cache_path = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--cache-path" => {
                        cache_path =
                            Some(PathBuf::from(args.next().unwrap_or_else(|| {
                                usage_and_exit("--cache-path needs a value")
                            })))
                    }
                    other => usage_and_exit(&format!("unknown flag: {other}")),
                }
            }
            run_hash_model(PathBuf::from(path), cache_path)
        }
        other => usage_and_exit(&format!("unknown subcommand: {other}")),
    }
}
