//! Test-only cross-implementation ML-DSA-87 parity harness for the kriya PQ lane.
//!
//! This crate has no runtime API surface — it exists solely to host the two
//! cross-implementation parity integration tests in `tests/parity.rs` (A5, doc 27,
//! design D7). Keeping them here rather than as `crates/kriya` dev-dependencies is
//! what lets `crates/kriya` carry zero `[dev-dependencies]`, so `cargo test -p kriya`
//! keeps working from the `apps/note-app/src-tauri` workspace where `kriya` is a
//! non-member path dependency.
