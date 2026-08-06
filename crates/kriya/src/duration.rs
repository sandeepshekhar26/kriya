//! O-3 (kriya-console doc 32 §4 O-3 / doc 33 §5.5) — the per-action **duration lane**.
//!
//! The hook is a fresh process per call (verified — see `kriya-hook.rs`'s module doc). To measure
//! how long a governed tool actually ran, the *pre* invocation must persist its start instant for
//! the *post* invocation to read. Contrast the pay chain (`pay.rs`), which correlates by *identity*
//! alone and needs no cross-process state: `pay.rs`'s own doc calls this out — "O-3's marker … the
//! duration lane needs because it correlates *timestamps*, not identity." So this lane DOES write a
//! marker file, but keys it by that same **identity** derivation (session + tool + canonical
//! `tool_input`) — the honest surrogate for a per-call id the Claude Code payload does not carry
//! (verified 2026-08-06: no `tool_use_id` field anywhere in the payload we consume).
//!
//! Everything here is honest and best-effort:
//!
//! - **Marker failures NEVER block the gate.** Writing/reading/sweeping the marker is a pure
//!   optimization for *evidence richness*, never a precondition for a decision — the same law the
//!   session-cond cache lives under ("the cache is never the source of truth"). Every I/O helper
//!   swallows its errors; the caller proceeds regardless.
//! - **Absent ⇒ absent.** A missing marker (pre never ran, a key mismatch, a swept-stale entry)
//!   yields NO duration params — an honest gap, never an estimated or zero-filled number.
//! - **A negative interval is a gap, not a zero.** If the recovered start is somehow *after* the
//!   end (a collided identity key from two identical in-flight calls, a clock step), the duration is
//!   treated as absent rather than clamped to 0 (a fabricated "0 ms" would be dishonest evidence).
//! - **Additive, optional params on the EXISTING action receipt.** No new `action_id`, no envelope
//!   change — a verifier with no duration awareness accepts the receipt byte-for-byte as before.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The two additive param KEYS a measured action receipt carries (kept `kriya.dur.*`-namespaced,
/// mirroring the other governance param families). These are receipt *params*, not new action ids —
/// so, unlike `kriya.pay.*`/`kriya.attest.shift.*`, they need no OT-1 vocabulary registration; they
/// affect the exported span's *timing*, not its attribute set.
pub const DUR_MS_KEY: &str = "kriya.dur.ms";
pub const DUR_BASIS_KEY: &str = "kriya.dur.basis";

/// `basis` values — HOW the interval was measured, recorded so a consumer never has to guess.
/// `hook-pre-post` = the two separate hook processes bracketing the real tool execution.
/// `in-process` = a single in-process timer (the gateway / llm-proxy lanes).
pub const BASIS_HOOK_PRE_POST: &str = "hook-pre-post";
pub const BASIS_IN_PROCESS: &str = "in-process";

/// A stale marker (its correlated *post* never arrived — e.g. the session ended, or a key collision
/// orphaned it) is swept after this age so `<state_dir>/durations/` can never grow without bound.
pub const MAX_MARKER_AGE_MS: u64 = 24 * 60 * 60 * 1000;

/// Schema-version marker every persisted start marker carries.
const MARKER_SCHEMA_V: u64 = 1;

/// A safety bound on the opportunistic sweep — never walk an unbounded directory in the hot path.
const SWEEP_SCAN_CAP: usize = 4096;

/// The persisted start marker: just the schema version + the pre-hook's wall-clock start. The
/// correlation key is the FILENAME (see [`marker_path`]), so it is deliberately not duplicated here.
#[derive(Debug, Serialize, Deserialize)]
struct StartMarker {
    v: u64,
    t_start_ms: u64,
}

