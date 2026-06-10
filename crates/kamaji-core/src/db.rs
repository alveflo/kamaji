//! SQLite persistence for projects and tickets: the schema, lightweight column
//! migrations (`PRAGMA table_info` checks), and the CRUD/query helpers the
//! daemon uses as its single source of truth.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::{Path, PathBuf};

use crate::models::{Agent, Project, Status, Ticket};

/// Expand a leading `~/` in a project root to the home directory. Project roots
/// arrive from several entry points (the browser form, the edit-project modal,
/// the HTTP API) and only the TUI's new-project picker expands `~` itself; doing
/// it here, at the single DB writer, guarantees every stored root is absolute.
/// That matters because git is invoked with `git -C <root>` and no shell, so a
/// literal `~` would never resolve. `~` and `~/...` expand to home; `~user` is
/// left untouched (it is a single component that is not exactly `~`).
fn expand_tilde(root: &Path) -> PathBuf {
    if let Ok(rest) = root.strip_prefix("~") {
        // `strip_prefix("~")` only matches when `~` is its own path component,
        // i.e. the path is `~` or starts with `~/...` — never `~user`.
        if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
            return home.join(rest);
        }
    }
    root.to_path_buf()
}

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
    session_name   TEXT,
    worktree_path  TEXT,
    branch         TEXT,
    auto_reviewed  INTEGER NOT NULL DEFAULT 0,
    instrumented   INTEGER NOT NULL DEFAULT 0,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
);
";

