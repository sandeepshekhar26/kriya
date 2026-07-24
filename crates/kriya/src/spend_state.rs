//! C2 (doc 27 §4 / `docs/design/c2-budget-gate.md` D1/D6) — a **read-only** loader for the
//! Console-written spend-state files. The runtime NEVER prices anything: every USD figure here is
//! copied verbatim out of a file the Console (`kriya-console`'s `spend/mod.rs`) already wrote,
//! reusing C1's bundled pricing sheet. This module has no pricing dependency at all (guarded by
//! `spend_state_module_pulls_in_no_pricing_dependency` below) — it is pure file-read + JSON parse.
//!
//! Two sources, layered (D1):
//! - **`<state-dir>/spend-live.json`** — the always-on primary. Written by the Console's settle-tick
//!   heartbeat: pre-seal running totals for every session (including in-flight ones) plus rolling-day
//!   and per-OS-user aggregates. Freshness ≈ one tick (default 5 min).
//! - **`<state-dir>/spend-turn/<session_id>.json`** — the optional accelerator. Written by the
//!   Console-provided `kriya-spend-statusline` shim on (almost) every Claude Code render. Freshness
//!   ≈ one assistant turn. Only ever consulted for a **session**-scope rule.
//!
//! For a session-scope rule, [`SpendState::resolve`] picks whichever of the two carries the larger
//! `as_of_ms` (D1: "the runtime reads whichever source is freshest per session, and records which one
//! it used") — never sums them, never re-derives a number.
//!
//! **State-dir seam (F-C1, no new flag).** The state directory is always `<audit-dir>/../state`,
//! derived from the SAME `--audit-log` flag the hook already parses — see
//! [`state_dir_for_audit_log`]. The Console's writer (`spend::spend_live_path`) mirrors the identical
//! derivation from its own `default_audit_dir()`, so reader and writer always agree without a new flag.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::permissions::BudgetScope;

/// `<audit-dir>/../state` (F-C1): `audit_log`'s PARENT directory is "the audit dir"
/// (`~/.kriya/audit/claude-code.jsonl` -> `~/.kriya/audit`); state sits beside it
/// (`~/.kriya/state`) — never inside it, so the state dir is never swept by an audit-log rotation.
pub fn state_dir_for_audit_log(audit_log: &Path) -> PathBuf {
    let audit_dir = audit_log.parent().unwrap_or_else(|| Path::new("."));
    audit_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("state")
}

/// Which state source produced a [`StateReading`] — carried onto the gate receipt verbatim (D1/D4)
/// so a reviewer can see exactly how the enforced number was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateSource {
    LiveTick,
    Statusline,
}

impl StateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            StateSource::LiveTick => "live-tick",
            StateSource::Statusline => "statusline",
        }
    }
}

