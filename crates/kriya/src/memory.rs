//! Persistent episodic memory (SQLite). The host records every action the agent takes —
//! across runs — so it can recall prior activity, surface "what have I done before?", and
//! resume an interrupted task. This is the durable counterpart to the in-process `history`
//! the inference backends see during a single run.
//!
//! Each episode carries a `run_id` (a UUID minted at the start of a run) and the originating
//! `goal`. That pair is what makes resume possible: given a goal, look up the most recent
//! run, replay its completed actions into the agent's in-memory history, and pick up where
//! we left off.
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
    pub run_id: String,
    pub goal: String,
}

/// One run we could resume from. The list of completed actions (ids + params) gives the
/// host enough to reconstruct the inference backend's in-memory `history`.
#[derive(Debug, Clone)]
pub struct ResumableRun {
    pub run_id: String,
    pub goal: String,
    pub completed: Vec<CompletedStep>,
}

#[derive(Debug, Clone)]
pub struct CompletedStep {
    pub action_id: String,
    pub params: Value,
    pub success: bool,
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

        // Backwards-compatible migration: stores opened before the run_id/goal addition
        // still work; new rows carry the metadata. Old rows get empty strings on read.
        if !column_exists(&conn, "episodes", "run_id")? {
            conn.execute("ALTER TABLE episodes ADD COLUMN run_id TEXT NOT NULL DEFAULT ''", [])
                .map_err(|e| e.to_string())?;
        }
        if !column_exists(&conn, "episodes", "goal")? {
            conn.execute("ALTER TABLE episodes ADD COLUMN goal TEXT NOT NULL DEFAULT ''", [])
                .map_err(|e| e.to_string())?;
        }

        // Index that makes resume lookup (by goal) fast even with thousands of episodes.
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_episodes_goal_id ON episodes (goal, id)",
            [],
        )
        .map_err(|e| e.to_string())?;

        Ok(Self { conn })
    }

    /// Append one action episode.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        ts_ms: u128,
        run_id: &str,
        goal: &str,
        action_id: &str,
        params: &Value,
        success: bool,
        reasoning: &str,
        signature: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO episodes (ts_ms, run_id, goal, action_id, params, success, reasoning, signature)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    ts_ms as i64,
                    run_id,
                    goal,
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
                "SELECT ts_ms, action_id, params, success, reasoning, signature, run_id, goal
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
                    run_id: r.get(6)?,
                    goal: r.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// The most recent run that targeted `goal`, if any, with its completed action steps
    /// in chronological order. The host uses this to reseed the in-memory `history` after
    /// a crash/restart so the inference backend doesn't propose actions already taken.
    pub fn last_resumable_run(&self, goal: &str) -> Result<Option<ResumableRun>, String> {
        // Find the latest run_id for this goal (newest by id), excluding the empty-string
        // run_id used for old pre-migration rows.
        let last_run_id: Option<String> = self
            .conn
            .query_row(
                "SELECT run_id FROM episodes
                 WHERE goal = ?1 AND run_id <> ''
                 ORDER BY id DESC LIMIT 1",
                params![goal],
                |r| r.get::<_, String>(0),
            )
            .ok();

        let Some(run_id) = last_run_id else { return Ok(None) };

        let mut stmt = self
            .conn
            .prepare(
                "SELECT action_id, params, success
                 FROM episodes WHERE run_id = ?1 ORDER BY id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![run_id], |r| {
                let action_id: String = r.get(0)?;
                let params_str: String = r.get(1)?;
                let success: i64 = r.get(2)?;
                Ok((action_id, params_str, success != 0))
            })
            .map_err(|e| e.to_string())?;

        let mut completed = Vec::new();
        for row in rows {
            let (action_id, params_str, success) = row.map_err(|e| e.to_string())?;
            let params: Value = serde_json::from_str(&params_str).unwrap_or(Value::Null);
            completed.push(CompletedStep { action_id, params, success });
        }

        Ok(Some(ResumableRun {
            run_id,
            goal: goal.to_string(),
            completed,
        }))
    }
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    for row in rows {
        if row.map_err(|e| e.to_string())? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(m: &AgentMemory, ts: u128, run: &str, goal: &str, action: &str, ok: bool) {
        m.record(ts, run, goal, action, &json!({ "id": "x" }), ok, "because", "sig").unwrap();
    }

    #[test]
    fn records_and_counts_across_calls() {
        let m = AgentMemory::open_in_memory().unwrap();
        assert_eq!(m.count().unwrap(), 0);
        m.record(1, "r1", "g1", "edit_note", &json!({ "id": "n1" }), true, "because", "sig1").unwrap();
        m.record(2, "r1", "g1", "delete_note", &json!({ "id": "n2" }), false, "denied", "sig2").unwrap();
        assert_eq!(m.count().unwrap(), 2);
    }

    #[test]
    fn recent_returns_newest_first_with_run_metadata() {
        let m = AgentMemory::open_in_memory().unwrap();
        rec(&m, 1, "r1", "organize notes", "edit_note", true);
        rec(&m, 2, "r1", "organize notes", "delete_note", true);
        let recent = m.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].action_id, "delete_note");
        assert_eq!(recent[0].run_id, "r1");
        assert_eq!(recent[0].goal, "organize notes");
        assert_eq!(recent[1].action_id, "edit_note");
    }

    #[test]
    fn last_resumable_run_returns_only_matching_goal() {
        let m = AgentMemory::open_in_memory().unwrap();
        rec(&m, 1, "r1", "goal_a", "a1", true);
        rec(&m, 2, "r1", "goal_a", "a2", true);
        rec(&m, 3, "r2", "goal_b", "b1", true);

        let resumable = m.last_resumable_run("goal_a").unwrap().unwrap();
        assert_eq!(resumable.run_id, "r1");
        assert_eq!(resumable.completed.len(), 2);
        assert_eq!(resumable.completed[0].action_id, "a1");
        assert_eq!(resumable.completed[1].action_id, "a2");

        let none = m.last_resumable_run("unknown_goal").unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn last_resumable_picks_newest_run_for_goal() {
        let m = AgentMemory::open_in_memory().unwrap();
        rec(&m, 1, "old_run", "goal_x", "a", true);
        rec(&m, 2, "old_run", "goal_x", "b", true);
        rec(&m, 3, "new_run", "goal_x", "c", true);
        rec(&m, 4, "new_run", "goal_x", "d", true);

        let resumable = m.last_resumable_run("goal_x").unwrap().unwrap();
        assert_eq!(resumable.run_id, "new_run");
        assert_eq!(resumable.completed.len(), 2);
        assert_eq!(resumable.completed[0].action_id, "c");
    }

    #[test]
    fn recent_respects_limit() {
        let m = AgentMemory::open_in_memory().unwrap();
        for i in 0..5 {
            m.record(i, "r", "g", "edit_note", &json!({}), true, "", "s").unwrap();
        }
        assert_eq!(m.recent(3).unwrap().len(), 3);
    }
}
