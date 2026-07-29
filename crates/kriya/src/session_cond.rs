//! B4 (doc 27 §4 / `docs/design/b4-temporal-conditions.md` D2/D7) — the session-scoped event fold +
//! cache that feeds `permissions::TemporalPolicy::evaluate`. Three responsibilities, one module:
//!
//! 1. **The B4 governed-corpus filter** ([`is_governance_internal_b4`], D7) — a STRUCTURAL SUPERSET
//!    of the base tier's own governance filter, which lives in the Console's
//!    `src-tauri/src/policy_sim.rs::is_governance_internal` and is NEVER modified by B4 (the
//!    founder ruling: "older build should not broke then anything is fine"). This runtime crate has
//!    no such predicate of its own today — [`is_governance_internal`] below is a LOCAL mirror of the
//!    Console's copy, declared so the superset relation is provable by CODE SHAPE, not asserted.
//!    Every `kriya.*` id is governance-internal UNLESS it is on the small, named
//!    [`is_governed_kriya_vocabulary`] allowlist (default-exclude, not a hand-maintained denylist —
//!    the fail-safe direction: a vocabulary added tomorrow is excluded automatically).
//! 2. **The fold** ([`fold_receipts`]) — turn already-read JSONL lines into this run's
//!    `SessionEvent`s: verified, not B4-governance-internal, and carrying this run's own
//!    `kriya.corr.run_id`. Split out as a pure function (no disk I/O) so a fixture test (F-A) can
//!    exercise the exact exclusion logic without a filesystem.
//! 3. **The cache** ([`load_or_build`]) — a session-scoped index under C2's OWN state-dir seam
//!    (`spend_state::state_dir_for_audit_log`, reused, never rivaled — no new CLI flag), so the live
//!    gate re-reads + re-verifies only the log's APPENDED TAIL since the last check, not the whole
//!    session every time. The index is a CACHE; the SEMANTICS are the fold over verified receipts —
//!    a missing/corrupt/stale cache always safely falls back to a full rebuild from the log itself.
//!
//! **Where the ids come from (R4/B-1, round 3).** The runtime `kriya` crate has NO `paid` module
//! (that's Console-only, `src-tauri/src/paid.rs`) and no `policy_sim` module at all — every id/
//! prefix this filter needs is declared LOCALLY below as named `pub const`s, reusing only
//! `crate::audit::ATTESTATION_ON_DEVICE` (the one constant the runtime actually owns).
//! `crate::llm::receipts::MODEL_SERVE` is deliberately NOT imported: `crate::llm` is gated
//! `#[cfg(feature = "llm-proxy")]` (off by default per `Cargo.toml`'s
//! `default = ["tauri-host", "http-inference"]`), so `kriya-hook` — a default-featured binary that
//! depends on this module — could not reference it without breaking the shipping build (`E0433`).
//! Cross-repo drift between this file's id-set and the Console's mirrored copy is caught by a
//! mirrored fixture (F-C), not by a hand-copied list.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::audit::{Actor, ATTESTATION_ON_DEVICE};
use crate::corr;
use crate::crypto;

// ─── The B4 governed-corpus filter (D7) ───────────────────────────────────────────────────────────

