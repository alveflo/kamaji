use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};

use crate::models::{Agent, Project, Status, Ticket};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS projects (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL,
    root_dir      TEXT NOT NULL,
    default_agent TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS tickets (
    id             INTEGER PRIMARY KEY,
    project_id     INTEGER NOT NULL REFERENCES projects(id),
    title          TEXT NOT NULL,
    description    TEXT NOT NULL DEFAULT '',
    initial_prompt TEXT,
    agent          TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'todo',
    position       INTEGER NOT NULL DEFAULT 0,
    session_name   TEXT,
    worktree_path  TEXT,
    branch         TEXT,
    auto_reviewed  INTEGER NOT NULL DEFAULT 0,
    instrumented   INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
";

/// Add a column to `table` if it isn't already present. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`, so we check `PRAGMA table_info` first. This keeps
/// databases created by older kamaji versions forward-compatible.
fn add_column_if_missing(conn: &Connection, table: &str, col: &str, decl: &str) -> Result<()> {
    let present = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(std::result::Result::ok)
        .any(|name| name == col);
    if !present {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl}"), [])?;
    }
    Ok(())
}

/// Bring an existing database up to the current schema (idempotent).
fn migrate(conn: &Connection) -> Result<()> {
    add_column_if_missing(
        conn,
        "tickets",
        "auto_reviewed",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "tickets",
        "instrumented",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(())
}

pub struct Db {
    conn: Connection,
}

fn parse_col<T: std::str::FromStr>(s: &str, col: &str) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    s.parse().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid {col}: {e}"),
            )),
        )
    })
}

fn row_to_project(row: &Row) -> rusqlite::Result<Project> {
    let agent: Option<String> = row.get("default_agent")?;
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        root_dir: PathBuf::from(row.get::<_, String>("root_dir")?),
        default_agent: match agent {
            Some(a) => Some(parse_col(&a, "default_agent")?),
            None => None,
        },
        created_at: row.get("created_at")?,
    })
}

