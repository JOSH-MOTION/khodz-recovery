use rusqlite::{OptionalExtension, Row, params};

use crate::{Database, DbError, DbResult, NewScan, Page, Scan, ScanStatus, now_ms};

const SCAN_COLUMNS: &str = "id, source_kind, source_path, source_label, source_size, scan_mode,
     status, error, started_at, finished_at, items_found";

fn scan_from_row(r: &Row<'_>) -> rusqlite::Result<Scan> {
    Ok(Scan {
        id: r.get(0)?,
        source_kind: r.get(1)?,
        source_path: r.get(2)?,
        source_label: r.get(3)?,
        source_size: r.get::<_, i64>(4)?.max(0) as u64,
        scan_mode: r.get(5)?,
        status: r.get(6)?,
        error: r.get(7)?,
        started_at: r.get(8)?,
        finished_at: r.get(9)?,
        items_found: r.get::<_, i64>(10)?.max(0) as u64,
    })
}

impl Database {
    pub fn create_scan(&self, new: &NewScan) -> DbResult<Scan> {
        let size = i64::try_from(new.source_size)
            .map_err(|_| DbError::InvalidState("source size exceeds i64".into()))?;
        self.conn.execute(
            "INSERT INTO scans (source_kind, source_path, source_label, source_size, scan_mode, status, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6)",
            params![new.source_kind, new.source_path, new.source_label, size, new.scan_mode, now_ms()],
        )?;
        self.get_scan(self.conn.last_insert_rowid())
    }

    pub fn get_scan(&self, id: i64) -> DbResult<Scan> {
        self.conn
            .query_row(&format!("SELECT {SCAN_COLUMNS} FROM scans WHERE id = ?1"), [id], scan_from_row)
            .optional()?
            .ok_or(DbError::NotFound("scan", id))
    }

