//! F4-wasm (doc 28 §F4) build spec item 4: "a tier option 'prefer deterministic lane' — when a
//! tool has a WASM variant registered, route to it; receipted either way."
//!
//! [`Governor`](crate::mcp::governor::Governor) already composes over a single
//! `Box<dyn ActionExecutor>` — the seam the module doc ("The MCP server doesn't know *how* an app
//! executes an action... So execution is a trait") deliberately left for exactly this kind of
//! decision. [`WasmRoutingExecutor`] is that composition: it wraps whatever executor a host was
//! already using, consults [`Policy::resolve_wasm_variant`] per call, and — when a variant is
//! registered AND the tier is on — runs the action through [`super::build_and_record`] instead,
//! emitting the `kriya.exec.deterministic` receipt via the SAME signer the host's normal action
//! receipts already use. When no variant applies, it's a pure passthrough: byte-identical to not
//! having this executor in the chain at all.
//!
//! **The params↔stdin convention.** A WASM variant's guest receives the action's `params` JSON,
//! canonically serialized, as its stdin — the same shape every governed action already carries, so
//! authoring a WASM variant for an existing tool needs no new param format. `argv[0]` is the
//! module's file name (matching `kriya-run-wasm`'s own convention); no other argv/env is
//! synthesized in v1 (a tool needing more can still opt out by not registering a variant).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::audit::{now_ms, Actor, Receipt, Signer};
use crate::mcp::executor::{ActionExecutor, ActionOutcome};
use crate::permissions::Policy;

use super::bundle::ACTION_ID;
use super::engine::{build_and_record, ExecutionInputs};

/// Default fuel budget for a policy-routed deterministic-execution call. Generous for the two
/// example tools' workloads; a host embedding this for heavier tools should build with
/// [`WasmRoutingExecutor::with_fuel_limit`] instead of relying on this default.
pub const DEFAULT_FUEL_LIMIT: u64 = 500_000_000;

pub struct WasmRoutingExecutor {
    inner: Box<dyn ActionExecutor>,
    policy: Arc<Policy>,
    signer: Arc<Signer>,
    actor: Option<Actor>,
    fuel_limit: u64,
}

impl WasmRoutingExecutor {
    pub fn new(inner: Box<dyn ActionExecutor>, policy: Arc<Policy>, signer: Arc<Signer>, actor: Option<Actor>) -> Self {
        Self {
            inner,
            policy,
            signer,
            actor,
            fuel_limit: DEFAULT_FUEL_LIMIT,
        }
    }

    /// Override the fuel budget every deterministic-execution call runs under. Chainable on `new`.
    pub fn with_fuel_limit(mut self, fuel_limit: u64) -> Self {
        self.fuel_limit = fuel_limit;
        self
    }

    fn emit_receipt(&self, bundle: &super::RunBundle, action_id: &str) {
        let params = bundle.receipt_params(Some(action_id));
        self.signer.record(
            Receipt::new(uuid::Uuid::new_v4().to_string(), ACTION_ID.to_string(), params, bundle.success, now_ms())
                .with_actor(self.actor.clone()),
        );
    }
}

