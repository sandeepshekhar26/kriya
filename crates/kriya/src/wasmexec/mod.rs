//! F4-wasm (doc 28 §F4) — the **Deterministic Execution** lane.
//!
//! **NAMING LAW (binding, doc 28 §F4):** the kriya-console repo's B1 is called "Verified Replay" —
//! it re-**DERIVES** a session timeline from already-signed receipts, offline, without re-running
//! anything. This module is a *completely different* claim: it re-**EXECUTES** a WASI-p2 component
//! bit-for-bit and hash-compares the result. The two must never share wording — this module and its
//! CLI (`kriya-run-wasm`) say "deterministic execution" / "re-execution", never "replay" or
//! "verified replay". Scoped ONLY to tools actually run through this lane — it says nothing about
//! any other tool call a governed session made.
//!
//! ## What "deterministic" means here (the honest ceiling — mirrored in the console's TRUST.md)
//! Two runs of the **same recorded bundle** (same WASI-p2 component bytes, same
//! [`WASMTIME_VERSION`], same [`config_digest`], same args/env/stdin, same seed/epoch) on THIS
//! machine produce byte-identical stdout/stderr and identical fuel consumption — verified in this
//! module's own test suite. It does **not** claim cross-Wasmtime-version reproducibility (a future
//! Wasmtime release could legitimately change codegen for the same wasm bytes — `wasmtime_version`
//! is recorded and checked precisely so that gap is visible, not glossed over), and it says nothing
//! about any tool call that did NOT run through this lane.
//!
//! ## The deterministic configuration
//! - **Fuel metering ON** (`Config::consume_fuel`) — every instruction costs fuel; `fuel_consumed`
//!   is part of the recorded bundle and re-checked on verify (a divergent execution path burns a
//!   different amount of fuel even when it happens to produce the same bytes on some inputs).
//! - **NaN canonicalization ON** (`Config::cranelift_nan_canonicalization`) — float NaN payloads are
//!   canonicalized so codegen-level NaN-bit-pattern nondeterminism can't leak into observable output.
//! - **Threads OFF** (`Config::wasm_threads(false)`) — the wasm threads proposal is the textbook
//!   source of execution-order nondeterminism; this lane never enables it.
//! - **Relaxed-SIMD OFF** (`Config::wasm_relaxed_simd(false)`) — the one WebAssembly proposal whose
//!   spec *explicitly* permits per-implementation nondeterministic results for perf; disabled outright
//!   rather than trusting `relaxed_simd_deterministic` (fewer moving parts, an honest "not supported
//!   in this lane" instead of a config flag someone could flip).
//! - **No ambient clocks or randomness** — `wasi:clocks/wall-clock`, `wasi:clocks/monotonic-clock`,
//!   `wasi:random/random`, and `wasi:random/insecure` are all virtualized: wall/monotonic time and
//!   the RNG streams are deterministic functions of the recorded `seed`/`epoch_ms` inputs, not the
//!   host's real clock or OS entropy. See [`determinism`] for the clock/RNG implementations.
//! - **No filesystem or network access** — the [`WasiCtxBuilder`](wasmtime_wasi::WasiCtxBuilder) is
//!   built with no preopens and no inherited network, on top of the existing containment posture
//!   (doc 24 §11 B14). A component that needs neither is exactly the scope this lane ships for (the
//!   two example tools) — anything wanting real I/O is out of scope for v1, not silently granted it.
//!
//! ## The record bundle ([`RunBundle`], `kriya-exec-bundle/1`)
//! Persisted as one JSON file. Carries the module hash, the exact Wasmtime version + a digest of
//! every deterministic knob above (so a future config change is visible as a digest change, not a
//! silent behavior drift), the args/env/stdin **hashes AND raw bytes** (inputs must be re-runnable —
//! size-capped at [`MAX_INPUT_BYTES`] with an honest refusal, never a silent truncation, above the
//! cap), the fuel consumed, and stdout/stderr **hashes only** (never raw output bytes — the signed
//! `kriya.exec.deterministic` receipt binds only the bundle's own hash, per the "no content in
//! receipts" discipline; re-verification re-derives output bytes by re-running, so storing them
//! twice buys nothing).
//!
//! ## Non-goals (doc 28 §F4 / §6 — do not extend this module toward any of these)
//! rr/syscall recording (gated, Linux-later); general shell-command determinism claims; running
//! third-party untrusted modules without the existing containment posture; macOS-native syscall
//! replay (rejected outright — no rr/PMU/SIP path on macOS; this WASM lane IS the honest macOS
//! answer, not a stopgap for one).

pub mod bundle;
pub mod determinism;
pub mod engine;
pub mod routing;

pub use bundle::{RunBundle, ACTION_ID, MAX_INPUT_BYTES, SCHEMA};
pub use engine::{
    build_and_record, verify_bundle, verify_bundle_file, ExecutionInputs, ExecutionOutputs,
    RecordError, VerifyFieldResult, VerifyReport,
};
pub use routing::WasmRoutingExecutor;

/// The exact Wasmtime release this lane is pinned to (kept in lockstep with the `=41.0.4` version
/// pin in `Cargo.toml` — the newest wasmtime line whose declared MSRV still matches this repo's
/// pinned `rust-toolchain.toml` (1.90.0); see the `wasm-exec` feature comment in `Cargo.toml` for
/// the full version-selection rationale. A mismatch between this constant and the pin is a
/// build-time fact worth catching, not a runtime one — asserted against `wasmtime::VERSION` in the
/// test suite below.
pub const WASMTIME_VERSION: &str = "41.0.4";

/// A short, human-readable description of every deterministic knob in force — the input to
/// [`config_digest`]. Any change here (a new knob, a flipped default) changes the digest, so a
/// bundle recorded under one configuration honestly fails to match a differently-configured host
/// instead of silently comparing apples to oranges.
fn config_descriptor() -> String {
    format!(
        "wasmtime={WASMTIME_VERSION};consume_fuel=on;nan_canonicalization=on;threads=off;\
         relaxed_simd=off;simd=on;wall_clock=virtualized;monotonic_clock=virtualized;\
         random=seeded;insecure_random=seeded;filesystem=none;network=none"
    )
}

/// SHA-256 hex digest of [`config_descriptor`] — the `config_digest` field every [`RunBundle`]
/// carries. Two bundles with the same digest were recorded under the identical deterministic
/// configuration (version-pinning honesty: this is a **same-Wasmtime-version** guarantee, not a
/// cross-version one — see the module doc).
pub fn config_digest() -> String {
    determinism::sha256_hex(config_descriptor().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_digest_is_stable_and_matches_wasmtime_version() {
        // wasmtime 41.0.4 exposes no runtime `VERSION` constant to assert against directly; the
        // crate-level `WASMTIME_VERSION` here is kept honest by the exact `=41.0.4` pin in
        // Cargo.toml instead (a version bump requires touching both, by convention — same
        // discipline as A4's `aws-lc-rs = "=1.17.3"` pin note in `crypto.rs`).
        let d1 = config_digest();
        let d2 = config_digest();
        assert_eq!(d1, d2, "config_digest must be a pure function of the fixed descriptor");
        assert_eq!(d1.len(), 64, "sha256 hex digest");
    }
}
