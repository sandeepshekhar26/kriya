//! The execution + verification engine: runs a WASI-p2 component under the deterministic
//! configuration (module doc), and re-runs a recorded [`RunBundle`] to hash-compare on verify.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::bundle::{hash_args, hash_env, RunBundle, MAX_INPUT_BYTES, SCHEMA};
use super::determinism::{insecure_rng, secure_rng, sha256_hex, FixedWallClock, SteppedMonotonicClock};

/// The recorded inputs to one deterministic-execution run — everything [`RunBundle`] needs to be
/// re-runnable, before any hashing happens.
#[derive(Debug, Clone)]
pub struct ExecutionInputs {
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub stdin: Vec<u8>,
    pub seed: u64,
    pub epoch_ms: u64,
    pub fuel_limit: u64,
}

/// What one execution produced, before it's folded into a [`RunBundle`].
#[derive(Debug, Clone)]
pub struct ExecutionOutputs {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub fuel_consumed: u64,
    pub success: bool,
    pub error: Option<String>,
}

struct GuestState {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl WasiView for GuestState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// The one Wasmtime `Config` this entire lane ever builds — every knob in the module doc's
/// "deterministic configuration" section, in one place so `config_digest`'s descriptor string
/// (mod.rs) and the ACTUAL config can never silently drift apart from each other.
pub fn deterministic_config() -> Config {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.cranelift_nan_canonicalization(true);
    config.wasm_threads(false);
    config.wasm_relaxed_simd(false);
    config
}

/// Execute `component_bytes` once under the deterministic configuration. Pure with respect to the
/// HOST: no ambient clock/RNG/filesystem/network reaches the guest — every source of "what time is
/// it" / "give me randomness" is a function of `inputs`.
pub fn execute_component(component_bytes: &[u8], inputs: &ExecutionInputs) -> Result<ExecutionOutputs, String> {
    let config = deterministic_config();
    let engine = Engine::new(&config).map_err(|e| format!("building wasmtime engine: {e}"))?;
    let component = Component::from_binary(&engine, component_bytes)
        .map_err(|e| format!("loading WASI-p2 component (is this a component, not a core module? \
            rebuild with `cargo build --target wasm32-wasip2`): {e}"))?;

    let stdout_pipe = MemoryOutputPipe::new(64 * 1024 * 1024);
    let stderr_pipe = MemoryOutputPipe::new(64 * 1024 * 1024);

    let mut builder = WasiCtxBuilder::new();
    builder
        .args(&inputs.args)
        .envs(&inputs.env.iter().collect::<Vec<_>>())
        .stdin(MemoryInputPipe::new(inputs.stdin.clone()))
        .stdout(stdout_pipe.clone())
        .stderr(stderr_pipe.clone())
        .wall_clock(FixedWallClock { epoch_ms: inputs.epoch_ms })
        .monotonic_clock(SteppedMonotonicClock::new())
        .insecure_random_seed(inputs.seed as u128)
        .secure_random(secure_rng(inputs.seed))
        .insecure_random(insecure_rng(inputs.seed));
    // Deliberately NOT called: `inherit_network`, any `preopened_dir` — this lane grants no
    // filesystem or network access (module doc: "no filesystem or network access").
    let wasi = builder.build();

    let mut store = Store::new(
        &engine,
        GuestState {
            table: ResourceTable::new(),
            wasi,
        },
    );
    store
        .set_fuel(inputs.fuel_limit)
        .map_err(|e| format!("setting fuel: {e}"))?;

    let mut linker: Linker<GuestState> = Linker::new(&engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| format!("linking WASI: {e}"))?;

    let command = Command::instantiate(&mut store, &component, &linker)
        .map_err(|e| format!("instantiating component: {e}"))?;

    let fuel_before = store.get_fuel().unwrap_or(inputs.fuel_limit);
    let run_result = command.wasi_cli_run().call_run(&mut store);
    let fuel_after = store.get_fuel().unwrap_or(0);
    let fuel_consumed = fuel_before.saturating_sub(fuel_after);

    let (success, error) = match run_result {
        Ok(Ok(())) => (true, None),
        Ok(Err(())) => (false, Some("guest wasi:cli/run returned failure".to_string())),
        Err(e) => (false, Some(format!("trap or host error: {e}"))),
    };

    Ok(ExecutionOutputs {
        stdout: stdout_pipe.contents().to_vec(),
        stderr: stderr_pipe.contents().to_vec(),
        fuel_consumed,
        success,
        error,
    })
}

/// Why [`build_and_record`] refused to produce a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    /// The combined args+env+stdin byte size exceeded [`MAX_INPUT_BYTES`] — an HONEST refusal
    /// (module doc): a truncated recording would verify against inputs that were never the real
    /// run, which is worse than refusing outright.
    InputTooLarge { size: usize, cap: usize },
    Io(String),
    Execution(String),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::InputTooLarge { size, cap } => write!(
                f,
                "refusing to record: combined args+env+stdin is {size} bytes, over the {cap}-byte \
                 re-run size cap (a size-capped bundle must be re-runnable — a truncated recording \
                 would silently verify against inputs that were never the real run)"
            ),
            RecordError::Io(e) => write!(f, "io error: {e}"),
            RecordError::Execution(e) => write!(f, "execution error: {e}"),
        }
    }
}