impl ActionExecutor for WasmRoutingExecutor {
    fn execute(&mut self, action_id: &str, params: &Value) -> ActionOutcome {
        let Some(module_path) = self.policy.resolve_wasm_variant(action_id).map(PathBuf::from) else {
            return self.inner.execute(action_id, params);
        };

        let module_name = module_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("module")
            .to_string();
        let stdin = serde_json::to_vec(params).unwrap_or_default();
        // A fresh seed per REAL call — genuine entropy at record time, exactly once; a
        // `--verify` re-run replays this SAME recorded seed, which is what makes it deterministic
        // from then on (module doc: "seed + epoch supplied as recorded inputs").
        let seed: u64 = rand::random();
        let inputs = ExecutionInputs {
            args: vec![module_name],
            env: BTreeMap::new(),
            stdin,
            seed,
            epoch_ms: now_ms() as u64,
            fuel_limit: self.fuel_limit,
        };

        match build_and_record(&module_path, inputs) {
            Ok(bundle) => {
                self.emit_receipt(&bundle, action_id);
                let data = serde_json::json!({
                    "lane": "deterministic",
                    "bundle_hash": bundle.bundle_hash(),
                    "stdout_hash": bundle.stdout_hash,
                    "stdout_len": bundle.stdout_len,
                    "fuel_consumed": bundle.fuel_consumed,
                });
                if bundle.success {
                    ActionOutcome::ok(data)
                } else {
                    ActionOutcome::failed(
                        bundle
                            .error
                            .clone()
                            .unwrap_or_else(|| "deterministic-execution lane: guest reported failure".to_string()),
                    )
                }
            }
            // Fails CLOSED for this one action (never silently falls back to the inner executor —
            // a registered-but-broken variant should surface loudly, not mask itself behind the
            // tool's normal path, which could hide exactly the kind of divergence this lane exists
            // to catch).
            Err(e) => ActionOutcome::failed(format!("deterministic-execution lane: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::executor::FnExecutor;
    use std::path::Path;

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf()
    }

    fn build_text_transform() -> PathBuf {
        let dir = repo_root().join("examples/f4-wasm-tools/text-transform");
        let status = std::process::Command::new("cargo")
            .args(["build", "--release", "--target", "wasm32-wasip2"])
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(status.success());
        dir.join("target/wasm32-wasip2/release/text-transform.wasm")
    }

    fn test_policy(module: &Path) -> Arc<Policy> {
        let yaml = format!(
            "rules: [{{action: \"*\", allow: true}}]\nexec:\n  prefer_deterministic_lane: true\n  wasm_variants:\n    routed_tool: {:?}\n",
            module.display().to_string()
        );
        Arc::new(serde_yaml::from_str(&yaml).expect("policy yaml parses"))
    }

    fn test_signer() -> Arc<Signer> {
        Arc::new(Signer::with_log_path(std::env::temp_dir().join(format!(
            "kriya-wasm-routing-test-{}.jsonl",
            uuid::Uuid::new_v4()
        ))))
    }

    /// An action WITH a registered WASM variant routes to the deterministic lane — the wrapped
    /// (inner) executor is never called for it.
    #[test]
    fn routes_a_registered_action_to_the_deterministic_lane() {
        let module = build_text_transform();
        let policy = test_policy(&module);
        let signer = test_signer();

        let inner_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let inner_flag = inner_called.clone();
        let inner = FnExecutor(move |_id: &str, _p: &Value| {
            inner_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            ActionOutcome::ok(Value::Null)
        });

        let mut routing = WasmRoutingExecutor::new(Box::new(inner), policy, signer, None);
        // The guest reads stdin (the canonical params JSON) and, given no recognized `--upper`/
        // `--lower`/etc mode in argv, exits non-zero — fine for THIS test, which only asserts
        // ROUTING happened, not the tool's business logic (that's covered in
        // `tests/wasmexec_determinism.rs`).
        let outcome = routing.execute("routed_tool", &serde_json::json!({"x": 1}));
        assert!(!inner_called.load(std::sync::atomic::Ordering::SeqCst), "the inner executor must not run for a routed action");
        assert_eq!(outcome.data["lane"], "deterministic");
    }

    /// An action with NO registered variant is a pure passthrough to the wrapped executor.
    #[test]
    fn passes_through_an_unregistered_action_to_the_inner_executor() {
        let module = build_text_transform();
        let policy = test_policy(&module);
        let signer = test_signer();

        let inner = FnExecutor(|_id: &str, _p: &Value| ActionOutcome::ok(serde_json::json!({"inner": true})));
        let mut routing = WasmRoutingExecutor::new(Box::new(inner), policy, signer, None);
        let outcome = routing.execute("some_other_action", &serde_json::json!({}));
        assert_eq!(outcome.data["inner"], true);
    }

    /// Routing a registered action ALSO signs a `kriya.exec.deterministic` receipt — "receipted
    /// either way" (doc 28 §F4 item 4): whichever path executed this call, a receipt landed.
    #[test]
    fn routing_emits_the_deterministic_receipt() {
        let module = build_text_transform();
        let policy = test_policy(&module);
        let signer = test_signer();
        let log_path = signer.log_path().to_path_buf();

        let inner = FnExecutor(|_id: &str, _p: &Value| ActionOutcome::ok(Value::Null));
        let mut routing = WasmRoutingExecutor::new(Box::new(inner), policy, signer, None);
        let _ = routing.execute("routed_tool", &serde_json::json!({}));

        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        assert!(log.contains(ACTION_ID), "the deterministic-execution receipt must be signed and appended");
    }
}
