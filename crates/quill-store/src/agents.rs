//! The log of what was done to the mail from outside Quill's window.
//!
//! One line for each thing asked by a command for a shell or by an agent
//! over MCP: when, by which way in, under what name, and what. A change
//! carries what is needed to take it back. A look is noted more briefly,
//! and never with what a message said.

use rusqlite::{OptionalExtension, params};

use crate::{Result, Store};

/// A line of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    pub id: i64,
    pub at: i64,
    /// `shell` or `mcp`.
    pub way: String,
    /// The name the caller gave.
    pub agent: String,
    /// The command.
    pub action: String,
    /// The mail was changed, or mail was sent.
    pub change: bool,
    pub summary: String,
    pub count: u32,
    /// What is needed to take the change back, as the caller wrote it.
    pub undo: Option<String>,
    /// When it was taken back.
    pub undone: Option<i64>,
}

/// A line to add to the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAgentEntry<'a> {
    pub at: i64,
    pub way: &'a str,
    pub agent: &'a str,
    pub action: &'a str,
    pub change: bool,
    pub summary: &'a str,
    pub count: u32,
    pub undo: Option<&'a str>,
    /// A message sent: when it is due to go.
    pub due: Option<i64>,
}

impl Store {
    /// Adds a line to the log and returns its number.
    pub fn log_agent(&mut self, entry: &NewAgentEntry<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO agent_log (at, way, agent, action, change, summary, count, undo, due)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                entry.at,
                entry.way,
                entry.agent,
                entry.action,
                entry.change,
                entry.summary,
                entry.count,
                entry.undo,
                entry.due,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The newest lines of the log, newest first: the changes, or with
    /// `looks` what was looked at as well.
    pub fn agent_log(&self, limit: usize, looks: bool) -> Result<Vec<AgentEntry>> {
        let mut statement = self.conn.prepare(
            "SELECT id, at, way, agent, action, change, summary, count, undo, undone
               FROM agent_log
              WHERE change = 1 OR ?1
              ORDER BY at DESC, id DESC
              LIMIT ?2",
        )?;
        let rows = statement
            .query_map(
                params![looks, i64::try_from(limit).unwrap_or(i64::MAX)],
                entry,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// One line of the log, by its number.
    pub fn agent_entry(&self, id: i64) -> Result<Option<AgentEntry>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, at, way, agent, action, change, summary, count, undo, undone
                   FROM agent_log WHERE id = ?1",
                [id],
                entry,
            )
            .optional()?)
    }

    /// The newest change that can still be taken back: one that carries
    /// what is needed and has not been.
    pub fn last_agent_change(&self) -> Result<Option<AgentEntry>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, at, way, agent, action, change, summary, count, undo, undone
                   FROM agent_log
                  WHERE change = 1 AND undo IS NOT NULL AND undone IS NULL
                  ORDER BY at DESC, id DESC
                  LIMIT 1",
                [],
                entry,
            )
            .optional()?)
    }

    /// Notes that a change was taken back. Returns whether it was there
    /// and had not been already.
    pub fn mark_agent_undone(&mut self, id: i64, now: i64) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE agent_log SET undone = ?2 WHERE id = ?1 AND undone IS NULL",
            params![id, now],
        )?;
        Ok(changed > 0)
    }

    /// Lets go of the lines older than `before`. Returns how many went.
    pub fn prune_agent_log(&mut self, before: i64) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM agent_log WHERE at < ?1", [before])?)
    }

    /// When the newest message sent from outside Quill's window is, or
    /// was, due to go: for keeping the wait between one and the next.
    pub fn last_agent_send(&self) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT max(due) FROM agent_log WHERE due IS NOT NULL AND undone IS NULL",
            [],
            |row| row.get(0),
        )?)
    }
}

fn entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentEntry> {
    Ok(AgentEntry {
        id: row.get(0)?,
        at: row.get(1)?,
        way: row.get(2)?,
        agent: row.get(3)?,
        action: row.get(4)?,
        change: row.get(5)?,
        summary: row.get(6)?,
        count: row.get(7)?,
        undo: row.get(8)?,
        undone: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line<'a>(
        at: i64,
        action: &'a str,
        change: bool,
        undo: Option<&'a str>,
    ) -> NewAgentEntry<'a> {
        NewAgentEntry {
            at,
            way: "shell",
            agent: "quill",
            action,
            change,
            summary: "a line",
            count: 2,
            undo,
            due: (action == "send").then_some(at),
        }
    }

    #[test]
    fn the_log_lists_changes_and_looks_only_when_asked() {
        let mut store = Store::open_in_memory().unwrap();
        let looked = store.log_agent(&line(10, "search", false, None)).unwrap();
        let moved = store
            .log_agent(&line(20, "move", true, Some("{}")))
            .unwrap();
        let marked = store.log_agent(&line(30, "mark", true, None)).unwrap();

        let changes: Vec<i64> = store
            .agent_log(10, false)
            .unwrap()
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(changes, [marked, moved]);
        let all: Vec<i64> = store
            .agent_log(10, true)
            .unwrap()
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(all, [marked, moved, looked]);
        assert_eq!(store.agent_log(1, true).unwrap().len(), 1);

        let entry = store.agent_entry(moved).unwrap().unwrap();
        assert_eq!(
            (entry.way.as_str(), entry.agent.as_str(), entry.count),
            ("shell", "quill", 2)
        );
        assert!(entry.change);
        assert_eq!(store.agent_entry(999).unwrap(), None);
    }

    #[test]
    fn a_change_is_taken_back_once_and_the_newest_that_can_be_is_found() {
        let mut store = Store::open_in_memory().unwrap();
        let moved = store
            .log_agent(&line(20, "move", true, Some("{}")))
            .unwrap();
        // A change that carries nothing to take it back with is passed
        // over.
        store.log_agent(&line(30, "mark", true, None)).unwrap();
        assert_eq!(store.last_agent_change().unwrap().unwrap().id, moved);
        assert!(store.mark_agent_undone(moved, 40).unwrap());
        assert!(!store.mark_agent_undone(moved, 50).unwrap());
        assert_eq!(store.agent_entry(moved).unwrap().unwrap().undone, Some(40));
        assert_eq!(store.last_agent_change().unwrap(), None);
    }

    #[test]
    fn old_lines_go_and_the_last_send_is_known() {
        let mut store = Store::open_in_memory().unwrap();
        assert_eq!(store.last_agent_send().unwrap(), None);
        store.log_agent(&line(10, "send", true, None)).unwrap();
        store.log_agent(&line(70, "send", true, None)).unwrap();
        store.log_agent(&line(90, "move", true, None)).unwrap();
        assert_eq!(store.last_agent_send().unwrap(), Some(70));
        assert_eq!(store.prune_agent_log(50).unwrap(), 1);
        assert_eq!(store.agent_log(10, true).unwrap().len(), 2);
    }
}
