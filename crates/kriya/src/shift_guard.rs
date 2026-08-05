//! F-6 (doc 31 §9 / doc 33 §4-C; design `../kriya-console/docs/ideas/design/F6-shift.md`) — the
//! runtime side of the shift lane: read the Console-written armed state (`~/.kriya/state/shift.json`)
//! and, when armed **and a heartbeat was missed inside the window**, CLAMP the resolved action tier
//! to the pre-declared fail-closed tier.
//!
//! **Tighten-only (axiom 5):** the clamp can only escalate an `Allow` to `approval`|`deny`, never
//! loosen anything — the exact idiom the temporal (B4) and budget (C2) lanes use.
//!
//! **State absent/unparseable ⇒ NO clamp** (the `session_cond.rs` "cache is never the source of
//! truth" law): availability is never held hostage to a state file the Console owns. The
//! *enforcement* fail-closed is the clamp itself — applied only when the state IS readable, says
//! armed, `now` is inside the window, and a beat was actually missed. The reader (`~/.kriya/state`)
//! is the same one `spend_state::state_dir_for_audit_log` derives; the Console writer is
//! `attest/shift.rs::write_shift_state` — the two agree on the path with no new flag.

use std::path::Path;

use serde::Deserialize;

use crate::permissions::Tier;

/// The on-disk shift-armed state, matching the Console writer's `ShiftState` field-for-field. Every
/// numeric field defaults to `0` and `fail_tier` to `None` so a partial/foreign file still
/// deserializes to a **disarmed-equivalent** shape (no clamp) rather than failing the read.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct ShiftGuardState {
    pub armed: bool,
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default)]
    pub end_ms: u64,
    #[serde(default)]
    pub fail_tier: Option<String>,
    #[serde(default)]
    pub cadence_ms: u64,
}

impl ShiftGuardState {
    /// The declared fail-closed tier as a [`Tier`] — only the two escalation tiers are valid (a
    /// clamp that "escalated" to `Allow` would be a no-op / a loosening, forbidden by axiom 5).
    pub fn fail_tier(&self) -> Option<Tier> {
        match self.fail_tier.as_deref() {
            Some("approval") => Some(Tier::Approval),
            Some("deny") => Some(Tier::Deny),
            _ => None,
        }
    }

    /// The clamp decision — pure, so the gap→tier-drop logic is table-testable without a filesystem.
    /// `Some(tier)` iff: armed **and** `now_ms ∈ [start_ms, end_ms)` **and** a beat was missed
    /// (no heartbeat observed at all, or the last one is older than `2× cadence` — the same "> 2×
    /// the observed cadence" gap rule the Console composer folds with). Otherwise `None`.
    pub fn clamp(&self, now_ms: u64, last_heartbeat_ms: Option<u64>) -> Option<Tier> {
        if !self.armed {
            return None;
        }
        if now_ms < self.start_ms || now_ms >= self.end_ms {
            return None; // outside the declared window — the shift isn't in force
        }
        let tier = self.fail_tier()?;
        let cadence = if self.cadence_ms == 0 { 60_000 } else { self.cadence_ms };
        let missed = match last_heartbeat_ms {
            None => true, // no beat observed at all — missed by definition (the watcher is silent)
            Some(last) => now_ms.saturating_sub(last) > 2 * cadence,
        };
        if missed {
            Some(tier)
        } else {
            None
        }
    }
}

