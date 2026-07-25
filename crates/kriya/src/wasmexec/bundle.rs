//! The [`RunBundle`] record format (`kriya-exec-bundle/1`) — see the [`super`] module doc for the
//! full rationale. One JSON file per recorded run; re-runnable by design (inputs are stored as
//! bytes, not just hashes), size-capped with an honest refusal rather than a silent truncation.

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

use super::determinism::sha256_hex;

pub const SCHEMA: &str = "kriya-exec-bundle/1";

/// The reserved `action_id` for the signed receipt this lane emits — binds to the bundle's own
/// hash only (never the bundle's content: inputs/outputs stay out of the signed receipt bytes,
/// per the "no content in receipts — hashes/counts/names only" discipline every F-item follows).
pub const ACTION_ID: &str = "kriya.exec.deterministic";

/// Combined byte budget across args + env + stdin. Inputs must be re-runnable (stored as raw
/// bytes in the bundle, not just hashed), so an unbounded input would make the bundle file itself
/// unbounded; above this cap, [`super::engine::build_and_record`] REFUSES to record rather than
/// silently truncating (a truncated "recording" would verify against inputs that were never the
/// real run — a worse failure mode than an honest refusal).
pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

/// One recorded (or re-verified) deterministic-execution run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunBundle {
    pub schema: String,
    /// sha256 hex of the WASI-p2 component's raw bytes, at record time.
    pub module_sha256: String,
    /// The module's path AS RECORDED — informational only; `--verify` re-hashes whatever module
    /// it's pointed at (by default the same path) and compares against `module_sha256`, so a
    /// changed/moved/tampered module is caught rather than trusted from this field.
    pub module_path: String,
    /// The exact Wasmtime release this run executed under (see [`super::WASMTIME_VERSION`]).
    pub wasmtime_version: String,
    /// sha256 hex of the deterministic-configuration descriptor (see [`super::config_digest`]).
    pub config_digest: String,
    pub args: Vec<String>,
    pub args_hash: String,
    pub env: BTreeMap<String, String>,
    pub env_hash: String,
    /// Raw stdin bytes, hex-encoded (never base64/UTF8 — stdin isn't guaranteed to be text).
    pub stdin_hex: String,
    pub stdin_hash: String,
    pub stdin_len: usize,
    /// The seed handed to both virtualized WASI random interfaces (`determinism::secure_rng` /
    /// `insecure_rng`).
    pub seed: u64,
    /// The fixed instant `wasi:clocks/wall-clock` reports throughout the run, ms since epoch.
    pub epoch_ms: u64,
    pub fuel_limit: u64,
    pub fuel_consumed: u64,
    /// sha256 hex of stdout bytes — never the raw bytes (verification re-derives them by
    /// re-running, so storing them twice in the bundle buys nothing and only grows it).
    pub stdout_hash: String,
    pub stdout_len: usize,
    pub stderr_hash: String,
    pub stderr_len: usize,
    /// `true` iff the guest's `wasi:cli/run` returned success (no trap, exit code 0).
    pub success: bool,
    /// A short, non-sensitive host-side error class when `success` is `false` (a trap category or
    /// exit code — never guest-produced content).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub recorded_ts_ms: u128,
}

impl RunBundle {
    /// sha256 hex of this bundle's own canonical JSON bytes — what the `kriya.exec.deterministic`
    /// receipt binds to (`params.bundle_hash`).
    pub fn bundle_hash(&self) -> String {
        sha256_hex(&self.canonical_bytes())
    }

    /// Deterministic serialization: `serde_json` on a `#[derive(Serialize)]` struct already emits
    /// fields in declaration order (not alphabetized, but FIXED — the same order every time for
    /// the same struct shape), which is all `bundle_hash` needs: two bundles with identical field
    /// values serialize identically.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// The additive, hash-only fields this lane's signed receipt carries — deliberately excludes
    /// `args`/`env`/`stdin_hex` (content) even though the bundle FILE stores them for re-runnability;
    /// the receipt only ever binds to the bundle's hash (C1 discipline: hashes/counts/names only).
    pub fn receipt_params(&self, tool_name: Option<&str>) -> serde_json::Value {
        let mut params = serde_json::json!({
            "bundle_hash": self.bundle_hash(),
            "module_sha256": self.module_sha256,
            "wasmtime_version": self.wasmtime_version,
            "config_digest": self.config_digest,
            "args_hash": self.args_hash,
            "env_hash": self.env_hash,
            "stdin_hash": self.stdin_hash,
            "fuel_consumed": self.fuel_consumed,
            "stdout_hash": self.stdout_hash,
            "stderr_hash": self.stderr_hash,
            "success": self.success,
        });
        if let Some(name) = tool_name {
            params["tool_name"] = serde_json::Value::String(name.to_string());
        }
        params
    }
}

pub fn hash_args(args: &[String]) -> String {
    let joined = serde_json::to_vec(args).unwrap_or_default();
    sha256_hex(&joined)
}

pub fn hash_env(env: &BTreeMap<String, String>) -> String {
    let joined = serde_json::to_vec(env).unwrap_or_default();
    sha256_hex(&joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RunBundle {
        let args = vec!["--upper".to_string()];
        let mut env = BTreeMap::new();
        env.insert("LANG".to_string(), "C".to_string());
        let stdin = b"hello".to_vec();
        RunBundle {
            schema: SCHEMA.to_string(),
            module_sha256: "a".repeat(64),
            module_path: "text-transform.wasm".to_string(),
            wasmtime_version: "47.0.2".to_string(),
            config_digest: "b".repeat(64),
            args_hash: hash_args(&args),
            args,
            env_hash: hash_env(&env),
            env,
            stdin_hash: sha256_hex(&stdin),
            stdin_len: stdin.len(),
            stdin_hex: hex::encode(&stdin),
            seed: 42,
            epoch_ms: 1_700_000_000_000,
            fuel_limit: 10_000_000,
            fuel_consumed: 12_345,
            stdout_hash: "c".repeat(64),
            stdout_len: 5,
            stderr_hash: sha256_hex(b""),
            stderr_len: 0,
            success: true,
            error: None,
            recorded_ts_ms: 1_700_000_000_001,
        }
    }

    #[test]
    fn bundle_hash_is_stable_for_identical_bundles() {
        let a = sample();
        let b = sample();
        assert_eq!(a.bundle_hash(), b.bundle_hash());
    }

    #[test]
    fn bundle_hash_changes_when_any_field_changes() {
        let a = sample();
        let mut b = sample();
        b.fuel_consumed += 1;
        assert_ne!(a.bundle_hash(), b.bundle_hash());
    }

    #[test]
    fn receipt_params_never_carries_raw_input_or_output_bytes() {
        let bundle = sample();
        let params = bundle.receipt_params(Some("text-transform"));
        let text = params.to_string();
        assert!(!text.contains("stdin_hex"), "receipt params must never carry raw stdin bytes");
        assert!(!text.contains("hello"), "receipt params must never leak stdin CONTENT");
        assert!(text.contains("args_hash"));
        assert!(text.contains("bundle_hash"));
        assert!(text.contains("tool_name"));
    }

    #[test]
    fn round_trips_through_json() {
        let bundle = sample();
        let json = serde_json::to_string(&bundle).unwrap();
        let back: RunBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(bundle, back);
    }
}