    /// Newest first.
    pub fn list_scans(&self, page: Page) -> DbResult<Vec<Scan>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SCAN_COLUMNS} FROM scans ORDER BY started_at DESC, id DESC LIMIT ?1 OFFSET ?2"
        ))?;
        let rows = stmt.query_map(params![page.clamped_limit(), page.offset], scan_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn update_scan_items_found(&self, id: i64, items_found: u64) -> DbResult<()> {
        let n = self.conn.execute(
            "UPDATE scans SET items_found = ?2 WHERE id = ?1 AND status = 'running'",
            params![id, i64::try_from(items_found).unwrap_or(i64::MAX)],
        )?;
        if n == 0 {
            return Err(self.not_running(id));
        }
        Ok(())
    }

    /// Move a running scan to a terminal state. Fails if the scan is not running, so a scan can
    /// only ever end once.
    pub fn finish_scan(&self, id: i64, status: ScanStatus, error: Option<&str>) -> DbResult<Scan> {
        if status == ScanStatus::Running {
            return Err(DbError::InvalidState("cannot finish a scan as 'running'".into()));
        }
        let n = self.conn.execute(
            "UPDATE scans SET status = ?2, error = ?3, finished_at = ?4 WHERE id = ?1 AND status = 'running'",
            params![id, status, error, now_ms()],
        )?;
        if n == 0 {
            return Err(self.not_running(id));
        }
        self.get_scan(id)
    }

    /// Called at startup: any scan still `running` belongs to a previous process that exited
    /// without finishing it. Returns the number of scans marked failed.
    pub fn fail_interrupted_scans(&self) -> DbResult<usize> {
        Ok(self.conn.execute(
            "UPDATE scans SET status = 'failed', error = 'interrupted: application closed during scan',
                 finished_at = ?1 WHERE status = 'running'",
            [now_ms()],
        )?)
    }

    pub fn delete_scan(&self, id: i64) -> DbResult<()> {
        let n = self.conn.execute("DELETE FROM scans WHERE id = ?1 AND status != 'running'", [id])?;
        if n == 0 {
            self.get_scan(id)?;
            return Err(DbError::InvalidState(format!("scan {id} is running and cannot be deleted")));
        }
        Ok(())
    }

    fn not_running(&self, id: i64) -> DbError {
        match self.get_scan(id) {
            Ok(s) => DbError::InvalidState(format!("scan {id} is {} not running", s.status.as_str())),
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScanMode, SourceKind};

    fn new_scan(path: &str) -> NewScan {
        NewScan {
            source_kind: SourceKind::Image,
            source_path: path.into(),
            source_label: Some("Test image".into()),
            source_size: 1 << 30,
            scan_mode: ScanMode::Quick,
        }
    }

    #[test]
    fn create_list_and_finish_scan() {
        let db = Database::open_in_memory().unwrap();
        let a = db.create_scan(&new_scan("a.img")).unwrap();
        let b = db.create_scan(&new_scan("b.img")).unwrap();
        assert_eq!(a.status, ScanStatus::Running);
        assert_eq!(a.source_size, 1 << 30);

        db.update_scan_items_found(a.id, 42).unwrap();
        let done = db.finish_scan(a.id, ScanStatus::Completed, None).unwrap();
        assert_eq!(done.status, ScanStatus::Completed);
        assert_eq!(done.items_found, 42);
        assert!(done.finished_at.is_some());

        let list = db.list_scans(Page::default()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, b.id, "newest first");
    }

    #[test]
    fn a_scan_can_only_finish_once() {
        let db = Database::open_in_memory().unwrap();
        let s = db.create_scan(&new_scan("a.img")).unwrap();
        db.finish_scan(s.id, ScanStatus::Cancelled, None).unwrap();
        assert!(matches!(db.finish_scan(s.id, ScanStatus::Completed, None), Err(DbError::InvalidState(_))));
        assert!(db.update_scan_items_found(s.id, 1).is_err());
        assert_eq!(db.get_scan(s.id).unwrap().status, ScanStatus::Cancelled);
    }

    #[test]
    fn cannot_finish_as_running_or_missing() {
        let db = Database::open_in_memory().unwrap();
        let s = db.create_scan(&new_scan("a.img")).unwrap();
        assert!(db.finish_scan(s.id, ScanStatus::Running, None).is_err());
        assert!(matches!(
            db.finish_scan(999, ScanStatus::Completed, None),
            Err(DbError::NotFound("scan", 999))
        ));
    }

    #[test]
    fn interrupted_scans_are_marked_failed_on_startup() {
        let db = Database::open_in_memory().unwrap();
        let running = db.create_scan(&new_scan("a.img")).unwrap();
        let done = db.create_scan(&new_scan("b.img")).unwrap();
        db.finish_scan(done.id, ScanStatus::Completed, None).unwrap();

        assert_eq!(db.fail_interrupted_scans().unwrap(), 1);
        let s = db.get_scan(running.id).unwrap();
        assert_eq!(s.status, ScanStatus::Failed);
        assert!(s.error.unwrap().contains("interrupted"));
        assert_eq!(db.get_scan(done.id).unwrap().status, ScanStatus::Completed);
    }

    #[test]
    fn running_scans_cannot_be_deleted() {
        let db = Database::open_in_memory().unwrap();
        let s = db.create_scan(&new_scan("a.img")).unwrap();
        assert!(db.delete_scan(s.id).is_err());
        db.finish_scan(s.id, ScanStatus::Failed, Some("boom")).unwrap();
        db.delete_scan(s.id).unwrap();
        assert!(matches!(db.get_scan(s.id), Err(DbError::NotFound(..))));
    }

    #[test]
    fn paging_is_bounded() {
        let db = Database::open_in_memory().unwrap();
        for i in 0..5 {
            db.create_scan(&new_scan(&format!("{i}.img"))).unwrap();
        }
        assert_eq!(db.list_scans(Page { limit: 2, offset: 0 }).unwrap().len(), 2);
        assert_eq!(db.list_scans(Page { limit: 2, offset: 4 }).unwrap().len(), 1);
    }
}