/// Combined size of everything [`build_and_record`] must store re-runnably, for the [`MAX_INPUT_BYTES`]
/// check.
fn combined_input_size(inputs: &ExecutionInputs) -> usize {
    let args_size: usize = inputs.args.iter().map(|a| a.len()).sum();
    let env_size: usize = inputs.env.iter().map(|(k, v)| k.len() + v.len()).sum();
    args_size + env_size + inputs.stdin.len()
}

/// Load `module_path`, execute it under the deterministic configuration, and build the full
/// [`RunBundle`] — everything needed to write it to disk and/or emit the `kriya.exec.deterministic`
/// receipt. Does NOT write the bundle file or sign anything (the caller — the CLI or the policy
/// routing executor — owns those side effects); this function is the pure record step.
pub fn build_and_record(module_path: &Path, inputs: ExecutionInputs) -> Result<RunBundle, RecordError> {
    let size = combined_input_size(&inputs);
    if size > MAX_INPUT_BYTES {
        return Err(RecordError::InputTooLarge { size, cap: MAX_INPUT_BYTES });
    }
    let component_bytes = std::fs::read(module_path)
        .map_err(|e| RecordError::Io(format!("reading module {}: {e}", module_path.display())))?;
    let module_sha256 = sha256_hex(&component_bytes);

    let outputs = execute_component(&component_bytes, &inputs).map_err(RecordError::Execution)?;

    let args_hash = hash_args(&inputs.args);
    let env_hash = hash_env(&inputs.env);
    let stdin_hash = sha256_hex(&inputs.stdin);

    Ok(RunBundle {
        schema: SCHEMA.to_string(),
        module_sha256,
        module_path: module_path.display().to_string(),
        wasmtime_version: super::WASMTIME_VERSION.to_string(),
        config_digest: super::config_digest(),
        args_hash,
        args: inputs.args,
        env_hash,
        env: inputs.env,
        stdin_hex: hex::encode(&inputs.stdin),
        stdin_hash,
        stdin_len: inputs.stdin.len(),
        seed: inputs.seed,
        epoch_ms: inputs.epoch_ms,
        fuel_limit: inputs.fuel_limit,
        fuel_consumed: outputs.fuel_consumed,
        stdout_hash: sha256_hex(&outputs.stdout),
        stdout_len: outputs.stdout.len(),
        stderr_hash: sha256_hex(&outputs.stderr),
        stderr_len: outputs.stderr.len(),
        success: outputs.success,
        error: outputs.error,
        recorded_ts_ms: crate::audit::now_ms(),
    })
}

/// One checked field in a [`VerifyReport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyFieldResult {
    pub field: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// The full outcome of re-executing a recorded bundle and hash-comparing it — what
/// `kriya-run-wasm --verify` and the console's `verify-exec` both print/return.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub ok: bool,
    pub fields: Vec<VerifyFieldResult>,
    /// The freshly re-executed bundle (for a caller that wants the raw numbers), `None` if
    /// re-execution never happened (a pre-execution check — e.g. a tampered stdin_hash — already
    /// failed closed).
    pub reexecuted: Option<RunBundle>,
}