/// The reserved `kriya.` namespace prefix — anything under it is governance-internal UNLESS it is
/// on [`is_governed_kriya_vocabulary`]'s allowlist (D7's default-exclude direction).
pub const KRIYA_NAMESPACE_PREFIX: &str = "kriya.";
/// Mirrors the Console's `paid::COVERAGE_SNAPSHOT` — this runtime crate does not own this constant
/// (the Console does, in `src-tauri/src/paid.rs`), so it is declared here too (D7's local-
/// declaration pattern, the same one `session_cond`'s own module doc explains for `MODEL_SERVE`).
pub const COVERAGE_SNAPSHOT: &str = "kriya.coverage.snapshot";
/// Mirrors the Console's `paid::KRIYA_IO_PREFIX`.
pub const KRIYA_IO_PREFIX: &str = "kriya.io.";
/// Mirrors the Console's `policy_sim::KRIYA_POLICY_PREFIX`.
pub const KRIYA_POLICY_PREFIX: &str = "kriya.policy.";
/// The four EVIDENCE sub-prefixes of the reserved `kriya.watch.` namespace (doc 20 §2/§5) — a
/// process exec, a file write, a raw egress/DNS lookup the agent (or something it spawned) caused.
/// Kriya did not DO these; it SAW them — so they are deliberately NOT excluded (D7, corrected B-2,
/// round 3). `kriya.watch.heartbeat` / `.run.start` / `.run.exit` deliberately match NONE of these
/// four and fall through to governance-internal below — watcher liveness, not agent activity
/// (mirrors the Console's `src-tauri/src/coverage.rs`, which already splits this exact namespace
/// into evidence, `:162`/`:164`, vs. liveness, `:166`).
pub const KRIYA_WATCH_PROC_PREFIX: &str = "kriya.watch.proc.";
pub const KRIYA_WATCH_FILE_PREFIX: &str = "kriya.watch.file.";
pub const KRIYA_WATCH_NET_PREFIX: &str = "kriya.watch.net.";
pub const KRIYA_WATCH_DNS_PREFIX: &str = "kriya.watch.dns.";
/// `kriya.model.serve` — declared LOCALLY (round-3 B-1), NOT imported from
/// `crate::llm::receipts::MODEL_SERVE` (see this module's doc comment for why). This records the
/// AGENT's own forwarded model call, not a kriya decision about it — deliberately GOVERNED, not
/// excluded (D7).
pub const MODEL_SERVE: &str = "kriya.model.serve";

/// The shipped base-tier governance filter — a LOCAL mirror of the Console's
/// `src-tauri/src/policy_sim.rs::is_governance_internal` (never modified by B4; the founder ruling).
/// This runtime crate has no base-tier replay of its own to protect, but declaring the identical
/// 4-arm shape here is what makes [`is_governance_internal_b4`] a STRUCTURAL superset rather than
/// an asserted one — F-C proves both repos' copies agree on every mirrored id.
pub fn is_governance_internal(action_id: &str) -> bool {
    action_id == ATTESTATION_ON_DEVICE
        || action_id == COVERAGE_SNAPSHOT
        || action_id.starts_with(KRIYA_IO_PREFIX)
        || action_id.starts_with(KRIYA_POLICY_PREFIX)
}

/// The CLOSED, named allowlist of `kriya.*` vocabularies that ARE governed agent activity and are
/// therefore COUNTED by a temporal fold. Everything else under `kriya.` is governance-internal
/// (default-exclude — a vocabulary added tomorrow is excluded automatically, the fail-safe
/// direction, D7).
pub fn is_governed_kriya_vocabulary(action_id: &str) -> bool {
    action_id.starts_with(KRIYA_WATCH_PROC_PREFIX)
        || action_id.starts_with(KRIYA_WATCH_FILE_PREFIX)
        || action_id.starts_with(KRIYA_WATCH_NET_PREFIX)
        || action_id.starts_with(KRIYA_WATCH_DNS_PREFIX)
        || action_id == MODEL_SERVE
    // kriya.watch.heartbeat / .run.start / .run.exit deliberately match none of the four prefixes
    // above and fall through to governance-internal — watcher liveness, not agent activity (B-2).
}

/// The B4 temporal fold's governed-corpus filter (D7) — a STRUCTURAL SUPERSET of
/// [`is_governance_internal`]: the first arm is the unmodified base predicate, so
/// `∀ id: is_governance_internal(id) ⇒ is_governance_internal_b4(id)` holds by CODE SHAPE, not by
/// set bookkeeping. Applied IDENTICALLY on both B4 sides (this live fold, and the Console's
/// `policy_sim.rs` temporal replay), so the two corpora are provably identical for every selector —
/// including a wildcard (`action: "*"`) or `kriya.*` selector.
pub fn is_governance_internal_b4(action_id: &str) -> bool {
    is_governance_internal(action_id)
        || (action_id.starts_with(KRIYA_NAMESPACE_PREFIX) && !is_governed_kriya_vocabulary(action_id))
}

