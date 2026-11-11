use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use khodz_core::CancelToken;
use khodz_db::Database;

use crate::commands::CommandError;

pub struct AppState {
    pub db: Mutex<Database>,
    pub db_path: PathBuf,
    /// Cancellation tokens of running background jobs, keyed by a frontend-chosen id.
    pub jobs: Mutex<HashMap<String, CancelToken>>,
}

impl AppState {
    pub fn new(db: Database, db_path: PathBuf) -> Self {
        Self { db: Mutex::new(db), db_path, jobs: Mutex::new(HashMap::new()) }
    }

    pub fn db(&self) -> Result<MutexGuard<'_, Database>, CommandError> {
        self.db.lock().map_err(|_| CommandError::internal("database lock poisoned"))
    }

    fn jobs(&self) -> Result<MutexGuard<'_, HashMap<String, CancelToken>>, CommandError> {
        self.jobs.lock().map_err(|_| CommandError::internal("job table lock poisoned"))
    }

    pub fn register_job(&self, id: &str) -> Result<CancelToken, CommandError> {
        if id.is_empty() || id.len() > 64 {
            return Err(CommandError { kind: "invalid_request", message: "invalid job id".into() });
        }
        let mut jobs = self.jobs()?;
        if jobs.contains_key(id) {
            return Err(CommandError {
                kind: "invalid_request",
                message: format!("job {id} already running"),
            });
        }
        let token = CancelToken::new();
        jobs.insert(id.to_string(), token.clone());
        Ok(token)
    }

    pub fn finish_job(&self, id: &str) {
        if let Ok(mut jobs) = self.jobs() {
            jobs.remove(id);
        }
    }

    /// Returns false if no such job is running.
    pub fn cancel_job(&self, id: &str) -> Result<bool, CommandError> {
        Ok(self.jobs()?.get(id).map(|t| t.cancel()).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_lifecycle() {
        let s = AppState::new(Database::open_in_memory().unwrap(), "m".into());
        let t = s.register_job("a").unwrap();
        assert!(s.register_job("a").is_err(), "duplicate ids rejected");
        assert!(s.register_job("").is_err());
        assert!(s.cancel_job("a").unwrap());
        assert!(t.is_cancelled());
        s.finish_job("a");
        assert!(!s.cancel_job("a").unwrap());
    }
}