impl VerifyReport {
    fn push(&mut self, field: &'static str, ok: bool, detail: impl Into<String>) {
        self.fields.push(VerifyFieldResult { field, ok, detail: detail.into() });
        if !ok {
            self.ok = false;
        }
    }

    fn fail_closed(reason: VerifyFieldResult) -> Self {
        VerifyReport { ok: false, fields: vec![reason], reexecuted: None }
    }
}

/// Re-execute a persisted [`RunBundle`] (loaded fresh from `bundle_path` — the "file round-trip"
/// case is just the normal path here, nothing special) and hash-compare against what it recorded.
/// `module_override` lets a caller point at a moved/renamed module file; when `None`, the bundle's
/// own `module_path` is used as recorded.
pub fn verify_bundle_file(bundle_path: &Path, module_override: Option<&Path>) -> VerifyReport {
    let text = match std::fs::read_to_string(bundle_path) {
        Ok(t) => t,
        Err(e) => {
            return VerifyReport::fail_closed(VerifyFieldResult {
                field: "bundle_file",
                ok: false,
                detail: format!("cannot read {}: {e}", bundle_path.display()),
            })
        }
    };
    let bundle: RunBundle = match serde_json::from_str(&text) {
        Ok(b) => b,
        Err(e) => {
            return VerifyReport::fail_closed(VerifyFieldResult {
                field: "bundle_file",
                ok: false,
                detail: format!("{} is not a valid kriya-exec-bundle: {e}", bundle_path.display()),
            })
        }
    };
    verify_bundle(&bundle, module_override)
}