/// The identity correlation key, shared by the *pre* and *post* invocations because it hashes only
/// fields present and identical in both: the session id, the tool name, and the canonical
/// `tool_input`. Returns the full 32-byte SHA-256 as hex (a filesystem-safe, collision-resistant
/// marker filename). `None`/`Some("")` session ids hash identically (mirrors [`crate::pay::pay_id_for`]).
pub fn call_key(session_id: Option<&str>, tool_name: &str, canonical_input: &str) -> String {
    let mut h = Sha256::new();
    h.update(session_id.unwrap_or("").as_bytes());
    h.update([0u8]);
    h.update(tool_name.as_bytes());
    h.update([0u8]);
    h.update(canonical_input.as_bytes());
    hex::encode(h.finalize())
}

/// `<state_dir>/durations/<call_key>.json` — sits beside C2's `spend-live.json` and the session-cond
/// index under the SAME state dir (`spend_state::state_dir_for_audit_log`, reused — never a rival
/// store, never a new CLI flag).
pub fn marker_path(state_dir: &Path, key: &str) -> PathBuf {
    state_dir.join("durations").join(format!("{key}.json"))
}

/// Best-effort atomic write of the pre-hook start marker (temp file + rename — session_cond's own
/// discipline). Any failure is swallowed: a marker we could not persist simply means the post hook
/// records an honest *absent* duration, never a blocked call.
pub fn write_start(state_dir: &Path, key: &str, t_start_ms: u64) {
    let path = marker_path(state_dir, key);
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let marker = StartMarker { v: MARKER_SCHEMA_V, t_start_ms };
    let Ok(serialized) = serde_json::to_string(&marker) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serialized.as_bytes()).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &path);
}

/// Read + DELETE the start marker for `key`, returning its `t_start_ms`. The delete makes the marker
/// single-use (the post hook consumes it exactly once); a subsequent post for the same key finds
/// nothing and records an honest absent duration. `None` on any miss/parse failure.
pub fn take_end(state_dir: &Path, key: &str) -> Option<u64> {
    let path = marker_path(state_dir, key);
    let text = std::fs::read_to_string(&path).ok()?;
    // Delete regardless of whether parsing below succeeds — a corrupt marker must not linger.
    let _ = std::fs::remove_file(&path);
    let marker: StartMarker = serde_json::from_str(&text).ok()?;
    Some(marker.t_start_ms)
}

/// The measured interval, or `None` when it cannot be trusted. `t_end < t_start` (a collided key or a
/// clock step) is a gap, never a clamped 0 — see the module doc's honesty note.
pub fn compute_dur_ms(t_start_ms: u64, t_end_ms: u64) -> Option<u64> {
    t_end_ms.checked_sub(t_start_ms)
}

/// Stamp the two additive params onto an action receipt's `params` object in place. A no-op when
/// `params` is not a JSON object (defensive — every real action receipt's params is an object).
pub fn annotate_duration(params: &mut Value, dur_ms: u64, basis: &str) {
    if let Value::Object(map) = params {
        map.insert(DUR_MS_KEY.to_string(), Value::from(dur_ms));
        map.insert(DUR_BASIS_KEY.to_string(), Value::from(basis));
    }
}

