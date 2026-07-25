//! The virtualized clocks + seeded RNGs that make a [`super::RunBundle`] re-runnable without any
//! ambient host state leaking in (module doc's "no ambient clocks/random" knob).
//!
//! `wasmtime-wasi` 41.0.4 (pinned — see the `wasm-exec` feature comment in `Cargo.toml`) re-exports
//! `rand::Rng` and requires `impl Rng + Send + 'static` at
//! [`WasiCtxBuilder::secure_random`]/[`insecure_random`]. It depends on the SAME `rand` 0.8 line
//! this crate already uses elsewhere (verified via `Cargo.lock` — no coincidence to rely on
//! silently: the pinned exact versions in `Cargo.toml` keep it that way), so the seeded RNGs below
//! are built directly from this crate's existing `rand` dependency — no second `rand` version.
//!
//! [`WasiCtxBuilder::secure_random`]: wasmtime_wasi::WasiCtxBuilder::secure_random
//! [`insecure_random`]: wasmtime_wasi::WasiCtxBuilder::insecure_random

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use wasmtime_wasi::{HostMonotonicClock, HostWallClock};

use rand::SeedableRng;

/// Domain-separation constant XORed into the seed for the `insecure` RNG stream, so the
/// `secure`/`insecure` WASI random interfaces don't (accidentally) produce the identical byte
/// stream from the same recorded seed — a cosmetic hygiene choice, not a security property (both
/// streams are fully seed-derived and reproducible either way, which is the entire point here).
const INSECURE_DOMAIN_SEP: u64 = 0x9E37_79B9_7F4A_7C15;

/// `wasi:clocks/wall-clock`, pinned to the bundle's recorded `epoch_ms` — every call returns the
/// SAME instant, so "what time is it" can never diverge between the record run and a `--verify`
/// re-run, or between two verify re-runs.
pub struct FixedWallClock {
    pub epoch_ms: u64,
}

impl HostWallClock for FixedWallClock {
    fn resolution(&self) -> Duration {
        Duration::from_nanos(1)
    }
    fn now(&self) -> Duration {
        Duration::from_millis(self.epoch_ms)
    }
}

/// `wasi:clocks/monotonic-clock`, deterministic but still monotonically increasing (some guest
/// code busy-loops on "has time advanced" — a clock permanently frozen at 0 could hang it). Each
/// call advances an internal counter by a FIXED step, so the exact sequence of values returned is
/// a pure function of how many times the guest asked — reproducible across runs of the identical
/// bundle, never sourced from the host's real monotonic clock.
pub struct SteppedMonotonicClock {
    ticks: AtomicU64,
}

impl SteppedMonotonicClock {
    /// Fixed nanoseconds advanced per call to `now()`.
    const STEP_NS: u64 = 1_000;

    pub fn new() -> Self {
        Self {
            ticks: AtomicU64::new(0),
        }
    }
}

impl Default for SteppedMonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl HostMonotonicClock for SteppedMonotonicClock {
    fn resolution(&self) -> u64 {
        1
    }
    fn now(&self) -> u64 {
        self.ticks.fetch_add(Self::STEP_NS, Ordering::SeqCst) + Self::STEP_NS
    }
}

/// A seeded, reproducible RNG for `wasi:random/random` (the guest's "secure" interface — virtualized
/// here on purpose: this lane's whole point is that randomness is a recorded input, not real entropy).
pub fn secure_rng(seed: u64) -> rand::rngs::StdRng {
    rand::rngs::StdRng::seed_from_u64(seed)
}

/// A seeded, reproducible RNG for `wasi:random/insecure`, domain-separated from [`secure_rng`].
pub fn insecure_rng(seed: u64) -> rand::rngs::StdRng {
    rand::rngs::StdRng::seed_from_u64(seed ^ INSECURE_DOMAIN_SEP)
}

/// Lowercase-hex SHA-256 — the one hashing primitive every bundle field and the config digest use.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_clock_is_deterministic_and_non_decreasing() {
        let c1 = SteppedMonotonicClock::new();
        let seq1: Vec<u64> = (0..5).map(|_| c1.now()).collect();
        let c2 = SteppedMonotonicClock::new();
        let seq2: Vec<u64> = (0..5).map(|_| c2.now()).collect();
        assert_eq!(seq1, seq2, "two fresh clocks must produce the identical call sequence");
        for w in seq1.windows(2) {
            assert!(w[1] > w[0], "monotonic clock must strictly increase");
        }
    }

    #[test]
    fn wall_clock_never_changes() {
        let c = FixedWallClock { epoch_ms: 1_700_000_000_000 };
        assert_eq!(c.now(), c.now());
        assert_eq!(c.now(), Duration::from_millis(1_700_000_000_000));
    }

    #[test]
    fn rngs_are_seed_deterministic_and_stream_separated() {
        use rand::RngCore;
        let mut a1 = secure_rng(7);
        let mut a2 = secure_rng(7);
        assert_eq!(a1.next_u64(), a2.next_u64(), "same seed -> same secure stream");

        let mut s = secure_rng(7);
        let mut i = insecure_rng(7);
        assert_ne!(
            s.next_u64(),
            i.next_u64(),
            "secure/insecure streams must be domain-separated even from the same seed"
        );
    }
}
