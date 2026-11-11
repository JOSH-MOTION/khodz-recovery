use rusqlite::{OptionalExtension, params};

use crate::{Database, DbError, DbResult};

/// Keys the app is allowed to persist. Rejecting unknown keys stops the UI layer from turning
/// the settings table into an arbitrary key/value store.
pub const ALLOWED_KEYS: &[&str] = &["theme", "allow_same_disk_recovery", "last_destination"];

const MAX_VALUE_LEN: usize = 4096;

impl Database {
    pub fn get_setting(&self, key: &str) -> DbResult<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> DbResult<()> {
        if !ALLOWED_KEYS.contains(&key) {
            return Err(DbError::InvalidState(format!("unknown setting '{key}'")));
        }
        if value.len() > MAX_VALUE_LEN {
            return Err(DbError::InvalidState(format!("setting '{key}' value too long")));
        }
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_overwrite() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_setting("theme").unwrap(), None);
        db.set_setting("theme", "dark").unwrap();
        db.set_setting("theme", "light").unwrap();
        assert_eq!(db.get_setting("theme").unwrap().as_deref(), Some("light"));
    }

    #[test]
    fn rejects_unknown_keys_and_oversized_values() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.set_setting("arbitrary", "x").is_err());
        assert!(db.set_setting("theme", &"x".repeat(MAX_VALUE_LEN + 1)).is_err());
    }
}