/// The in-memory counterpart of [`verify_bundle_file`] — takes an already-parsed [`RunBundle`].
pub fn verify_bundle(bundle: &RunBundle, module_override: Option<&Path>) -> VerifyReport {
    let mut report = VerifyReport { ok: true, fields: Vec::new(), reexecuted: None };

    if bundle.schema != SCHEMA {
        report.push("schema", false, format!("unrecognized schema {:?}, expected {SCHEMA:?}", bundle.schema));
        return report;
    }
    report.push("schema", true, SCHEMA.to_string());

    // Internal tamper check FIRST, before spending any CPU on re-execution: the recorded
    // args/env/stdin bytes must themselves still match their own recorded hashes. A bundle whose
    // `stdin_hex` was edited without updating `stdin_hash` (or vice versa) is caught here, with the
    // precise reason, rather than surfacing as a confusing downstream stdout mismatch.
    let stdin_bytes = match hex::decode(&bundle.stdin_hex) {
        Ok(b) => b,
        Err(e) => {
            report.push("stdin_hex", false, format!("not valid hex: {e}"));
            return report;
        }
    };
    let recomputed_stdin_hash = sha256_hex(&stdin_bytes);
    report.push(
        "stdin_hash",
        recomputed_stdin_hash == bundle.stdin_hash,
        if recomputed_stdin_hash == bundle.stdin_hash {
            "matches recorded stdin bytes".to_string()
        } else {
            format!("bundle tampered: recorded stdin_hash {} but the stored stdin bytes hash to {recomputed_stdin_hash}", bundle.stdin_hash)
        },
    );

    let recomputed_args_hash = hash_args(&bundle.args);
    report.push(
        "args_hash",
        recomputed_args_hash == bundle.args_hash,
        if recomputed_args_hash == bundle.args_hash {
            "matches recorded args".to_string()
        } else {
            format!("bundle tampered: recorded args_hash {} but the stored args hash to {recomputed_args_hash}", bundle.args_hash)
        },
    );

    let recomputed_env_hash = hash_env(&bundle.env);
    report.push(
        "env_hash",
        recomputed_env_hash == bundle.env_hash,
        if recomputed_env_hash == bundle.env_hash {
            "matches recorded env".to_string()
        } else {
            format!("bundle tampered: recorded env_hash {} but the stored env hashes to {recomputed_env_hash}", bundle.env_hash)
        },
    );

    if !report.ok {
        // The bundle's own internal consistency already broke — re-executing against
        // self-contradictory recorded inputs would only manufacture a misleading second failure.
        return report;
    }

    let module_path: PathBuf = module_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(&bundle.module_path));
    let component_bytes = match std::fs::read(&module_path) {
        Ok(b) => b,
        Err(e) => {
            report.push("module_sha256", false, format!("cannot read module {}: {e}", module_path.display()));
            return report;
        }
    };
    let module_sha256 = sha256_hex(&component_bytes);
    report.push(
        "module_sha256",
        module_sha256 == bundle.module_sha256,
        if module_sha256 == bundle.module_sha256 {
            "module bytes match the recorded hash".to_string()
        } else {
            format!("module TAMPERED or replaced: recorded {} but {} now hashes to {module_sha256}", bundle.module_sha256, module_path.display())
        },
    );
    if !report.ok {
        return report;
    }

    report.push(
        "wasmtime_version",
        bundle.wasmtime_version == super::WASMTIME_VERSION,
        if bundle.wasmtime_version == super::WASMTIME_VERSION {
            "same Wasmtime version as recorded".to_string()
        } else {
            format!(
                "recorded under wasmtime {}, this verifier runs wasmtime {} — cross-version \
                 reproducibility is NOT claimed by this lane (see docs/TRUST.md)",
                bundle.wasmtime_version,
                super::WASMTIME_VERSION
            )
        },
    );
    report.push(
        "config_digest",
        bundle.config_digest == super::config_digest(),
        if bundle.config_digest == super::config_digest() {
            "identical deterministic configuration".to_string()
        } else {
            "the deterministic-configuration digest differs from this verifier's — the bundle was \
             recorded under different knobs"
                .to_string()
        },
    );
    if !report.ok {
        return report;
    }

    let inputs = ExecutionInputs {
        args: bundle.args.clone(),
        env: bundle.env.clone(),
        stdin: stdin_bytes,
        seed: bundle.seed,
        epoch_ms: bundle.epoch_ms,
        fuel_limit: bundle.fuel_limit,
    };
    let outputs = match execute_component(&component_bytes, &inputs) {
        Ok(o) => o,
        Err(e) => {
            report.push("execution", false, format!("re-execution failed: {e}"));
            return report;
        }
    };

    let stdout_hash = sha256_hex(&outputs.stdout);
    let stderr_hash = sha256_hex(&outputs.stderr);

    report.push(
        "stdout_hash",
        stdout_hash == bundle.stdout_hash,
        if stdout_hash == bundle.stdout_hash {
            "byte-identical stdout".to_string()
        } else {
            format!("stdout MISMATCH: recorded {} but re-execution produced {stdout_hash}", bundle.stdout_hash)
        },
    );
    report.push(
        "stderr_hash",
        stderr_hash == bundle.stderr_hash,
        if stderr_hash == bundle.stderr_hash {
            "byte-identical stderr".to_string()
        } else {
            format!("stderr MISMATCH: recorded {} but re-execution produced {stderr_hash}", bundle.stderr_hash)
        },
    );
    report.push(
        "fuel_consumed",
        outputs.fuel_consumed == bundle.fuel_consumed,
        if outputs.fuel_consumed == bundle.fuel_consumed {
            format!("identical fuel consumption ({} units)", bundle.fuel_consumed)
        } else {
            format!(
                "fuel MISMATCH: recorded {} but re-execution consumed {} — the execution path diverged",
                bundle.fuel_consumed, outputs.fuel_consumed
            )
        },
    );
    report.push(
        "success",
        outputs.success == bundle.success,
        format!("recorded success={}, re-execution success={}", bundle.success, outputs.success),
    );

    report.reexecuted = Some(RunBundle {
        schema: SCHEMA.to_string(),
        module_sha256,
        module_path: module_path.display().to_string(),
        wasmtime_version: super::WASMTIME_VERSION.to_string(),
        config_digest: super::config_digest(),
        args_hash: recomputed_args_hash,
        args: bundle.args.clone(),
        env_hash: recomputed_env_hash,
        env: bundle.env.clone(),
        stdin_hex: bundle.stdin_hex.clone(),
        stdin_hash: recomputed_stdin_hash,
        stdin_len: bundle.stdin_len,
        seed: bundle.seed,
        epoch_ms: bundle.epoch_ms,
        fuel_limit: bundle.fuel_limit,
        fuel_consumed: outputs.fuel_consumed,
        stdout_hash,
        stdout_len: outputs.stdout.len(),
        stderr_hash,
        stderr_len: outputs.stderr.len(),
        success: outputs.success,
        error: outputs.error,
        recorded_ts_ms: crate::audit::now_ms(),
    });

    report
}
