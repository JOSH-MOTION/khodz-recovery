use std::fmt;

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    /// A row was not found.
    NotFound(&'static str, i64),
    /// An operation was attempted from an invalid state (e.g. finishing a scan twice).
    InvalidState(String),
    /// The database was written by a newer version of the app.
    SchemaTooNew {
        found: u32,
        supported: u32,
    },
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Sqlite(e) => write!(f, "database error: {e}"),
            DbError::NotFound(what, id) => write!(f, "{what} {id} not found"),
            DbError::InvalidState(m) => write!(f, "invalid state: {m}"),
            DbError::SchemaTooNew { found, supported } => write!(
                f,
                "database schema v{found} is newer than this app supports (v{supported}); update Khodz Recovery"
            ),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

pub type DbResult<T> = Result<T, DbError>;