fn row_to_ticket(row: &Row) -> rusqlite::Result<Ticket> {
    let agent: String = row.get("agent")?;
    let status: String = row.get("status")?;
    let worktree: Option<String> = row.get("worktree_path")?;
    Ok(Ticket {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        title: row.get("title")?,
        description: row.get("description")?,
        initial_prompt: row.get("initial_prompt")?,
        agent: parse_col(&agent, "agent")?,
        status: parse_col(&status, "status")?,
        position: row.get("position")?,
        session_name: row.get("session_name")?,
        worktree_path: worktree.map(PathBuf::from),
        branch: row.get("branch")?,
        auto_reviewed: row.get::<_, i64>("auto_reviewed")? != 0,
        instrumented: row.get::<_, i64>("instrumented")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Db { conn })
    }

    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Db { conn })
    }

    pub fn create_project(
        &self,
        name: &str,
        root_dir: &Path,
        default_agent: Option<Agent>,
    ) -> Result<Project> {
        self.conn.execute(
            "INSERT INTO projects (name, root_dir, default_agent) VALUES (?1, ?2, ?3)",
            params![
                name,
                root_dir.to_string_lossy(),
                default_agent.map(|a| a.as_str())
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.get_project(id)?.expect("just inserted"))
    }

    pub fn get_project(&self, id: i64) -> Result<Option<Project>> {
        Ok(self
            .conn
            .query_row("SELECT * FROM projects WHERE id = ?1", [id], row_to_project)
            .optional()?)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare("SELECT * FROM projects ORDER BY name")?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn create_ticket(
        &self,
        project_id: i64,
        title: &str,
        description: &str,
        initial_prompt: Option<&str>,
        agent: Agent,
    ) -> Result<Ticket> {
        self.conn.execute(
            "INSERT INTO tickets (project_id, title, description, initial_prompt, agent, status)
             VALUES (?1, ?2, ?3, ?4, ?5, 'todo')",
            params![
                project_id,
                title,
                description,
                initial_prompt,
                agent.as_str()
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.get_ticket(id)?.expect("just inserted"))
    }

    pub fn get_ticket(&self, id: i64) -> Result<Option<Ticket>> {
        Ok(self
            .conn
            .query_row("SELECT * FROM tickets WHERE id = ?1", [id], row_to_ticket)
            .optional()?)
    }

    pub fn list_tickets(&self, project_id: i64) -> Result<Vec<Ticket>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM tickets WHERE project_id = ?1 ORDER BY position, id")?;
        let rows = stmt.query_map([project_id], row_to_ticket)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn update_ticket_fields(&self, id: i64, title: &str, description: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET title = ?2, description = ?3, updated_at = datetime('now') WHERE id = ?1",
            params![id, title, description],
        )?;
        Ok(())
    }

    pub fn set_ticket_status(&self, id: i64, status: Status) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, status.as_str()],
        )?;
        Ok(())
    }

    pub fn set_ticket_session(
        &self,
        id: i64,
        session_name: &str,
        worktree_path: &str,
        branch: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET session_name = ?2, worktree_path = ?3, branch = ?4,
             updated_at = datetime('now') WHERE id = ?1",
            params![id, session_name, worktree_path, branch],
        )?;
        Ok(())
    }

    /// Mark (or unmark) a ticket as auto-moved to Review by kamaji. Persisted so
    /// the move back to In Progress survives a restart.
    pub fn set_ticket_auto_reviewed(&self, id: i64, value: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET auto_reviewed = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, value as i64],
        )?;
        Ok(())
    }

    /// Record whether a ticket's session was started with the idle-detection
    /// hooks. Only an instrumented session's activity signal is trustworthy.
    pub fn set_ticket_instrumented(&self, id: i64, value: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET instrumented = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, value as i64],
        )?;
        Ok(())
    }

    /// Clear the session/worktree/branch columns (e.g. after cleanup or when a
    /// session no longer exists). Also resets the per-session detection flags,
    /// since they describe a session that no longer exists.
    pub fn clear_ticket_session(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET session_name = NULL, worktree_path = NULL, branch = NULL,
             auto_reviewed = 0, instrumented = 0, updated_at = datetime('now') WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn delete_ticket(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM tickets WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn project_create_get_list() {
        let db = db();
        let p = db
            .create_project("acme", &PathBuf::from("/tmp/acme"), Some(Agent::Codex))
            .unwrap();
        assert!(p.id > 0);
        assert_eq!(db.get_project(p.id).unwrap().unwrap().name, "acme");
        assert_eq!(db.list_projects().unwrap().len(), 1);
        assert_eq!(
            db.get_project(p.id).unwrap().unwrap().default_agent,
            Some(Agent::Codex)
        );
    }

    #[test]
    fn ticket_lifecycle() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        let t = db
            .create_ticket(p.id, "Add login", "desc", Some("do it"), Agent::Claude)
            .unwrap();
        assert_eq!(t.status, Status::Todo);
        assert_eq!(t.session_name, None);

        db.update_ticket_fields(t.id, "Add SSO", "new desc")
            .unwrap();
        db.set_ticket_status(t.id, Status::InProgress).unwrap();
        db.set_ticket_session(t.id, "kamaji-1-add-sso", "/wt", "kamaji-1-add-sso")
            .unwrap();

        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(got.title, "Add SSO");
        assert_eq!(got.status, Status::InProgress);
        assert_eq!(got.session_name.as_deref(), Some("kamaji-1-add-sso"));
        assert_eq!(got.worktree_path, Some(PathBuf::from("/wt")));

        assert_eq!(db.list_tickets(p.id).unwrap().len(), 1);
        db.delete_ticket(t.id).unwrap();
        assert_eq!(db.list_tickets(p.id).unwrap().len(), 0);
    }

    #[test]
    fn clear_ticket_session_nulls_columns() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        let t = db
            .create_ticket(p.id, "t", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(t.id, "kamaji-1-t", "/wt", "kamaji-1-t")
            .unwrap();
        db.clear_ticket_session(t.id).unwrap();
        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(got.session_name, None);
        assert_eq!(got.worktree_path, None);
        assert_eq!(got.branch, None);
    }

    #[test]
    fn detection_flags_default_false_and_round_trip() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        let t = db
            .create_ticket(p.id, "t", "", None, Agent::Claude)
            .unwrap();
        assert!(!t.auto_reviewed);
        assert!(!t.instrumented);

        db.set_ticket_auto_reviewed(t.id, true).unwrap();
        db.set_ticket_instrumented(t.id, true).unwrap();
        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert!(got.auto_reviewed);
        assert!(got.instrumented);
    }

    #[test]
    fn clear_ticket_session_resets_detection_flags() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        let t = db
            .create_ticket(p.id, "t", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_session(t.id, "s", "/wt", "s").unwrap();
        db.set_ticket_auto_reviewed(t.id, true).unwrap();
        db.set_ticket_instrumented(t.id, true).unwrap();
        db.clear_ticket_session(t.id).unwrap();
        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert!(!got.auto_reviewed);
        assert!(!got.instrumented);
    }

    #[test]
    fn migrate_adds_missing_columns_and_is_idempotent() {
        // A pre-migration tickets table (no auto_reviewed / instrumented).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tickets (
                id INTEGER PRIMARY KEY, project_id INTEGER, title TEXT, description TEXT,
                initial_prompt TEXT, agent TEXT, status TEXT, position INTEGER,
                session_name TEXT, worktree_path TEXT, branch TEXT,
                created_at TEXT, updated_at TEXT);",
        )
        .unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap(); // idempotent: second run must not error
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(tickets)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();
        assert!(cols.contains(&"auto_reviewed".to_string()));
        assert!(cols.contains(&"instrumented".to_string()));
    }
}
