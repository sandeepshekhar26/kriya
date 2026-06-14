//! Persistent episodic memory (SQLite). The host records every action the agent takes —
//! across runs — so it can recall prior activity, surface "what have I done before?", and
//! (in a later phase) resume an interrupted task. This is the durable counterpart to the
//! in-process `history` the inference backends see during a single run.
//!
//! The connection lives on the single agent thread, so it is never shared concurrently.

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

pub struct AgentMemory {
    conn: Connection,
}

/// One recorded action, as read back from the store. Serializes to the frontend inspector.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub ts_ms: i64,
    pub action_id: String,
    pub params: String,
    pub success: bool,
    pub reasoning: String,
    pub signature: String,
}

impl AgentMemory {
    /// Open (creating if needed) the episodic store at `path`.
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        Self::init(conn)
    }

    /// An ephemeral in-memory store (used by tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, String> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS episodes (
                 id        INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts_ms     INTEGER NOT NULL,
                 action_id TEXT    NOT NULL,
                 params    TEXT    NOT NULL,
                 success   INTEGER NOT NULL,
                 reasoning TEXT    NOT NULL,
                 signature TEXT    NOT NULL
             );",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    /// Append one action episode.
    pub fn record(
        &self,
        ts_ms: u128,
        action_id: &str,
        params: &Value,
        success: bool,
        reasoning: &str,
        signature: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO episodes (ts_ms, action_id, params, success, reasoning, signature)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    ts_ms as i64,
                    action_id,
                    params.to_string(),
                    success as i64,
                    reasoning,
                    signature
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Total number of episodes on record (across all runs).
    pub fn count(&self) -> Result<u64, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(|e| e.to_string())
    }

    /// The most recent `limit` episodes, newest first.
    pub fn recent(&self, limit: u32) -> Result<Vec<Episode>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ts_ms, action_id, params, success, reasoning, signature
                 FROM episodes ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(Episode {
                    ts_ms: r.get(0)?,
                    action_id: r.get(1)?,
                    params: r.get(2)?,
                    success: r.get::<_, i64>(3)? != 0,
                    reasoning: r.get(4)?,
                    signature: r.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_and_counts_across_calls() {
        let m = AgentMemory::open_in_memory().unwrap();
        assert_eq!(m.count().unwrap(), 0);
        m.record(1, "edit_note", &json!({ "id": "n1" }), true, "because", "sig1").unwrap();
        m.record(2, "delete_note", &json!({ "id": "n2" }), false, "denied", "sig2").unwrap();
        assert_eq!(m.count().unwrap(), 2);
    }

    #[test]
    fn recent_returns_newest_first() {
        let m = AgentMemory::open_in_memory().unwrap();
        m.record(1, "edit_note", &json!({}), true, "", "a").unwrap();
        m.record(2, "delete_note", &json!({}), true, "", "b").unwrap();
        let recent = m.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].action_id, "delete_note");
        assert_eq!(recent[1].action_id, "edit_note");
    }

    #[test]
    fn recent_respects_limit() {
        let m = AgentMemory::open_in_memory().unwrap();
        for i in 0..5 {
            m.record(i, "edit_note", &json!({}), true, "", "s").unwrap();
        }
        assert_eq!(m.recent(3).unwrap().len(), 3);
    }
}
