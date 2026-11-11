//! SQLite persistence for Khodz Recovery.
//!
//! The database stores scan history, scan findings (paged — never loaded wholesale), recovery
//! history and user settings. It never stores file *contents* from scanned media.

mod error;
mod migrations;
pub mod models;
mod recoveries;
mod scans;
mod settings;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

pub use error::{DbError, DbResult};
pub use models::*;

/// Handle to the application database. Not `Sync`; wrap in a `Mutex` to share.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (creating if needed) a database file and bring its schema up to date.
    pub fn open(path: impl AsRef<Path>) -> DbResult<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    /// In-memory database, used by tests.
    pub fn open_in_memory() -> DbResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> DbResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrations::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    /// Current schema version (SQLite `user_version`).
    pub fn schema_version(&self) -> DbResult<u32> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }
}

/// Milliseconds since the Unix epoch.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_database_persists_and_reopens_without_remigrating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("khodz.db");
        {
            let db = Database::open(&path).unwrap();
            db.set_setting("theme", "light").unwrap();
            assert_eq!(db.schema_version().unwrap(), migrations::LATEST);
        }
        let db = Database::open(&path).unwrap();
        assert_eq!(db.get_setting("theme").unwrap().as_deref(), Some("light"));
        assert_eq!(db.schema_version().unwrap(), migrations::LATEST);
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = Database::open_in_memory().unwrap();
        let fk: i64 = db.conn.pragma_query_value(None, "foreign_keys", |r| r.get(0)).unwrap();
        assert_eq!(fk, 1);
    }
}