/// A bare SQL identifier (table or column name): non-empty, ASCII letters,
/// digits and underscores only, not starting with a digit. We interpolate such
/// identifiers directly into SQL because SQLite cannot bind identifiers as
/// parameters; this guard keeps that interpolation from ever becoming an
/// injection vector if a future caller forgets the "trusted literal" rule.
fn is_identifier_safe(s: &str) -> bool {
    let mut bytes = s.bytes();
    match bytes.next() {
        Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
        _ => return false,
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Add a column to `table` if it isn't already present. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`, so we check `PRAGMA table_info` first. This keeps
/// databases created by older kamaji versions forward-compatible.
///
/// SECURITY: `table` and `col` are interpolated directly into SQL (SQLite cannot
/// bind identifiers as parameters), and `decl` is appended verbatim. All three
/// MUST be trusted, compile-time literals — never user input or anything derived
/// from it. `table`/`col` are debug-asserted to be bare identifiers as a
/// defence-in-depth backstop; `decl` (e.g. `"INTEGER NOT NULL DEFAULT 0"`) is a
/// declaration fragment and cannot be identifier-checked, so it relies entirely
/// on the trusted-literal contract.
fn add_column_if_missing(conn: &Connection, table: &str, col: &str, decl: &str) -> Result<()> {
    if !column_exists(conn, table, col)? {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl}"), [])?;
    }
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    debug_assert!(is_identifier_safe(table), "untrusted table name: {table:?}");
    debug_assert!(is_identifier_safe(col), "untrusted column name: {col:?}");
    let present = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(std::result::Result::ok)
        .any(|name| name == col);
    Ok(present)
}

fn drop_column_if_present(conn: &Connection, table: &str, col: &str) -> Result<()> {
    if column_exists(conn, table, col)? {
        conn.execute(&format!("ALTER TABLE {table} DROP COLUMN {col}"), [])?;
    }
    Ok(())
}

/// The ordered migration ladder. Each entry transforms the database from the
/// state left by the previous entry to the next. An entry's 1-based position is
/// the `PRAGMA user_version` recorded after it runs, so the ladder is the single
/// source of truth for "what schema version is this database at".
///
/// Rules for adding a migration:
/// - **Append only.** Never reorder, edit, or remove an existing entry — already
///   migrated databases have advanced past it and will never run it again.
/// - Each entry must be idempotent-safe against a database that already has the
///   end-state schema (a fresh DB created from [`SCHEMA`] sits at `user_version`
///   0 and must survive replaying the additive entries below as no-ops).
/// - Additive column adds use [`add_column_if_missing`]. Non-additive changes
///   (rename / backfill / drop) belong here too — that is the whole point of the
///   ladder: a versioned hook to run them exactly once, in order.
const MIGRATIONS: &[fn(&Connection) -> Result<()>] = &[
    // v1: per-session detection flags. Additive; a no-op on databases (including
    // every freshly created one) that already declare these columns.
    |conn| {
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
    },
    // v2: remove the unused ticket ordering column. It was always left at its
    // default value, so list ordering was already effectively id order.
    |conn| {
        drop_column_if_present(conn, "tickets", "position")?;
        Ok(())
    },
];

/// Bring an existing database up to the current schema by running every ladder
/// entry whose version is newer than the database's recorded `user_version`,
/// then stamping the new version. Idempotent: a second call is a no-op.
fn migrate(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, migration) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as i64;
        if version > current {
            migration(conn)?;
            // `user_version` takes no bound parameters; `version` is an i64 we
            // control, so `pragma_update` interpolates it safely.
            conn.pragma_update(None, "user_version", version)?;
        }
    }
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
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Db { conn })
    }

    pub fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
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
                expand_tilde(root_dir).to_string_lossy(),
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

    /// Edit a project's caller-editable fields (full replace): name, root dir,
    /// and default agent. Returns the updated row, or `None` when no project
    /// with `id` exists (zero rows touched).
    pub fn update_project(
        &self,
        id: i64,
        name: &str,
        root_dir: &Path,
        default_agent: Option<Agent>,
    ) -> Result<Option<Project>> {
        self.conn.execute(
            "UPDATE projects SET name = ?2, root_dir = ?3, default_agent = ?4 WHERE id = ?1",
            params![
                id,
                name,
                expand_tilde(root_dir).to_string_lossy(),
                default_agent.map(|a| a.as_str())
            ],
        )?;
        self.get_project(id)
    }

    /// Delete a project together with every ticket that belongs to it, returning
    /// the ids of the removed tickets (in id order) so callers can emit one
    /// `ticket.deleted` per card. Tickets are removed first to satisfy the
    /// `tickets.project_id REFERENCES projects(id)` foreign key. Like
    /// [`Self::delete_done_tickets`], this only removes rows — tearing down any
    /// worktree/zellij session a ticket still owns is the caller's concern
    /// (the daemon's project-delete route does it before calling this).
    pub fn delete_project(&self, id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM tickets WHERE project_id = ?1 ORDER BY id")?;
        let ids = stmt
            .query_map([id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        self.conn
            .execute("DELETE FROM tickets WHERE project_id = ?1", [id])?;
        self.conn
            .execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(ids)
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
            .prepare("SELECT * FROM tickets WHERE project_id = ?1 ORDER BY id")?;
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

    /// Edit all caller-editable ticket fields at once (full replace): title,
    /// description, initial prompt, and agent.
    pub fn update_ticket_full(
        &self,
        id: i64,
        title: &str,
        description: &str,
        initial_prompt: Option<&str>,
        agent: Agent,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE tickets SET title = ?2, description = ?3, initial_prompt = ?4, agent = ?5,
             updated_at = datetime('now') WHERE id = ?1",
            params![id, title, description, initial_prompt, agent.as_str()],
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

    /// Delete every ticket in the Done column for `project_id`, returning the
    /// ids of the rows removed (in id order) so callers can emit one
    /// `ticket.deleted` per card. Like [`Self::delete_ticket`], this only
    /// removes the rows — it does not tear down any worktree/zellij session a
    /// ticket may still own.
    pub fn delete_done_tickets(&self, project_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM tickets WHERE project_id = ?1 AND status = ?2 ORDER BY id")?;
        let ids = stmt
            .query_map(params![project_id, Status::Done.as_str()], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        self.conn.execute(
            "DELETE FROM tickets WHERE project_id = ?1 AND status = ?2",
            params![project_id, Status::Done.as_str()],
        )?;
        Ok(ids)
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
    fn update_project_replaces_fields_and_returns_the_row() {
        let db = db();
        let p = db
            .create_project("acme", &PathBuf::from("/tmp/acme"), Some(Agent::Claude))
            .unwrap();
        let updated = db
            .update_project(p.id, "acme-renamed", &PathBuf::from("/tmp/moved"), None)
            .unwrap()
            .expect("an existing project returns the updated row");
        assert_eq!(updated.name, "acme-renamed");
        assert_eq!(updated.root_dir, PathBuf::from("/tmp/moved"));
        assert_eq!(updated.default_agent, None);
        // The id and created_at are stable across the edit.
        assert_eq!(updated.id, p.id);
        assert_eq!(updated.created_at, p.created_at);
        // The change is persisted, not just returned.
        let reread = db.get_project(p.id).unwrap().unwrap();
        assert_eq!(reread.name, "acme-renamed");
        assert_eq!(reread.default_agent, None);
    }

    #[test]
    fn update_project_on_missing_id_returns_none() {
        let db = db();
        assert!(db
            .update_project(999, "ghost", &PathBuf::from("/tmp/ghost"), None)
            .unwrap()
            .is_none());
    }

    #[test]
    fn create_project_expands_a_leading_tilde_to_home() {
        let db = db();
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .expect("home dir");
        let p = db
            .create_project("homed", &PathBuf::from("~/dev/kamaji"), None)
            .unwrap();
        // The stored root must be absolute — git checks it without a shell, so a
        // literal `~` would never resolve.
        assert_eq!(p.root_dir, home.join("dev/kamaji"));
        let reread = db.get_project(p.id).unwrap().unwrap();
        assert_eq!(reread.root_dir, home.join("dev/kamaji"));
    }

    #[test]
    fn update_project_expands_a_leading_tilde_to_home() {
        let db = db();
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .expect("home dir");
        let p = db
            .create_project("homed", &PathBuf::from("/tmp/abs"), None)
            .unwrap();
        let updated = db
            .update_project(p.id, "homed", &PathBuf::from("~/code"), None)
            .unwrap()
            .expect("existing row");
        assert_eq!(updated.root_dir, home.join("code"));
    }

    #[test]
    fn create_project_leaves_an_absolute_path_untouched() {
        let db = db();
        let p = db
            .create_project("abs", &PathBuf::from("/tmp/acme"), None)
            .unwrap();
        assert_eq!(p.root_dir, PathBuf::from("/tmp/acme"));
    }

    #[test]
    fn delete_project_removes_it_with_its_tickets_and_returns_their_ids() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        // A second project whose tickets must be left completely untouched.
        let other = db
            .create_project("other", &PathBuf::from("/tmp/other"), None)
            .unwrap();
        let other_ticket = db
            .create_ticket(other.id, "keep me", "", None, Agent::Claude)
            .unwrap();

        let a = db
            .create_ticket(p.id, "a", "", None, Agent::Claude)
            .unwrap();
        let b = db
            .create_ticket(p.id, "b", "", None, Agent::Claude)
            .unwrap();

        let deleted = db.delete_project(p.id).unwrap();
        assert_eq!(deleted, vec![a.id, b.id], "returns its ticket ids in order");

        // The project and its tickets are gone; the FK never trips.
        assert!(db.get_project(p.id).unwrap().is_none());
        assert!(db.get_ticket(a.id).unwrap().is_none());
        assert!(db.get_ticket(b.id).unwrap().is_none());

        // The other project and its ticket survive intact.
        assert!(db.get_project(other.id).unwrap().is_some());
        assert!(db.get_ticket(other_ticket.id).unwrap().is_some());
    }

    #[test]
    fn delete_project_with_no_tickets_returns_empty_and_removes_the_row() {
        let db = db();
        let p = db
            .create_project("empty", &PathBuf::from("/tmp/empty"), None)
            .unwrap();
        let deleted = db.delete_project(p.id).unwrap();
        assert!(deleted.is_empty());
        assert!(db.get_project(p.id).unwrap().is_none());
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
    fn delete_done_tickets_removes_only_done_and_returns_their_ids() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        // Other projects' Done tickets must be untouched (scoped by project_id).
        let other = db
            .create_project("other", &PathBuf::from("/tmp/other"), None)
            .unwrap();
        let other_done = db
            .create_ticket(other.id, "other-done", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_status(other_done.id, Status::Done).unwrap();

        let todo = db
            .create_ticket(p.id, "todo", "", None, Agent::Claude)
            .unwrap();
        let done_a = db
            .create_ticket(p.id, "done-a", "", None, Agent::Claude)
            .unwrap();
        let done_b = db
            .create_ticket(p.id, "done-b", "", None, Agent::Claude)
            .unwrap();
        let review = db
            .create_ticket(p.id, "review", "", None, Agent::Claude)
            .unwrap();
        db.set_ticket_status(done_a.id, Status::Done).unwrap();
        db.set_ticket_status(done_b.id, Status::Done).unwrap();
        db.set_ticket_status(review.id, Status::Review).unwrap();

        let deleted = db.delete_done_tickets(p.id).unwrap();
        assert_eq!(deleted, vec![done_a.id, done_b.id]);

        // Only the non-Done tickets of this project survive.
        let remaining: Vec<i64> = db
            .list_tickets(p.id)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(remaining, vec![todo.id, review.id]);

        // The other project's Done ticket is left intact.
        assert!(db.get_ticket(other_done.id).unwrap().is_some());
    }

    #[test]
    fn delete_done_tickets_on_empty_done_column_is_a_noop() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        db.create_ticket(p.id, "todo", "", None, Agent::Claude)
            .unwrap();
        let deleted = db.delete_done_tickets(p.id).unwrap();
        assert!(deleted.is_empty());
        assert_eq!(db.list_tickets(p.id).unwrap().len(), 1);
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
    fn update_ticket_full_replaces_all_fields() {
        let db = db();
        let p = db
            .create_project("p", &PathBuf::from("/tmp/p"), None)
            .unwrap();
        let t = db
            .create_ticket(p.id, "t", "d", Some("p1"), Agent::Claude)
            .unwrap();
        db.update_ticket_full(t.id, "t2", "d2", Some("p2"), Agent::Codex)
            .unwrap();
        let got = db.get_ticket(t.id).unwrap().unwrap();
        assert_eq!(got.title, "t2");
        assert_eq!(got.description, "d2");
        assert_eq!(got.initial_prompt.as_deref(), Some("p2"));
        assert_eq!(got.agent, Agent::Codex);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = db();
        // Inserting a ticket under a non-existent project must fail (FK ON).
        let err = db.create_ticket(9999, "t", "", None, Agent::Claude);
        assert!(
            err.is_err(),
            "FK enforcement should reject a bad project_id"
        );
    }

    #[test]
    fn migrate_drops_position_adds_missing_columns_and_is_idempotent() {
        // A pre-migration tickets table with the obsolete position column and
        // no auto_reviewed / instrumented columns.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tickets (
                id INTEGER PRIMARY KEY, project_id INTEGER, title TEXT, description TEXT,
                initial_prompt TEXT, agent TEXT, status TEXT, position INTEGER,
                session_name TEXT, worktree_path TEXT, branch TEXT,
                created_at TEXT, updated_at TEXT);",
        )
        .unwrap();
        // Legacy databases sit at user_version 0 with the columns missing.
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 0);

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
        assert!(!cols.contains(&"position".to_string()));
    }

    #[test]
    fn new_schema_has_no_ticket_position_column() {
        let db = db();
        let cols: Vec<String> = db
            .conn
            .prepare("PRAGMA table_info(tickets)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();
        assert!(!cols.contains(&"position".to_string()));
    }

    #[test]
    fn migrate_stamps_user_version_to_latest() {
        // A freshly opened database must be stamped at the top of the ladder, so
        // already-run migrations are skipped on the next open.
        let db = db();
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn migrate_skips_entries_at_or_below_current_version() {
        // A database already stamped at the top of the ladder runs no entries
        // and keeps its version — the ladder only moves forward.
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", MIGRATIONS.len() as i64)
            .unwrap();
        migrate(&conn).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
    }

    #[test]
    fn identifier_safety_guard() {
        assert!(is_identifier_safe("tickets"));
        assert!(is_identifier_safe("auto_reviewed"));
        assert!(is_identifier_safe("_x9"));
        assert!(!is_identifier_safe(""));
        assert!(!is_identifier_safe("9col"));
        assert!(!is_identifier_safe("a b"));
        assert!(!is_identifier_safe("a;DROP TABLE tickets"));
        assert!(!is_identifier_safe("col)--"));
    }
}
