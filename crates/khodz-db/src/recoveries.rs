use rusqlite::{OptionalExtension, Row, params};

use crate::{
    Database, DbError, DbResult, NewRecoveryItem, Page, Recovery, RecoveryItemRow, RecoveryStatus,
    RecoveryTotals, now_ms,
};

const COLUMNS: &str = "id, scan_id, destination, status, error, started_at, finished_at,
     files_total, files_verified, files_failed, bytes_written, files_partial, files_mismatch, same_disk_override";

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
        files_partial: u64_col(r, 11)?,
        files_mismatch: u64_col(r, 12)?,
        same_disk_override: r.get(13)?,
    })
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl Database {
    pub fn create_recovery(
        &self,
        scan_id: Option<i64>,
        destination: &str,
        same_disk_override: bool,
    ) -> DbResult<Recovery> {
        self.conn.execute(
            "INSERT INTO recoveries (scan_id, destination, status, started_at, same_disk_override)
             VALUES (?1, ?2, 'running', ?3, ?4)",
            params![scan_id, destination, now_ms(), same_disk_override],
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
                 files_verified = ?6, files_failed = ?7, bytes_written = ?8, files_partial = ?9, files_mismatch = ?10
             WHERE id = ?1 AND status = 'running'",
            params![
                id,
                status,
                error,
                now_ms(),
                to_i64(totals.files_total),
                to_i64(totals.files_verified),
                to_i64(totals.files_failed),
                to_i64(totals.bytes_written),
                to_i64(totals.files_partial),
                to_i64(totals.files_mismatch)
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

    pub fn insert_recovery_items(&mut self, recovery_id: i64, items: &[NewRecoveryItem]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO recovery_items (recovery_id, scan_item_id, output_path, size, sha256, outcome, error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for it in items {
                stmt.execute(params![
                    recovery_id,
                    it.scan_item_id,
                    it.output_path,
                    to_i64(it.size),
                    it.sha256,
                    it.outcome,
                    it.error
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Items of a recovery, problems first (failed, mismatch, partial, verified), then by path.
    pub fn list_recovery_items(&self, recovery_id: i64, page: Page) -> DbResult<Vec<RecoveryItemRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.scan_item_id, s.path, r.output_path, r.size, r.sha256, r.outcome, r.error
             FROM recovery_items r LEFT JOIN scan_items s ON s.id = r.scan_item_id
             WHERE r.recovery_id = ?1
             ORDER BY CASE r.outcome WHEN 'failed' THEN 0 WHEN 'mismatch' THEN 1 WHEN 'partial_source' THEN 2 ELSE 3 END,
                      r.output_path COLLATE NOCASE
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![recovery_id, page.clamped_limit(), page.offset], |r| {
            Ok(RecoveryItemRow {
                id: r.get(0)?,
                scan_item_id: r.get(1)?,
                source_path: r.get(2)?,
                output_path: r.get(3)?,
                size: u64_col(r, 4)?,
                sha256: r.get(5)?,
                outcome: r.get(6)?,
                error: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewScan, ScanMode, SourceKind};

    fn scan(db: &Database) -> i64 {
        db.create_scan(&NewScan {
            source_kind: SourceKind::Image,
            source_path: "x.img".into(),
            source_label: None,
            source_size: 10,
            scan_mode: ScanMode::Deep,
        })
        .unwrap()
        .id
    }

    #[test]
    fn recovery_lifecycle() {
        let db = Database::open_in_memory().unwrap();
        let s = scan(&db);
        let r = db.create_recovery(Some(s), "D:\\Recovered\\Khodz Recovery 1", true).unwrap();
        assert_eq!(r.status, RecoveryStatus::Running);
        assert!(r.same_disk_override);

        let totals = RecoveryTotals {
            files_total: 5,
            files_verified: 2,
            files_failed: 1,
            bytes_written: 2048,
            files_partial: 1,
            files_mismatch: 1,
        };
        let done = db.finish_recovery(r.id, RecoveryStatus::Completed, totals, None).unwrap();
        assert_eq!((done.files_verified, done.files_partial, done.files_mismatch), (2, 1, 1));
        assert_eq!(done.bytes_written, 2048);
        assert!(db.finish_recovery(r.id, RecoveryStatus::Failed, totals, None).is_err());
        assert_eq!(db.list_recoveries(Page::default()).unwrap().len(), 1);
    }

    #[test]
    fn recovery_items_listed_problems_first() {
        let mut db = Database::open_in_memory().unwrap();
        let r = db.create_recovery(None, "E:\\out", false).unwrap();
        let mk = |p: &str, o: &str| NewRecoveryItem {
            scan_item_id: None,
            output_path: p.into(),
            size: 1,
            sha256: Some("ab".into()),
            outcome: o.into(),
            error: None,
        };
        db.insert_recovery_items(r.id, &[mk("a", "verified"), mk("b", "failed"), mk("c", "partial_source")])
            .unwrap();
        let rows = db.list_recovery_items(r.id, Page::default()).unwrap();
        assert_eq!(
            rows.iter().map(|x| x.outcome.as_str()).collect::<Vec<_>>(),
            vec!["failed", "partial_source", "verified"]
        );
        let mut bad = mk("d", "great");
        bad.output_path = "d".into();
        assert!(db.insert_recovery_items(r.id, &[bad]).is_err(), "CHECK constraint on outcome");
    }

    #[test]
    fn deleting_a_scan_keeps_recovery_history() {
        let db = Database::open_in_memory().unwrap();
        let s = scan(&db);
        db.finish_scan(s, crate::ScanStatus::Completed, None).unwrap();
        let r = db.create_recovery(Some(s), "E:\\out", false).unwrap();
        db.delete_scan(s).unwrap();
        assert_eq!(db.get_recovery(r.id).unwrap().scan_id, None);
    }

    #[test]
    fn interrupted_recoveries_fail_on_startup() {
        let db = Database::open_in_memory().unwrap();
        let r = db.create_recovery(None, "E:\\out", false).unwrap();
        assert_eq!(db.fail_interrupted_recoveries().unwrap(), 1);
        assert_eq!(db.get_recovery(r.id).unwrap().status, RecoveryStatus::Failed);
    }
}