/// Opportunistic sweep of `<state_dir>/durations/` — remove any marker older than `max_age_ms`
/// (its correlated post never arrived). Best-effort and bounded ([`SWEEP_SCAN_CAP`]); a marker whose
/// age cannot be read is left for a later sweep rather than blindly deleted. Runs on the *pre* path,
/// so a marker written this run (now-aged 0) is never its own victim.
pub fn sweep_stale(state_dir: &Path, now_ms: u64, max_age_ms: u64) {
    let dir = state_dir.join("durations");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten().take(SWEEP_SCAN_CAP) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(marker) = serde_json::from_str::<StartMarker>(&text) else { continue };
        if now_ms.saturating_sub(marker.t_start_ms) > max_age_ms {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn call_key_is_deterministic_and_identity_scoped() {
        let input = r#"{"command":"ls -la"}"#;
        let a = call_key(Some("sess-1"), "Bash", input);
        let b = call_key(Some("sess-1"), "Bash", input);
        assert_eq!(a, b, "same inputs ⇒ same key (the pre/post correlation)");
        assert_eq!(a.len(), 64, "full sha-256 hex");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "filesystem-safe hex");
        // Every identity component distinguishes.
        assert_ne!(a, call_key(Some("sess-2"), "Bash", input));
        assert_ne!(a, call_key(Some("sess-1"), "Read", input));
        assert_ne!(a, call_key(Some("sess-1"), "Bash", r#"{"command":"pwd"}"#));
        // Absent session hashes identically to an empty one (mirrors pay_id_for).
        assert_eq!(call_key(None, "Bash", "{}"), call_key(Some(""), "Bash", "{}"));
    }

    #[test]
    fn compute_dur_is_honest_about_negatives() {
        assert_eq!(compute_dur_ms(1000, 1120), Some(120));
        assert_eq!(compute_dur_ms(1000, 1000), Some(0), "a genuine sub-ms call is 0, not absent");
        assert_eq!(compute_dur_ms(1120, 1000), None, "end before start ⇒ gap, never a clamped 0");
    }

    #[test]
    fn annotate_inserts_both_params_and_noops_on_non_object() {
        let mut p = json!({"command": "ls"});
        annotate_duration(&mut p, 4300, BASIS_HOOK_PRE_POST);
        assert_eq!(p[DUR_MS_KEY], json!(4300));
        assert_eq!(p[DUR_BASIS_KEY], json!("hook-pre-post"));
        assert_eq!(p["command"], json!("ls"), "existing params untouched");

        let mut arr = json!([1, 2, 3]);
        annotate_duration(&mut arr, 10, BASIS_IN_PROCESS);
        assert_eq!(arr, json!([1, 2, 3]), "no-op on a non-object");
    }

    fn tmp_state_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kriya-dur-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn marker_roundtrip_is_single_use() {
        let dir = tmp_state_dir("roundtrip");
        let key = call_key(Some("s"), "Bash", "{}");
        write_start(&dir, &key, 1_700_000_000_000);
        assert!(marker_path(&dir, &key).is_file(), "marker persisted");
        assert_eq!(take_end(&dir, &key), Some(1_700_000_000_000), "post recovers the start");
        assert!(!marker_path(&dir, &key).exists(), "marker deleted on take (single-use)");
        assert_eq!(take_end(&dir, &key), None, "a second take is an honest miss");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn take_missing_marker_is_none() {
        let dir = tmp_state_dir("missing");
        assert_eq!(take_end(&dir, "deadbeef"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_removes_only_stale_markers() {
        let dir = tmp_state_dir("sweep");
        let now = 1_700_000_000_000u64;
        let old = call_key(Some("old"), "Bash", "{}");
        let fresh = call_key(Some("fresh"), "Bash", "{}");
        write_start(&dir, &old, now - MAX_MARKER_AGE_MS - 1); // just past the horizon
        write_start(&dir, &fresh, now - 1000); // a second ago
        sweep_stale(&dir, now, MAX_MARKER_AGE_MS);
        assert!(!marker_path(&dir, &old).exists(), "stale marker swept");
        assert!(marker_path(&dir, &fresh).is_file(), "fresh marker kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_never_panics_when_state_dir_is_unwritable() {
        // A state dir that is actually a FILE ⇒ create_dir_all under it fails ⇒ write is a silent
        // no-op (the gate must proceed regardless). The test asserts only that nothing panics and
        // no marker appears.
        let base = tmp_state_dir("unwritable");
        let state_as_file = base.join("state");
        std::fs::write(&state_as_file, b"i am a file, not a dir").unwrap();
        let key = call_key(Some("s"), "Bash", "{}");
        write_start(&state_as_file, &key, 123); // must not panic
        assert!(!marker_path(&state_as_file, &key).exists());
        assert_eq!(take_end(&state_as_file, &key), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