/// One resolved, already-priced observation the gate can act on. Every field here is copied through
/// from the Console-written file — this crate never computes `observed_usd` itself.
#[derive(Debug, Clone, PartialEq)]
pub struct StateReading {
    pub observed_usd: f64,
    pub as_of_ms: u64,
    pub source: StateSource,
    pub pricing_sheet: Option<String>,
    pub pricing_sheet_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Amount {
    observed_usd: f64,
    as_of_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct LiveSnapshot {
    #[serde(default)]
    pricing_sheet: Option<String>,
    #[serde(default)]
    pricing_sheet_hash: Option<String>,
    #[serde(default)]
    sessions: HashMap<String, Amount>,
    #[serde(default)]
    rolling_day: Option<Amount>,
    #[serde(default)]
    user: Option<Amount>,
}

#[derive(Debug, Clone, Deserialize)]
struct TurnSnapshot {
    #[serde(default)]
    observed_usd: f64,
    #[serde(default)]
    as_of_ms: u64,
    #[serde(default)]
    pricing_sheet: Option<String>,
    #[serde(default)]
    pricing_sheet_hash: Option<String>,
}

/// The loaded (possibly-absent) spend state for one gate evaluation. Load once per `kriya-hook pre`
/// invocation (this process reads the file at most twice: the tick file, and — only for a
/// session-scope rule — that one session's turn file).
pub struct SpendState {
    live: Option<LiveSnapshot>,
    state_dir: PathBuf,
}

impl SpendState {
    /// Load `<state_dir>/spend-live.json`, if present and parseable. A missing/corrupt file is not
    /// an error here — it surfaces as `resolve(..) -> None`, which [`crate::permissions::BudgetPolicy`]
    /// treats as missing state (D2's fail-closed-by-default matrix), never a panic.
    pub fn load(state_dir: &Path) -> Self {
        let live = std::fs::read_to_string(state_dir.join("spend-live.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<LiveSnapshot>(&text).ok());
        SpendState { live, state_dir: state_dir.to_path_buf() }
    }

    fn load_turn(&self, session_id: &str) -> Option<TurnSnapshot> {
        let path = self
            .state_dir
            .join("spend-turn")
            .join(format!("{session_id}.json"));
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Resolve the freshest reading for `scope`. For [`BudgetScope::Session`], `session_id` is
    /// required — `None` (the payload carried no session id) is treated as an unresolvable scope key,
    /// i.e. missing state (Red-team b: "never silently skipped"). `RollingDay`/`User` only ever
    /// consult the tick file — the statusline accelerator is a SESSION-scope-only accelerator (D1).
    pub fn resolve(&self, scope: BudgetScope, session_id: Option<&str>) -> Option<StateReading> {
        match scope {
            BudgetScope::Session => {
                let sid = session_id?;
                let tick = self.live.as_ref().and_then(|l| l.sessions.get(sid)).map(|a| StateReading {
                    observed_usd: a.observed_usd,
                    as_of_ms: a.as_of_ms,
                    source: StateSource::LiveTick,
                    pricing_sheet: self.live.as_ref().and_then(|l| l.pricing_sheet.clone()),
                    pricing_sheet_hash: self.live.as_ref().and_then(|l| l.pricing_sheet_hash.clone()),
                });
                let turn = self.load_turn(sid).map(|t| StateReading {
                    observed_usd: t.observed_usd,
                    as_of_ms: t.as_of_ms,
                    source: StateSource::Statusline,
                    pricing_sheet: t.pricing_sheet,
                    pricing_sheet_hash: t.pricing_sheet_hash,
                });
                match (tick, turn) {
                    (Some(a), Some(b)) => Some(if b.as_of_ms > a.as_of_ms { b } else { a }),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                }
            }
            BudgetScope::RollingDay => self.live.as_ref().and_then(|l| l.rolling_day.as_ref()).map(|a| {
                StateReading {
                    observed_usd: a.observed_usd,
                    as_of_ms: a.as_of_ms,
                    source: StateSource::LiveTick,
                    pricing_sheet: self.live.as_ref().and_then(|l| l.pricing_sheet.clone()),
                    pricing_sheet_hash: self.live.as_ref().and_then(|l| l.pricing_sheet_hash.clone()),
                }
            }),
            BudgetScope::User => self.live.as_ref().and_then(|l| l.user.as_ref()).map(|a| StateReading {
                observed_usd: a.observed_usd,
                as_of_ms: a.as_of_ms,
                source: StateSource::LiveTick,
                pricing_sheet: self.live.as_ref().and_then(|l| l.pricing_sheet.clone()),
                pricing_sheet_hash: self.live.as_ref().and_then(|l| l.pricing_sheet_hash.clone()),
            }),
        }
    }

    /// Test/fixture-only constructor — builds a [`SpendState`] entirely in memory, no filesystem
    /// access, for the `permissions.rs` budget-check matrix (missing/stale/scope-key unit tests).
    #[cfg(test)]
    pub(crate) fn synthetic(
        pricing_sheet: Option<&str>,
        pricing_sheet_hash: Option<&str>,
        sessions: &[(&str, f64, u64)],
        rolling_day: Option<(f64, u64)>,
        user: Option<(f64, u64)>,
    ) -> Self {
        let mut sess = HashMap::new();
        for (sid, usd, ts) in sessions {
            sess.insert((*sid).to_string(), Amount { observed_usd: *usd, as_of_ms: *ts });
        }
        SpendState {
            live: Some(LiveSnapshot {
                pricing_sheet: pricing_sheet.map(|s| s.to_string()),
                pricing_sheet_hash: pricing_sheet_hash.map(|s| s.to_string()),
                sessions: sess,
                rolling_day: rolling_day.map(|(usd, ts)| Amount { observed_usd: usd, as_of_ms: ts }),
                user: user.map(|(usd, ts)| Amount { observed_usd: usd, as_of_ms: ts }),
            }),
            state_dir: std::env::temp_dir().join("kriya-spend-state-synthetic-unused"),
        }
    }

    /// Test-only: a [`SpendState`] with no tick file at all (every scope resolves to missing).
    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        SpendState { live: None, state_dir: std::env::temp_dir().join("kriya-spend-state-empty-unused") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_derives_as_the_audit_dirs_sibling() {
        let log = Path::new("/home/op/.kriya/audit/claude-code.jsonl");
        assert_eq!(state_dir_for_audit_log(log), PathBuf::from("/home/op/.kriya/state"));
    }

    #[test]
    fn state_dir_derivation_matches_a_nested_sandbox_audit_subdir() {
        // The smoke-test build note (c2-budget-gate-rereview.md F-C1): nesting the sandbox's audit
        // log under an `audit/` subdir makes `<audit-dir>/../state` land at a per-sandbox path.
        let log = Path::new("/tmp/sbx-1/audit/claude-code.jsonl");
        assert_eq!(state_dir_for_audit_log(log), PathBuf::from("/tmp/sbx-1/state"));
    }

    #[test]
    fn session_scope_prefers_the_fresher_of_tick_and_statusline() {
        let state = SpendState::synthetic(
            Some("sheet-1"),
            Some("hash-1"),
            &[("s1", 1.0, 1_000)],
            None,
            None,
        );
        let r = state.resolve(BudgetScope::Session, Some("s1")).expect("tick-only reading");
        assert_eq!(r.observed_usd, 1.0);
        assert_eq!(r.source, StateSource::LiveTick);
    }

    #[test]
    fn session_scope_with_no_session_id_is_unresolvable() {
        let state = SpendState::synthetic(None, None, &[("s1", 1.0, 1_000)], None, None);
        assert!(state.resolve(BudgetScope::Session, None).is_none());
    }

    #[test]
    fn rolling_day_and_user_never_consult_the_statusline_accelerator() {
        let state = SpendState::synthetic(None, None, &[], Some((5.0, 2_000)), Some((9.0, 3_000)));
        let day = state.resolve(BudgetScope::RollingDay, None).expect("day reading");
        assert_eq!(day.observed_usd, 5.0);
        let user = state.resolve(BudgetScope::User, None).expect("user reading");
        assert_eq!(user.observed_usd, 9.0);
    }

    #[test]
    fn empty_state_resolves_every_scope_to_missing() {
        let state = SpendState::empty();
        assert!(state.resolve(BudgetScope::Session, Some("s1")).is_none());
        assert!(state.resolve(BudgetScope::RollingDay, None).is_none());
        assert!(state.resolve(BudgetScope::User, None).is_none());
    }

    /// No-pricing-dependency guard (D6/§3): this module's own Cargo dependency footprint is nothing
    /// beyond `serde`/`serde_json`, already present crate-wide for every other policy type — proven
    /// structurally (this file imports nothing from `crate::spend` or any pricing module, which
    /// doesn't exist in this crate at all) rather than by inspecting `Cargo.lock`.
    #[test]
    fn spend_state_module_pulls_in_no_pricing_dependency() {
        // The mere fact that this crate has no `pricing`/`spend` module to import from is the guard:
        // grep for the module path to prove it structurally, not just by convention.
        assert!(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pricing.rs").exists().not(),
            "the runtime crate must never gain a pricing module — pricing is Console-only (D6)"
        );
    }
}

#[cfg(test)]
use std::ops::Not;