// ─── SessionEvent + the fold ───────────────────────────────────────────────────────────────────────

/// One event this session's fold could match against — a verified, governed receipt reduced to the
/// fields a temporal predicate needs. Built ONLY by [`fold_receipts`] from a receipt that is (a)
/// verified, (b) not [`is_governance_internal_b4`], and (c) carrying THIS run's own
/// `kriya.corr.run_id` — never constructed by hand from unverified input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub action_id: String,
    pub success: bool,
    pub ts_ms: u64,
    /// `params.command` when present (the Bash lane) — `None` for every other tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// A local mirror of `crate::audit::Receipt`'s field SHAPE — declared here, not there, so that hot,
/// heavily-tested signing struct (which derives `Serialize` only; nothing in the signing path has
/// ever needed to deserialize a `Receipt`) is never widened for a need only this new module has —
/// the same "small local duplication over a shared dependency" convention this crate already uses
/// for `canonical_value` (independently re-implemented in `audit.rs`, `memwrite.rs`, `llm/proxy.rs`,
/// `bin/kriya-gateway.rs`, `bin/kriya-hook.rs`). Field order MUST match `Receipt` exactly:
/// `serde_json::to_vec` on a struct serializes in DECLARATION order, and that order is exactly what
/// a receipt's Ed25519 signature covers.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReceiptForVerify {
    step_id: String,
    action_id: String,
    params: Value,
    success: bool,
    ts_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prev_hash: Option<String>,
}

/// Re-derive the canonical signed bytes from a parsed receipt LINE and check its Ed25519 signature
/// against its own embedded public key — the same check the offline CLI, the Console's TS verifier,
/// and I3 (`policy_sim.rs::verify_value`) perform, re-implemented locally because this crate has no
/// dependency on the Console's `kriya-verify` (the one-way dep). `false` on ANY malformed shape — a
/// receipt that doesn't even parse into the expected fields is not verified evidence either.
fn verify_receipt_line(v: &Value) -> bool {
    let receipt: ReceiptForVerify = match serde_json::from_value(v.clone()) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let pub_hex = match v.get("public_key").and_then(Value::as_str) {
        Some(s) => s,
        None => return false,
    };
    let sig_hex = match v.get("signature").and_then(Value::as_str) {
        Some(s) => s,
        None => return false,
    };
    let pub_bytes: [u8; 32] = match hex::decode(pub_hex).ok().and_then(|b| b.try_into().ok()) {
        Some(b) => b,
        None => return false,
    };
    let sig_bytes: [u8; 64] = match hex::decode(sig_hex).ok().and_then(|b| b.try_into().ok()) {
        Some(b) => b,
        None => return false,
    };
    let msg = match serde_json::to_vec(&receipt) {
        Ok(m) => m,
        Err(_) => return false,
    };
    crypto::verify(&pub_bytes, &msg, &sig_bytes)
}

