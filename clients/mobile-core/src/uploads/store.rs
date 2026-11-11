//! The queue of picked files, kept between runs of the app.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS queue (
        position INTEGER PRIMARY KEY AUTOINCREMENT,
        key      TEXT NOT NULL UNIQUE,
        parent   TEXT,                  -- server folder; NULL for Home
        folder   TEXT NOT NULL,         -- that folder's name, for the row
        name     TEXT NOT NULL,
        source   TEXT,                  -- the platform's way to the original, until it is copied
        ready    INTEGER NOT NULL,      -- the copy is complete
        size     INTEGER NOT NULL,
        done     INTEGER NOT NULL,      -- bytes the server had when last heard
        session  TEXT,                  -- the server's upload session
        problem  TEXT,                  -- why it stopped, for the user; it waits for a retry
        paused   INTEGER NOT NULL DEFAULT 0, -- it waits for the user to continue
        mime     TEXT                   -- the type the app gave; NULL for the server to guess
    );
";
const COLUMNS: &str =
    "key, parent, folder, name, source, ready, size, done, session, problem, paused, mime";

pub(super) type Result<T> = rusqlite::Result<T>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Row {
    pub key: String,
    pub parent: Option<String>,
    pub folder: String,
    pub name: String,
    pub source: Option<String>,
    pub ready: bool,
    pub size: u64,
    pub done: u64,
    pub session: Option<Uuid>,
    pub problem: Option<String>,
    pub paused: bool,
    pub mime: Option<String>,
}

fn row_of(row: &rusqlite::Row<'_>) -> Result<Row> {
    let session: Option<String> = row.get(8)?;
    Ok(Row {
        key: row.get(0)?,
        parent: row.get(1)?,
        folder: row.get(2)?,
        name: row.get(3)?,
        source: row.get(4)?,
        ready: row.get(5)?,
        size: row.get::<_, i64>(6)?.cast_unsigned(),
        done: row.get::<_, i64>(7)?.cast_unsigned(),
        // One that can't be read is one the server can't be asked about, so it starts anew.
        session: session.and_then(|id| Uuid::parse_str(&id).ok()),
        problem: row.get(9)?,
        paused: row.get(10)?,
        mime: row.get(11)?,
    })
}

fn has_column(db: &Connection, name: &str) -> Result<bool> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('queue') WHERE name = ?1)",
        [name],
        |row| row.get(0),
    )
}

pub(super) struct Store {
    db: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("PRAGMA journal_mode = WAL;")?;
        db.execute_batch(SCHEMA)?;
        // A queue from before pausing keeps its files.
        if !has_column(&db, "paused")? {
            db.execute_batch("ALTER TABLE queue ADD COLUMN paused INTEGER NOT NULL DEFAULT 0")?;
        }
        // A queue from before types were kept leaves them to the server, as it did.
        if !has_column(&db, "mime")? {
            db.execute_batch("ALTER TABLE queue ADD COLUMN mime TEXT")?;
        }
        Ok(Self { db: Mutex::new(db) })
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn add(
        &self,
        key: &str,
        parent: Option<&str>,
        folder: &str,
        name: &str,
        source: Option<&str>,
        mime: Option<&str>,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO queue (key, parent, folder, name, source, ready, size, done, mime)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, 0, ?6)",
            params![key, parent, folder, name, source, mime],
        )?;
        Ok(())
    }

    /// Everything in the order it goes up.
    pub fn rows(&self) -> Result<Vec<Row>> {
        let db = self.db();
        let mut statement =
            db.prepare(&format!("SELECT {COLUMNS} FROM queue ORDER BY position"))?;
        let rows = statement.query_map([], row_of)?;
        rows.collect()
    }

    /// The next one that can go: not paused or stopped by a problem, and with a copy or a way to
    /// make one.
    pub fn next(&self) -> Result<Option<Row>> {
        self.db()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM queue
                     WHERE problem IS NULL AND paused = 0 AND (ready = 1 OR source IS NOT NULL)
                     ORDER BY position LIMIT 1"
                ),
                [],
                row_of,
            )
            .optional()
    }

    /// The copy is complete. False if the row has gone meanwhile.
    pub fn mark_ready(&self, key: &str, name: Option<&str>, size: u64) -> Result<bool> {
        let changed = self.db().execute(
            "UPDATE queue SET ready = 1, source = NULL, size = ?2, name = COALESCE(?3, name)
             WHERE key = ?1",
            params![key, size.cast_signed(), name],
        )?;
        Ok(changed > 0)
    }

    /// False if the row has gone meanwhile.
    pub fn set_session(&self, key: &str, session: Uuid, done: u64) -> Result<bool> {
        let changed = self.db().execute(
            "UPDATE queue SET session = ?2, done = ?3 WHERE key = ?1",
            params![key, session.to_string(), done.cast_signed()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_done(&self, key: &str, done: u64) -> Result<()> {
        self.db().execute(
            "UPDATE queue SET done = ?2 WHERE key = ?1",
            params![key, done.cast_signed()],
        )?;
        Ok(())
    }

    pub fn set_problem(&self, key: &str, problem: Option<&str>) -> Result<()> {
        self.db().execute(
            "UPDATE queue SET problem = ?2 WHERE key = ?1",
            params![key, problem],
        )?;
        Ok(())
    }

    /// Everything that could still go waits for the user.
    pub fn pause_all(&self) -> Result<()> {
        self.db()
            .execute("UPDATE queue SET paused = 1 WHERE problem IS NULL", [])?;
        Ok(())
    }

    /// It can go again, whether it was paused or stopped by a problem.
    pub fn resume(&self, key: &str) -> Result<()> {
        self.db().execute(
            "UPDATE queue SET problem = NULL, paused = 0 WHERE key = ?1",
            [key],
        )?;
        Ok(())
    }

    /// The copy has gone and the original can't be reached. Retrying can't help.
    pub fn lost(&self, key: &str, problem: &str) -> Result<()> {
        self.db().execute(
            "UPDATE queue SET ready = 0, source = NULL, problem = ?2 WHERE key = ?1",
            params![key, problem],
        )?;
        Ok(())
    }

    /// Rows whose copy the app was still making when it ended, with no way back to the original.
    pub fn lose_unprepared(&self, problem: &str) -> Result<()> {
        self.db().execute(
            "UPDATE queue SET problem = ?1
             WHERE ready = 0 AND source IS NULL AND problem IS NULL",
            [problem],
        )?;
        Ok(())
    }

    pub fn remove(&self, key: &str) -> Result<Option<Row>> {
        let db = self.db();
        let row = db
            .query_row(
                &format!("SELECT {COLUMNS} FROM queue WHERE key = ?1"),
                [key],
                row_of,
            )
            .optional()?;
        db.execute("DELETE FROM queue WHERE key = ?1", [key])?;
        Ok(row)
    }
}