/// Load `<state_dir>/shift.json`, or `None` (absent/unparseable ⇒ no clamp — never an error the hot
/// path must handle).
pub fn load(state_dir: &Path) -> Option<ShiftGuardState> {
    let text = std::fs::read_to_string(state_dir.join("shift.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The `ts_ms` of the most recent `kriya.watch.heartbeat` receipt in `audit_log`, or `None`.
/// **Bounded tail read** (`MAX_TAIL_BYTES`) so a large audit log never turns the hook into an
/// O(file) scan on the hot path — heartbeats tick ~every 60s, so the last one is always well within
/// the tail window.
pub fn last_heartbeat_ms(audit_log: &Path) -> Option<u64> {
    const MAX_TAIL_BYTES: u64 = 512 * 1024;
    let text = read_tail(audit_log, MAX_TAIL_BYTES)?;
    let mut last: Option<u64> = None;
    for line in text.lines() {
        if !line.contains("kriya.watch.heartbeat") {
            continue; // cheap pre-filter before the JSON parse
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("action_id").and_then(|a| a.as_str()) == Some("kriya.watch.heartbeat") {
                if let Some(ts) = v.get("ts_ms").and_then(|t| t.as_u64()) {
                    last = Some(last.map_or(ts, |p| p.max(ts)));
                }
            }
        }
    }
    last
}

/// Read at most the final `max_bytes` of `path` as UTF-8, dropping any partial first line when the
/// read started mid-file (so callers only ever see whole lines).
fn read_tail(path: &Path, max_bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let s = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        if let Some(nl) = s.find('\n') {
            return Some(s[nl + 1..].to_string());
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const START: u64 = 1_700_000_000_000;
    const END: u64 = START + 9 * 3_600_000; // 22:00 → 07:00

    fn armed(fail: &str) -> ShiftGuardState {
        ShiftGuardState { armed: true, start_ms: START, end_ms: END, fail_tier: Some(fail.into()), cadence_ms: 60_000 }
    }

    #[test]
    fn disarmed_or_out_of_window_never_clamps() {
        let mut s = armed("deny");
        s.armed = false;
        assert_eq!(s.clamp(START + 1000, None), None, "disarmed → no clamp");

        let s = armed("deny");
        assert_eq!(s.clamp(START - 1, None), None, "before the window → no clamp");
        assert_eq!(s.clamp(END, None), None, "at/after the window end → no clamp");
    }

    #[test]
    fn armed_and_missed_beat_clamps_to_fail_tier() {
        let s = armed("deny");
        // no beat at all inside the window
        assert_eq!(s.clamp(START + 5_000, None), Some(Tier::Deny));
        // last beat older than 2× cadence (>120s ago)
        assert_eq!(s.clamp(START + 300_000, Some(START + 100_000)), Some(Tier::Deny));

        let s = armed("approval");
        assert_eq!(s.clamp(START + 5_000, None), Some(Tier::Approval));
    }

    #[test]
    fn armed_but_recent_beat_does_not_clamp() {
        let s = armed("deny");
        // last beat 60s ago (< 2× cadence) — the watcher is alive, no clamp
        assert_eq!(s.clamp(START + 160_000, Some(START + 100_000)), None);
    }

    #[test]
    fn invalid_or_absent_fail_tier_never_clamps() {
        let s = ShiftGuardState { armed: true, start_ms: START, end_ms: END, fail_tier: None, cadence_ms: 60_000 };
        assert_eq!(s.clamp(START + 5_000, None), None);
        let s = ShiftGuardState { armed: true, start_ms: START, end_ms: END, fail_tier: Some("allow".into()), cadence_ms: 60_000 };
        assert_eq!(s.clamp(START + 5_000, None), None, "an 'allow' clamp would be a loosening — forbidden");
    }

    #[test]
    fn load_roundtrips_and_absent_is_none() {
        let dir = std::env::temp_dir().join(format!("kriya-shiftguard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir), None, "absent state → None, never an error");

        let body = serde_json::json!({ "armed": true, "start_ms": START, "end_ms": END, "fail_tier": "deny", "cadence_ms": 60000 });
        std::fs::write(dir.join("shift.json"), serde_json::to_vec(&body).unwrap()).unwrap();
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.clamp(START + 5_000, None), Some(Tier::Deny));

        // a partial/foreign file still deserializes to a disarmed-equivalent (no clamp), never a panic
        std::fs::write(dir.join("shift.json"), br#"{"armed":false}"#).unwrap();
        assert_eq!(load(&dir).unwrap().clamp(START + 5_000, None), None);
        // garbage → None
        std::fs::write(dir.join("shift.json"), b"not json").unwrap();
        assert_eq!(load(&dir), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_heartbeat_reads_the_freshest_beat_from_the_tail() {
        let dir = std::env::temp_dir().join(format!("kriya-shiftguard-hb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("claude-code.jsonl");
        let mut f = std::fs::File::create(&log).unwrap();
        // mixed receipts + heartbeats out of order; the reader takes the max ts_ms heartbeat
        writeln!(f, r#"{{"action_id":"claude-code__bash","ts_ms":{}}}"#, START + 1).unwrap();
        writeln!(f, r#"{{"action_id":"kriya.watch.heartbeat","ts_ms":{}}}"#, START + 100).unwrap();
        writeln!(f, r#"{{"action_id":"kriya.watch.heartbeat","ts_ms":{}}}"#, START + 400).unwrap();
        writeln!(f, r#"{{"action_id":"kriya.watch.run.start","ts_ms":{}}}"#, START + 500).unwrap();
        drop(f);
        assert_eq!(last_heartbeat_ms(&log), Some(START + 400));

        // a log with no heartbeat at all → None (missed by definition)
        let log2 = dir.join("empty.jsonl");
        std::fs::write(&log2, r#"{"action_id":"claude-code__bash","ts_ms":1}"#).unwrap();
        assert_eq!(last_heartbeat_ms(&log2), None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
