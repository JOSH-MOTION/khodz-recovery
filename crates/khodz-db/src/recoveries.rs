use rusqlite::{OptionalExtension, Row, params};

use crate::{Database, DbError, DbResult, Page, Recovery, RecoveryStatus, RecoveryTotals, now_ms};

const COLUMNS: &str = "id, scan_id, destination, status, error, started_at, finished_at,
     files_total, files_verified, files_failed, bytes_written";

fn u64_col(r: &Row<'_>, i: usize) -> rusqlite::Result<u64> {
    Ok(r.get::<_, i64>(i)?.max(0) as u64)
}

fn recovery_from_row(r: &Row<'_>) -> rusqlite::Result<Recovery> {
    Ok(Recovery {
        id: r.get(0)?,
        scan_id: r.get(1)?,
        destination: r.get(2)?,
        status: r.get(3)?,
        error: r.get(4)?,
        started_at: r.get(5)?,
        finished_at: r.get(6)?,
        files_total: u64_col(r, 7)?,
        files_verified: u64_col(r, 8)?,
        files_failed: u64_col(r, 9)?,
        bytes_written: u64_col(r, 10)?,
    })
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl Database {
    pub fn create_recovery(&self, scan_id: Option<i64>, destination: &str) -> DbResult<Recovery> {
        self.conn.execute(
            "INSERT INTO recoveries (scan_id, destination, status, started_at) VALUES (?1, ?2, 'running', ?3)",
            params![scan_id, destination, now_ms()],
        )?;
        self.get_recovery(self.conn.last_insert_rowid())
    }

    pub fn get_recovery(&self, id: i64) -> DbResult<Recovery> {
        self.conn
            .query_row(&format!("SELECT {COLUMNS} FROM recoveries WHERE id = ?1"), [id], recovery_from_row)
            .optional()?
            .ok_or(DbError::NotFound("recovery", id))
    }

    pub fn list_recoveries(&self, page: Page) -> DbResult<Vec<Recovery>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM recoveries ORDER BY started_at DESC, id DESC LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = stmt.query_map(params![page.clamped_limit(), page.offset], recovery_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn finish_recovery(
        &self,
        id: i64,
        status: RecoveryStatus,
        totals: RecoveryTotals,
        error: Option<&str>,
    ) -> DbResult<Recovery> {
        if status == RecoveryStatus::Running {
            return Err(DbError::InvalidState("cannot finish a recovery as 'running'".into()));
        }
        let n = self.conn.execute(
            "UPDATE recoveries SET status = ?2, error = ?3, finished_at = ?4, files_total = ?5,
                 files_verified = ?6, files_failed = ?7, bytes_written = ?8
             WHERE id = ?1 AND status = 'running'",
            params![
                id,
                status,
                error,
                now_ms(),
                to_i64(totals.files_total),
                to_i64(totals.files_verified),
                to_i64(totals.files_failed),
                to_i64(totals.bytes_written)
            ],
        )?;
        if n == 0 {
            let existing = self.get_recovery(id)?;
            return Err(DbError::InvalidState(format!(
                "recovery {id} is {} not running",
                existing.status.as_str()
            )));
        }
        self.get_recovery(id)
    }

    pub fn fail_interrupted_recoveries(&self) -> DbResult<usize> {
        Ok(self.conn.execute(
            "UPDATE recoveries SET status = 'failed', error = 'interrupted: application closed during recovery',
                 finished_at = ?1 WHERE status = 'running'",
            [now_ms()],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewScan, ScanMode, SourceKind};

    #[test]
    fn recovery_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        let scan = db
            .create_scan(&NewScan {
                source_kind: SourceKind::Image,
                source_path: "x.img".into(),
                source_label: None,
                source_size: 10,
                scan_mode: ScanMode::Deep,
            })
            .unwrap();
        let r = db.create_recovery(Some(scan.id), "D:\\Recovered").unwrap();
        assert_eq!(r.status, RecoveryStatus::Running);

        let totals =
            RecoveryTotals { files_total: 3, files_verified: 2, files_failed: 1, bytes_written: 2048 };
        let done = db.finish_recovery(r.id, RecoveryStatus::Completed, totals, None).unwrap();
        assert_eq!(done.files_verified, 2);
        assert_eq!(done.bytes_written, 2048);
        assert!(db.finish_recovery(r.id, RecoveryStatus::Failed, totals, None).is_err());

        assert_eq!(db.list_recoveries(Page::default()).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_scan_keeps_recovery_history() {
        let db = Database::open_in_memory().unwrap();
        let scan = db
            .create_scan(&NewScan {
                source_kind: SourceKind::Image,
                source_path: "x.img".into(),
                source_label: None,
                source_size: 10,
                scan_mode: ScanMode::Quick,
            })
            .unwrap();
        db.finish_scan(scan.id, crate::ScanStatus::Completed, None).unwrap();
        let r = db.create_recovery(Some(scan.id), "E:\\out").unwrap();
        db.delete_scan(scan.id).unwrap();
        assert_eq!(db.get_recovery(r.id).unwrap().scan_id, None);
    }

    #[test]
    fn interrupted_recoveries_fail_on_startup() {
        let db = Database::open_in_memory().unwrap();
        let r = db.create_recovery(None, "E:\\out").unwrap();
        assert_eq!(db.fail_interrupted_recoveries().unwrap(), 1);
        assert_eq!(db.get_recovery(r.id).unwrap().status, RecoveryStatus::Failed);
    }
}
