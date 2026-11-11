use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use khodz_db::Database;

use crate::commands::CommandError;

pub struct AppState {
    pub db: Mutex<Database>,
    pub db_path: PathBuf,
}

impl AppState {
    pub fn db(&self) -> Result<MutexGuard<'_, Database>, CommandError> {
        self.db.lock().map_err(|_| CommandError::internal("database lock poisoned"))
    }
}
