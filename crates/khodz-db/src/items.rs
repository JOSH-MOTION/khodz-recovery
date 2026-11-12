//! Scan items: bulk insert in transactions, filtered paged listing, counts.

use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::{Database, DbError, DbResult, Page};

/// One extent: absolute offset (None = sparse) and length.
pub type ItemExtent = (Option<u64>, u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewScanItem {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub is_deleted: bool,
    pub is_system: bool,
    pub name_complete: bool,
    /// `metadata` or `carved`.
    pub origin: String,
    /// `exact`, `contiguity_assumed`, `partial` or `unknown`.
    pub extent_quality: String,
    pub filesystem: Option<String>,
    pub file_type: Option<String>,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub volume_offset: u64,
    pub extents: Vec<ItemExtent>,
    /// `present`, `erased` or `unknown`.
    pub data_state: String,
    /// `complete`, `unverified`, `partial` for carved items.
    pub carve_status: Option<String>,
    pub note: Option<String>,
    pub resident: Option<Vec<u8>>,
    /// Deleted files: share (0-100) of their assumed location other files now occupy.
    pub overwritten_pct: Option<u8>,
    /// Deleted files of checkable formats: `verified` or `damaged`.
    pub content_check: Option<String>,
}

/// Listing row (no extents / resident data: fetch [`ScanItemDetail`] for those).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanItemRow {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub is_deleted: bool,
    pub name_complete: bool,
    pub origin: String,
    pub extent_quality: String,
    pub filesystem: Option<String>,
    pub file_type: Option<String>,
    pub modified_at: Option<i64>,
    pub data_state: String,
    pub carve_status: Option<String>,
    pub note: Option<String>,
    pub overwritten_pct: Option<u8>,
    pub content_check: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanItemDetail {
    pub scan_id: i64,
    #[serde(flatten)]
    pub row: ScanItemRow,
    pub volume_offset: u64,
    pub extents: Vec<ItemExtent>,
    pub resident: Option<Vec<u8>>,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemView {
    /// Files (not folders) a user would want back: live + deleted + carved, no system files.
    #[default]
    AllFiles,
    Deleted,
    Carved,
    /// Everything, including folders and filesystem metadata files.
    Everything,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFilter {
    #[serde(default)]
    pub view: ItemView,
    /// Case-insensitive substring of the path.
    #[serde(default)]
    pub search: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ItemStats {
    pub files: u64,
    pub deleted: u64,
    pub erased: u64,
    pub carved: u64,
    pub carved_complete: u64,
    pub total_bytes: u64,
}

fn encode_extents(ext: &[ItemExtent]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ext.len().saturating_mul(16));
    for (off, len) in ext {
        out.extend_from_slice(&off.unwrap_or(u64::MAX).to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
    }
    out
}

fn decode_extents(b: &[u8]) -> Vec<ItemExtent> {
    b.chunks_exact(16)
        .map(|c| {
            let off = u64::from_le_bytes(c[0..8].try_into().unwrap_or_default());
            let len = u64::from_le_bytes(c[8..16].try_into().unwrap_or_default());
            (if off == u64::MAX { None } else { Some(off) }, len)
        })
        .collect()
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

const ROW_COLUMNS: &str = "id, path, name, size, is_dir, is_deleted, name_complete, origin, extent_quality,
     filesystem, file_type, modified_at, data_state, carve_status, note, overwritten_pct, content_check";

/// Number of columns in [`ROW_COLUMNS`]; queries that select more read them after these.
const ROW_COLUMN_COUNT: usize = 17;

fn row(r: &Row<'_>) -> rusqlite::Result<ScanItemRow> {
    Ok(ScanItemRow {
        id: r.get(0)?,
        path: r.get(1)?,
        name: r.get(2)?,
        size: r.get::<_, i64>(3)?.max(0) as u64,
        is_dir: r.get(4)?,
        is_deleted: r.get(5)?,
        name_complete: r.get(6)?,
        origin: r.get(7)?,
        extent_quality: r.get(8)?,
        filesystem: r.get(9)?,
        file_type: r.get(10)?,
        modified_at: r.get(11)?,
        data_state: r.get(12)?,
        carve_status: r.get(13)?,
        note: r.get(14)?,
        overwritten_pct: r.get(15)?,
        content_check: r.get(16)?,
    })
}

/// SQL condition (prefixed with " AND") selecting the items a view shows.
pub(crate) fn view_condition(view: ItemView) -> &'static str {
    match view {
        ItemView::AllFiles => " AND is_dir = 0 AND is_system = 0",
        ItemView::Deleted => " AND is_dir = 0 AND is_deleted = 1 AND origin = 'metadata'",
        ItemView::Carved => " AND origin = 'carved'",
        ItemView::Everything => "",
    }
}

pub(crate) const ROW_SELECT: &str = ROW_COLUMNS;

pub(crate) fn map_row(r: &Row<'_>) -> rusqlite::Result<ScanItemRow> {
    row(r)
}

/// WHERE clause fragment and bound search pattern for a filter.
fn where_clause(f: &ItemFilter) -> (String, Option<String>) {
    let mut w = String::from("scan_id = ?1");
    w.push_str(view_condition(f.view));
    let pattern = f.search.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| {
        // Escape LIKE wildcards so user input is matched literally.
        let esc = s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        format!("%{esc}%")
    });
    if pattern.is_some() {
        w.push_str(" AND path LIKE ?2 ESCAPE '\\'");
    }
    (w, pattern)
}

impl Database {
    /// Insert a batch of items in one transaction. Fails if the scan does not exist.
    pub fn insert_scan_items(&mut self, scan_id: i64, items: &[NewScanItem]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO scan_items (scan_id, name, path, size, is_dir, is_deleted, origin, extent_quality,
                    filesystem, file_type, created_at, modified_at, extents, volume_offset, name_complete,
                    is_system, data_state, carve_status, note, resident, overwritten_pct, content_check)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                         ?21, ?22)",
            )?;
            for it in items {
                stmt.execute(params![
                    scan_id,
                    it.name,
                    it.path,
                    to_i64(it.size),
                    it.is_dir,
                    it.is_deleted,
                    it.origin,
                    it.extent_quality,
                    it.filesystem,
                    it.file_type,
                    it.created_at,
                    it.modified_at,
                    encode_extents(&it.extents),
                    to_i64(it.volume_offset),
                    it.name_complete,
                    it.is_system,
                    it.data_state,
                    it.carve_status,
                    it.note,
                    it.resident,
                    it.overwritten_pct,
                    it.content_check,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_scan_items(
        &self,
        scan_id: i64,
        filter: &ItemFilter,
        page: Page,
    ) -> DbResult<Vec<ScanItemRow>> {
        let (w, pattern) = where_clause(filter);
        let sql = format!(
            "SELECT {ROW_COLUMNS} FROM scan_items WHERE {w} ORDER BY path COLLATE NOCASE, id LIMIT {} OFFSET {}",
            page.clamped_limit(),
            page.offset
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = match &pattern {
            Some(p) => stmt.query_map(params![scan_id, p], row)?.collect::<Result<Vec<_>, _>>()?,
            None => stmt.query_map(params![scan_id], row)?.collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    pub fn count_scan_items(&self, scan_id: i64, filter: &ItemFilter) -> DbResult<u64> {
        let (w, pattern) = where_clause(filter);
        let sql = format!("SELECT COUNT(*) FROM scan_items WHERE {w}");
        let n: i64 = match &pattern {
            Some(p) => self.conn.query_row(&sql, params![scan_id, p], |r| r.get(0))?,
            None => self.conn.query_row(&sql, params![scan_id], |r| r.get(0))?,
        };
        Ok(n.max(0) as u64)
    }

    pub fn scan_item_stats(&self, scan_id: i64) -> DbResult<ItemStats> {
        Ok(self.conn.query_row(
            "SELECT
                COUNT(*) FILTER (WHERE is_dir = 0 AND is_system = 0),
                COUNT(*) FILTER (WHERE is_dir = 0 AND is_deleted = 1 AND origin = 'metadata'),
                COUNT(*) FILTER (WHERE data_state = 'erased'),
                COUNT(*) FILTER (WHERE origin = 'carved'),
                COUNT(*) FILTER (WHERE carve_status = 'complete'),
                COALESCE(SUM(size) FILTER (WHERE is_dir = 0 AND is_system = 0), 0)
             FROM scan_items WHERE scan_id = ?1",
            [scan_id],
            |r| {
                let u = |i: usize| r.get::<_, i64>(i).map(|v| v.max(0) as u64);
                Ok(ItemStats {
                    files: u(0)?,
                    deleted: u(1)?,
                    erased: u(2)?,
                    carved: u(3)?,
                    carved_complete: u(4)?,
                    total_bytes: u(5)?,
                })
            },
        )?)
    }

    pub fn get_scan_item(&self, id: i64) -> DbResult<ScanItemDetail> {
        self.conn
            .query_row(
                &format!("SELECT {ROW_COLUMNS}, volume_offset, extents, resident, created_at, scan_id FROM scan_items WHERE id = ?1"),
                [id],
                |r| {
                    // Detail columns follow the row columns.
                    let n = ROW_COLUMN_COUNT;
                    Ok(ScanItemDetail {
                        scan_id: r.get(n + 4)?,
                        row: row(r)?,
                        volume_offset: r.get::<_, i64>(n)?.max(0) as u64,
                        extents: r
                            .get::<_, Option<Vec<u8>>>(n + 1)?
                            .as_deref()
                            .map(decode_extents)
                            .unwrap_or_default(),
                        resident: r.get(n + 2)?,
                        created_at: r.get(n + 3)?,
                    })
                },
            )
            .optional()?
            .ok_or(DbError::NotFound("scan item", id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewScan, ScanMode, SourceKind};

    fn item(path: &str, deleted: bool, origin: &str) -> NewScanItem {
        NewScanItem {
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            size: 100,
            is_dir: false,
            is_deleted: deleted,
            is_system: false,
            name_complete: true,
            origin: origin.into(),
            extent_quality: "exact".into(),
            filesystem: Some("NTFS".into()),
            file_type: Some("jpg".into()),
            created_at: None,
            modified_at: Some(1),
            volume_offset: 1_048_576,
            extents: vec![(Some(4096), 50), (None, 50)],
            data_state: if deleted { "erased".into() } else { "present".into() },
            carve_status: (origin == "carved").then(|| "complete".into()),
            note: None,
            resident: None,
            overwritten_pct: deleted.then_some(25),
            content_check: deleted.then(|| "damaged".into()),
        }
    }

    fn db_with_scan() -> (Database, i64) {
        let db = Database::open_in_memory().unwrap();
        let s = db
            .create_scan(&NewScan {
                source_kind: SourceKind::Image,
                source_path: "x.img".into(),
                source_label: None,
                source_size: 1 << 30,
                scan_mode: ScanMode::Deep,
            })
            .unwrap();
        (db, s.id)
    }

    #[test]
    fn insert_list_filter_and_detail() {
        let (mut db, id) = db_with_scan();
        let mut items = vec![
            item("/Photos/b.jpg", false, "metadata"),
            item("/Photos/a_50%_off.jpg", true, "metadata"),
            item("/$Carved/jpg/jpg_000000001000.jpg", true, "carved"),
        ];
        let mut sys = item("/$MFT", false, "metadata");
        sys.is_system = true;
        items.push(sys);
        db.insert_scan_items(id, &items).unwrap();

        let all = db.list_scan_items(id, &ItemFilter::default(), Page::default()).unwrap();
        assert_eq!(all.len(), 3, "system files hidden by default");
        assert_eq!(all[0].path, "/$Carved/jpg/jpg_000000001000.jpg", "sorted by path");

        let deleted = ItemFilter { view: ItemView::Deleted, search: None };
        assert_eq!(db.count_scan_items(id, &deleted).unwrap(), 1);
        let carved = ItemFilter { view: ItemView::Carved, search: None };
        assert_eq!(
            db.list_scan_items(id, &carved, Page::default()).unwrap()[0].carve_status.as_deref(),
            Some("complete")
        );

        // LIKE wildcards in user input are literal.
        let search = ItemFilter { view: ItemView::AllFiles, search: Some("50%".into()) };
        assert_eq!(db.count_scan_items(id, &search).unwrap(), 1);
        let underscore = ItemFilter { view: ItemView::AllFiles, search: Some("a_5".into()) };
        assert_eq!(db.count_scan_items(id, &underscore).unwrap(), 1);
        let none = ItemFilter { view: ItemView::AllFiles, search: Some("zzz".into()) };
        assert_eq!(db.count_scan_items(id, &none).unwrap(), 0);

        let s = db.scan_item_stats(id).unwrap();
        assert_eq!(
            (s.files, s.deleted, s.erased, s.carved, s.carved_complete, s.total_bytes),
            (3, 1, 2, 1, 1, 300)
        );

        let d = db.get_scan_item(all[1].id).unwrap();
        assert_eq!(d.extents, vec![(Some(4096), 50), (None, 50)], "extents round-trip incl. sparse");
        assert_eq!(d.volume_offset, 1_048_576);
        let del = db.list_scan_items(id, &deleted, Page::default()).unwrap();
        assert_eq!((del[0].overwritten_pct, del[0].content_check.as_deref()), (Some(25), Some("damaged")));
        assert_eq!((all[2].overwritten_pct, all[2].content_check.as_deref()), (None, None));
    }

    #[test]
    fn row_column_count_matches_the_column_list() {
        assert_eq!(ROW_COLUMNS.split(',').count(), ROW_COLUMN_COUNT);
    }

    #[test]
    fn overwritten_and_content_check_values_are_constrained() {
        let (mut db, id) = db_with_scan();
        let mut over = item("/o", true, "metadata");
        over.overwritten_pct = Some(101);
        assert!(db.insert_scan_items(id, &[over]).is_err(), "percentage above 100 rejected");
        let mut check = item("/c", true, "metadata");
        check.content_check = Some("playable?".into());
        assert!(db.insert_scan_items(id, &[check]).is_err(), "unknown check value rejected");
    }

    #[test]
    fn items_are_deleted_with_their_scan_and_bad_values_rejected() {
        let (mut db, id) = db_with_scan();
        db.insert_scan_items(id, &[item("/a", false, "metadata")]).unwrap();
        let mut bad = item("/b", false, "metadata");
        bad.data_state = "vanished".into();
        assert!(db.insert_scan_items(id, &[bad]).is_err(), "CHECK constraint");
        assert_eq!(
            db.count_scan_items(id, &ItemFilter::default()).unwrap(),
            1,
            "failed batch rolled back entirely"
        );
        assert!(db.insert_scan_items(999, &[item("/c", false, "metadata")]).is_err(), "unknown scan");
        db.finish_scan(id, crate::ScanStatus::Completed, None).unwrap();
        db.delete_scan(id).unwrap();
        assert_eq!(db.count_scan_items(id, &ItemFilter::default()).unwrap(), 0, "cascade delete");
    }

    #[test]
    fn paging_over_many_items() {
        let (mut db, id) = db_with_scan();
        let items: Vec<_> = (0..1200).map(|i| item(&format!("/f{i:05}"), i % 3 == 0, "metadata")).collect();
        db.insert_scan_items(id, &items).unwrap();
        let p1 = db.list_scan_items(id, &ItemFilter::default(), Page { limit: 500, offset: 0 }).unwrap();
        let p3 = db.list_scan_items(id, &ItemFilter::default(), Page { limit: 500, offset: 1000 }).unwrap();
        assert_eq!((p1.len(), p3.len()), (500, 200));
        assert_eq!(p3.last().unwrap().path, "/f01199");
    }
}