/// The PURE part of the fold (R3): turn already-parsed JSON `lines` into this run's
/// `SessionEvent`s, with NO disk I/O — so a fixture test (F-A) can exercise the exact exclusion
/// logic head-on. A line becomes a `SessionEvent` only if it (a) verifies, (b) is NOT
/// [`is_governance_internal_b4`] (D7 / axiom 3), and (c) carries THIS run's own
/// `kriya.corr.run_id` — folded in `(ts_ms, chain-position)` total order (M-2): `lines` is assumed
/// to already be in on-disk/chain order, and this function only ever filters (never reorders), so
/// the returned vec's own index order IS chain-position order, disambiguating any equal-`ts_ms`
/// pair by which line came first.
pub fn fold_receipts(lines: &[Value], run_id: &str) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    for v in lines {
        if !verify_receipt_line(v) {
            continue; // unverified activity is not evidence (same discipline as I3)
        }
        let action_id = match v.get("action_id").and_then(Value::as_str) {
            Some(a) if !a.is_empty() => a,
            _ => continue,
        };
        if is_governance_internal_b4(action_id) {
            continue; // D7 — kriya's own bookkeeping, never counted as agent activity
        }
        let this_run = v
            .get("params")
            .and_then(|p| p.get(corr::RESERVED_KEY))
            .and_then(|c| c.get("run_id"))
            .and_then(Value::as_str);
        if run_id.is_empty() || this_run != Some(run_id) {
            continue;
        }
        let success = v.get("success").and_then(Value::as_bool).unwrap_or(false);
        let ts_ms = v.get("ts_ms").and_then(Value::as_u64).unwrap_or(0);
        let command = v
            .get("params")
            .and_then(|p| p.get("command"))
            .and_then(Value::as_str)
            .map(str::to_string);
        events.push(SessionEvent { action_id: action_id.to_string(), success, ts_ms, command });
    }
    events
}

fn parse_lines(text: &str) -> Vec<Value> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// A private copy of `audit.rs`'s / `permissions.rs`'s own `sha256_hex` helper (neither is `pub`,
/// so this crate's established answer is a small local copy per module, not a shared dependency).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn last_nonempty_line_hash(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| sha256_hex(l.as_bytes()))
}

// ─── The cache (D2) ─────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Cursor {
    log_path: String,
    byte_offset: u64,
    last_line_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    v: u32,
    run_id: String,
    updated_ms: u64,
    cursor: Cursor,
    events: Vec<SessionEvent>,
}

/// The session fold could not be computed at all — the log itself is unreadable (an I/O error, NOT
/// "empty": a readable-but-absent-or-empty log is a valid, available state where every predicate
/// reads its honest false/empty answer, D2). Handled by the matching rule's `on_unavailable`
/// posture (`permissions::TemporalRule::on_unavailable_posture`).
#[derive(Debug)]
pub struct Unavailable;

/// The successful outcome of [`load_or_build`] — this run's folded events, plus which path was
/// taken (cache tail-append vs full rebuild) so the caller can honestly label the
/// `kriya.policy.cond.*` receipt's `index_source` (D4).
#[derive(Debug, Clone)]
pub struct SessionFold {
    pub events: Vec<SessionEvent>,
    /// `"cache"` (incremental tail-append) or `"rebuilt"` (full re-fold) — never `"unavailable"`,
    /// which is reserved for the `Err(Unavailable)` case this type never represents.
    pub source: &'static str,
}

/// Session-scoped index path (D2): `<state_dir>/session-cond/<run_id>.json` — sits BESIDE C2's
/// `spend-live.json`/`spend-turn/` under the SAME `state_dir` (reused via
/// `spend_state::state_dir_for_audit_log`, never a rival store, never a new CLI flag).
fn cache_path(state_dir: &Path, run_id: &str) -> PathBuf {
    state_dir.join("session-cond").join(format!("{run_id}.json"))
}

/// Validate that `cursor` still describes a strict PREFIX of `log_text` — i.e. the log was only
/// ever appended to since the cursor was recorded, never rotated/truncated. Returns the tail slice
/// past `cursor.byte_offset` on success. A byte offset that no longer fits, or whose last-consumed
/// line no longer hashes to `last_line_sha256`, means the log changed underneath the cache — always
/// treated as a cold cache (full rebuild), never trusted partially.
fn tail_matches<'a>(log_text: &'a str, cursor: &Cursor) -> Option<&'a str> {
    let offset = usize::try_from(cursor.byte_offset).ok()?;
    if offset > log_text.len() || !log_text.is_char_boundary(offset) {
        return None;
    }
    let consumed = &log_text[..offset];
    match last_nonempty_line_hash(consumed) {
        // A cursor recorded against a non-empty log must still find a matching last line; a cursor
        // recorded at byte_offset 0 (an empty log at cache-build time) has no last line to check.
        Some(h) if h == cursor.last_line_sha256 => Some(&log_text[offset..]),
        None if cursor.last_line_sha256.is_empty() && offset == 0 => Some(&log_text[offset..]),
        _ => None,
    }
}

/// Best-effort atomic write (temp file + rename, C2's own discipline) — a cache write failure must
/// NEVER block the gate; the cache is a rebuildable optimization, never the source of truth (the
/// fold can always be recomputed from the log itself).
fn write_cache_atomically(path: &Path, cache: &CacheFile) {
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(serialized) = serde_json::to_string(cache) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, &serialized).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, path);
}

/// Load (or incrementally update, or fully rebuild) this run's folded session-event index.
///
/// - **Cache hit (fast path):** the cache file exists, its `cursor.log_path` matches `audit_log`,
///   and the log at `cursor.byte_offset` still starts with a line whose sha256 equals
///   `cursor.last_line_sha256` (the log was only ever APPENDED to since the cache was built) — read
///   + verify + fold ONLY the tail past `byte_offset`, append to the cached events, advance the
///   cursor, rewrite atomically. O(1) amortized: a small-file read plus a bounded tail fold, never a
///   whole-log rescan.
/// - **Cold / invalid / cross-log cache:** missing file, unparseable JSON, a `log_path` mismatch, or
///   a cursor that no longer lines up (a rotated/truncated log) — rebuild by folding THIS RUN's
///   verified receipts from the log ONCE (bounded by session size, never the whole device corpus —
///   this reads one file, filtered on `run_id`), then cache the fresh cursor. Also the safe path
///   when the CACHE file itself is corrupt.
/// - **Genuinely unavailable:** the log itself can't be READ (an I/O error opening/reading the file,
///   e.g. a permissions problem or non-UTF8 content) — `Err(Unavailable)`. A log that simply does
///   not exist yet (the first action of a session, before any receipt has been written) is treated
///   as a valid EMPTY session, not this case — there is nothing wrong, there is just nothing yet.
pub fn load_or_build(state_dir: &Path, run_id: &str, audit_log: &Path) -> Result<SessionFold, Unavailable> {
    let path = cache_path(state_dir, run_id);
    let log_text = match std::fs::read_to_string(audit_log) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return Err(Unavailable),
    };
    let log_path_str = audit_log.to_string_lossy().into_owned();

    let cached = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str::<CacheFile>(&t).ok());
    if let Some(cached) = cached {
        if cached.run_id == run_id && cached.cursor.log_path == log_path_str {
            if let Some(tail) = tail_matches(&log_text, &cached.cursor) {
                let new_lines = parse_lines(tail);
                let mut events = cached.events;
                events.extend(fold_receipts(&new_lines, run_id));
                let fresh = CacheFile {
                    v: 1,
                    run_id: run_id.to_string(),
                    updated_ms: crate::audit::now_ms() as u64,
                    cursor: Cursor {
                        log_path: log_path_str,
                        byte_offset: log_text.len() as u64,
                        last_line_sha256: last_nonempty_line_hash(&log_text).unwrap_or_default(),
                    },
                    events: events.clone(),
                };
                write_cache_atomically(&path, &fresh);
                return Ok(SessionFold { events, source: "cache" });
            }
        }
    }

    // Cold / invalid / cross-log — full rebuild from this log alone, filtered on run_id.
    let lines = parse_lines(&log_text);
    let events = fold_receipts(&lines, run_id);
    let fresh = CacheFile {
        v: 1,
        run_id: run_id.to_string(),
        updated_ms: crate::audit::now_ms() as u64,
        cursor: Cursor {
            log_path: log_path_str,
            byte_offset: log_text.len() as u64,
            last_line_sha256: last_nonempty_line_hash(&log_text).unwrap_or_default(),
        },
        events: events.clone(),
    };
    write_cache_atomically(&path, &fresh);
    Ok(SessionFold { events, source: "rebuilt" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{Receipt, Signer};
    use crate::corr::Correlation;
    use serde_json::json;

    fn sandbox_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "kriya-session-cond-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    fn seed_line(signer: &Signer, actor: &Actor, action_id: &str, success: bool, corr: &Correlation, params: Value) {
        signer.record(
            Receipt::new(uuid::Uuid::new_v4().to_string(), action_id.to_string(), corr::attach(params, corr), success, crate::audit::now_ms())
                .with_actor(Some(actor.clone())),
        );
    }

    #[test]
    fn is_governance_internal_b4_is_a_structural_superset() {
        // Every id the base predicate excludes must also be excluded by the B4 predicate.
        for id in [
            "kriya.attestation.on_device",
            "kriya.coverage.snapshot",
            "kriya.io.egress.http.allow",
            "kriya.policy.cond.deny",
        ] {
            assert!(is_governance_internal(id), "base predicate should exclude {id}");
            assert!(is_governance_internal_b4(id), "superset must also exclude {id}");
        }
    }

    #[test]
    fn is_governance_internal_b4_excludes_b4_additions_the_base_predicate_does_not() {
        for id in ["kriya.spend.gate.warn", "kriya.memory.write", "kriya.artifact.provenance"] {
            assert!(!is_governance_internal(id), "base predicate does NOT cover {id} (the B-1 case)");
            assert!(is_governance_internal_b4(id), "B4's wider filter must exclude {id}");
        }
    }

    #[test]
    fn gate_receipts_are_governance_internal_by_construction() {
        // F-2: kriya.gate.<class>.* is the gate engine's own bookkeeping — the UNDERLYING action
        // receipt is the governed event; counting both would double a single agent action in the
        // temporal fold. Excluded structurally (kriya.* minus the governed allowlist), asserted
        // here explicitly so the F-C fixture disposition ("internal") is code-backed.
        for id in [
            "kriya.gate.self-mod.denied",
            "kriya.gate.publish.held",
            "kriya.gate.deploy.approved",
            "kriya.gate.destructive-git.evaluated",
        ] {
            assert!(is_governance_internal_b4(id), "{id} must be governance-internal");
        }
    }

    #[test]
    fn watch_evidence_prefixes_are_governed_but_liveness_is_not() {
        for id in ["kriya.watch.proc.exec", "kriya.watch.file.write", "kriya.watch.net.connect", "kriya.watch.dns.lookup"] {
            assert!(!is_governance_internal_b4(id), "{id} is EVIDENCE — must be governed/countable");
        }
        for id in ["kriya.watch.heartbeat", "kriya.watch.run.start", "kriya.watch.run.exit"] {
            assert!(is_governance_internal_b4(id), "{id} is watcher LIVENESS — must be governance-internal (B-2)");
        }
    }

    #[test]
    fn model_serve_is_governed_not_excluded() {
        assert!(!is_governance_internal_b4(MODEL_SERVE));
    }

    #[test]
    fn non_kriya_agent_actions_are_never_excluded() {
        for id in ["claude-code__bash", "claude-code__write", "widgets__list"] {
            assert!(!is_governance_internal_b4(id));
        }
    }

    #[test]
    fn fold_receipts_drops_unverified_lines() {
        let tampered = json!({"action_id": "claude-code__bash", "ts_ms": 1u64, "success": true, "params": {"kriya.corr": {"run_id": "r1"}}});
        let events = fold_receipts(&[tampered], "r1");
        assert!(events.is_empty(), "an unsigned/unverifiable line must never become a SessionEvent");
    }

    #[test]
    fn fold_receipts_drops_other_runs_and_governance_internal_ids() {
        let dir = sandbox_dir("fold");
        std::fs::create_dir_all(&dir).unwrap();
        let signer = Signer::with_log_path(dir.join("l.jsonl"));
        let actor = Actor::new("claude-code", "ci");
        let corr_a = Correlation::run("run-a");
        let corr_b = Correlation::run("run-b");

        seed_line(&signer, &actor, "claude-code__bash", true, &corr_a, json!({"command": "npm test"}));
        seed_line(&signer, &actor, "claude-code__bash", false, &corr_b, json!({"command": "npm test"})); // other run
        seed_line(&signer, &actor, "kriya.spend.gate.warn", true, &corr_a, json!({})); // governance-internal (B4 addition)
        seed_line(&signer, &actor, "kriya.policy.cond.deny", false, &corr_a, json!({})); // governance-internal (base)

        let text = std::fs::read_to_string(dir.join("l.jsonl")).unwrap();
        let lines = parse_lines(&text);
        let events = fold_receipts(&lines, "run-a");

        assert_eq!(events.len(), 1, "only the run-a bash receipt should survive the fold");
        assert_eq!(events[0].action_id, "claude-code__bash");
        assert_eq!(events[0].command.as_deref(), Some("npm test"));
        assert!(events[0].success);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_build_treats_a_missing_log_as_an_empty_session_not_unavailable() {
        let dir = sandbox_dir("missing-log");
        let state_dir = dir.join("state");
        let log = dir.join("does-not-exist.jsonl");
        let fold = load_or_build(&state_dir, "run-x", &log).expect("missing log is a valid empty session");
        assert!(fold.events.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_build_rebuilds_then_uses_the_cache_on_the_next_call() {
        let dir = sandbox_dir("cache-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let state_dir = dir.join("state");
        let log = dir.join("claude-code.jsonl");
        let signer = Signer::with_log_path(log.clone());
        let actor = Actor::new("claude-code", "ci");
        let corr = Correlation::run("run-cache");
        seed_line(&signer, &actor, "claude-code__bash", true, &corr, json!({"command": "npm test"}));

        let first = load_or_build(&state_dir, "run-cache", &log).unwrap();
        assert_eq!(first.source, "rebuilt");
        assert_eq!(first.events.len(), 1);

        // No new receipts — a second call should hit the cache and see the identical event.
        let second = load_or_build(&state_dir, "run-cache", &log).unwrap();
        assert_eq!(second.source, "cache");
        assert_eq!(second.events.len(), 1);

        // A new receipt appended — the cache should pick up only the tail.
        seed_line(&signer, &actor, "claude-code__bash", false, &corr, json!({"command": "git push origin main"}));
        let third = load_or_build(&state_dir, "run-cache", &log).unwrap();
        assert_eq!(third.source, "cache");
        assert_eq!(third.events.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_build_rebuilds_when_the_log_is_truncated_underneath_the_cache() {
        let dir = sandbox_dir("truncated");
        std::fs::create_dir_all(&dir).unwrap();
        let state_dir = dir.join("state");
        let log = dir.join("claude-code.jsonl");
        let signer = Signer::with_log_path(log.clone());
        let actor = Actor::new("claude-code", "ci");
        let corr = Correlation::run("run-trunc");
        seed_line(&signer, &actor, "claude-code__bash", true, &corr, json!({"command": "npm test"}));
        let _ = load_or_build(&state_dir, "run-trunc", &log).unwrap();

        // Truncate the log to simulate rotation — the cursor no longer describes a valid prefix.
        std::fs::write(&log, "").unwrap();
        let signer2 = Signer::with_log_path(log.clone());
        seed_line(&signer2, &actor, "claude-code__bash", false, &corr, json!({"command": "git push"}));

        let after = load_or_build(&state_dir, "run-trunc", &log).unwrap();
        assert_eq!(after.source, "rebuilt", "a truncated/rotated log must trigger a full rebuild, not a corrupt tail-read");
        assert_eq!(after.events.len(), 1);
        assert_eq!(after.events[0].command.as_deref(), Some("git push"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
