//! Browsing scan results as a tree: volumes ("locations"), then folders derived from item
//! paths, then files. Folder queries use path *range* conditions (`path >= 'a/' AND path < 'a0'`)
//! so the `(scan_id, path)` index is used and user-controlled path characters need no escaping.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::items::{ROW_SELECT, map_row, view_condition};
use crate::{Database, DbResult, ItemView, Page, ScanItemRow};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewScanVolume {
    pub offset: u64,
    pub size: u64,
    pub filesystem: String,
    pub label: Option<String>,
    pub entries: u64,
    pub deleted: u64,
    pub corrupt: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanVolume {
    pub offset: u64,
    pub size: Option<u64>,
    pub filesystem: String,
    pub label: Option<String>,
    pub error: Option<String>,
    /// Files in this volume visible in the current view.
    pub files: u64,
    pub deleted: u64,
}

/// Where in a scan's results a folder lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// Metadata items of the volume starting at this byte offset.
    Volume { offset: u64 },
    /// Files found by signature carving.
    Carved,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
    /// Files (recursively) under this folder that the current view shows.
    pub files: u64,
    pub deleted: u64,
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// `"/docs"` → (`"/docs/"`, `"/docs0"`): every path strictly inside the folder sorts in between
/// ('0' is the character after '/').
fn path_range(dir: &str) -> (String, String) {
    let base = dir.trim_end_matches('/');
    (format!("{base}/"), format!("{base}0"))
}

fn scope_condition(scope: Scope) -> (&'static str, Option<i64>) {
    match scope {
        Scope::Volume { offset } => (" AND origin = 'metadata' AND volume_offset = ?4", Some(to_i64(offset))),
        Scope::Carved => (" AND origin = 'carved'", None),
    }
}

impl Database {
    pub fn insert_scan_volumes(&mut self, scan_id: i64, volumes: &[NewScanVolume]) -> DbResult<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO scan_volumes (scan_id, offset, size, filesystem, label, entries, deleted, corrupt, error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for v in volumes {
                stmt.execute(params![
                    scan_id,
                    to_i64(v.offset),
                    to_i64(v.size),
                    v.filesystem,
                    v.label,
                    to_i64(v.entries),
                    to_i64(v.deleted),
                    to_i64(v.corrupt),
                    v.error
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Volumes of a scan with per-view file counts. Scans recorded before volumes were stored
    /// fall back to grouping their items.
    pub fn list_scan_volumes(&self, scan_id: i64, view: ItemView) -> DbResult<Vec<ScanVolume>> {
        let cond = view_condition(view);
        let counts_sql = format!(
            "SELECT volume_offset, MAX(filesystem), COUNT(*), COALESCE(SUM(is_deleted), 0)
             FROM scan_items WHERE scan_id = ?1 AND origin = 'metadata'{cond} GROUP BY volume_offset"
        );
        let mut counts: std::collections::HashMap<u64, (Option<String>, u64, u64)> =
            std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(&counts_sql)?;
            let rows = stmt.query_map([scan_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?.max(0) as u64,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?.max(0) as u64,
                    r.get::<_, i64>(3)?.max(0) as u64,
                ))
            })?;
            for row in rows {
                let (off, fs, n, d) = row?;
                counts.insert(off, (fs, n, d));
            }
        }
        let mut stmt = self.conn.prepare(
            "SELECT offset, size, filesystem, label, error FROM scan_volumes WHERE scan_id = ?1 ORDER BY offset",
        )?;
        let stored: Vec<ScanVolume> = stmt
            .query_map([scan_id], |r| {
                Ok(ScanVolume {
                    offset: r.get::<_, i64>(0)?.max(0) as u64,
                    size: Some(r.get::<_, i64>(1)?.max(0) as u64),
                    filesystem: r.get(2)?,
                    label: r.get(3)?,
                    error: r.get(4)?,
                    files: 0,
                    deleted: 0,
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut out = if stored.is_empty() {
            let mut v: Vec<ScanVolume> = counts
                .iter()
                .map(|(off, (fs, _, _))| ScanVolume {
                    offset: *off,
                    size: None,
                    filesystem: fs.clone().unwrap_or_else(|| "Unknown".into()),
                    label: None,
                    error: None,
                    files: 0,
                    deleted: 0,
                })
                .collect();
            v.sort_by_key(|x| x.offset);
            v
        } else {
            stored
        };
        for v in &mut out {
            if let Some((_, n, d)) = counts.get(&v.offset) {
                v.files = *n;
                v.deleted = *d;
            }
        }
        Ok(out)
    }

    /// Immediate sub-folders of `dir` within `scope` that contain at least one file the view shows.
    pub fn list_folders(
        &self,
        scan_id: i64,
        scope: Scope,
        dir: &str,
        view: ItemView,
    ) -> DbResult<Vec<FolderEntry>> {
        let (lo, hi) = path_range(dir);
        let (scope_sql, scope_arg) = scope_condition(scope);
        let sql = format!(
            "SELECT substr(rest, 1, instr(rest, '/') - 1) AS seg, COUNT(*), COALESCE(SUM(is_deleted), 0)
             FROM (SELECT substr(path, length(?2) + 1) AS rest, is_deleted FROM scan_items
                   WHERE scan_id = ?1 AND path >= ?2 AND path < ?3 AND is_dir = 0{}{scope_sql})
             WHERE instr(rest, '/') > 0
             GROUP BY seg ORDER BY seg COLLATE NOCASE",
            view_condition(view)
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let map = |r: &rusqlite::Row<'_>| -> rusqlite::Result<FolderEntry> {
            let name: String = r.get(0)?;
            Ok(FolderEntry {
                path: format!("{lo}{name}"),
                name,
                files: r.get::<_, i64>(1)?.max(0) as u64,
                deleted: r.get::<_, i64>(2)?.max(0) as u64,
            })
        };
        let rows = match scope_arg {
            Some(o) => stmt.query_map(params![scan_id, lo, hi, o], map)?.collect::<Result<Vec<_>, _>>()?,
            None => stmt.query_map(params![scan_id, lo, hi], map)?.collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// Ids of every file the view shows under `dir` (recursively) within `scope` — what
    /// "recover this folder" means.
    pub fn folder_item_ids(
        &self,
        scan_id: i64,
        scope: Scope,
        dir: &str,
        view: ItemView,
    ) -> DbResult<Vec<i64>> {
        let (lo, hi) = path_range(dir);
        let (scope_sql, scope_arg) = scope_condition(scope);
        let sql = format!(
            "SELECT id FROM scan_items WHERE scan_id = ?1 AND path >= ?2 AND path < ?3 AND is_dir = 0{}{scope_sql}
             ORDER BY path",
            view_condition(view)
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = match scope_arg {
            Some(o) => {
                stmt.query_map(params![scan_id, lo, hi, o], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?
            }
            None => {
                stmt.query_map(params![scan_id, lo, hi], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?
            }
        };
        Ok(ids)
    }

    /// Files directly inside `dir` within `scope`, paged, plus the total count.
    pub fn list_files(
        &self,
        scan_id: i64,
        scope: Scope,
        dir: &str,
        view: ItemView,
        page: Page,
    ) -> DbResult<(Vec<ScanItemRow>, u64)> {
        let (lo, hi) = path_range(dir);
        let (scope_sql, scope_arg) = scope_condition(scope);
        let cond = format!(
            "scan_id = ?1 AND path >= ?2 AND path < ?3 AND is_dir = 0
             AND instr(substr(path, length(?2) + 1), '/') = 0{}{scope_sql}",
            view_condition(view)
        );
        let list_sql = format!(
            "SELECT {ROW_SELECT} FROM scan_items WHERE {cond} ORDER BY name COLLATE NOCASE, id LIMIT {} OFFSET {}",
            page.clamped_limit(),
            page.offset
        );
        let count_sql = format!("SELECT COUNT(*) FROM scan_items WHERE {cond}");
        let mut stmt = self.conn.prepare(&list_sql)?;
        let (rows, total): (Vec<ScanItemRow>, i64) = match scope_arg {
            Some(o) => (
                stmt.query_map(params![scan_id, lo, hi, o], map_row)?.collect::<Result<_, _>>()?,
                self.conn.query_row(&count_sql, params![scan_id, lo, hi, o], |r| r.get(0))?,
            ),
            None => (
                stmt.query_map(params![scan_id, lo, hi], map_row)?.collect::<Result<_, _>>()?,
                self.conn.query_row(&count_sql, params![scan_id, lo, hi], |r| r.get(0))?,
            ),
        };
        Ok((rows, total.max(0) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewScan, NewScanItem, ScanMode, SourceKind};

    fn item(path: &str, vol: u64, deleted: bool, origin: &str) -> NewScanItem {
        NewScanItem {
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            size: 10,
            is_dir: false,
            is_deleted: deleted,
            is_system: false,
            name_complete: true,
            origin: origin.into(),
            extent_quality: "exact".into(),
            filesystem: (origin == "metadata").then(|| "NTFS".into()),
            file_type: None,
            created_at: None,
            modified_at: None,
            volume_offset: vol,
            extents: vec![],
            data_state: "present".into(),
            carve_status: (origin == "carved").then(|| "complete".into()),
            note: None,
            resident: None,
            overwritten_pct: None,
            content_check: None,
        }
    }

    fn setup() -> (Database, i64) {
        let mut db = Database::open_in_memory().unwrap();
        let scan = db
            .create_scan(&NewScan {
                source_kind: SourceKind::Image,
                source_path: "x".into(),
                source_label: None,
                source_size: 1,
                scan_mode: ScanMode::Deep,
            })
            .unwrap();
        db.insert_scan_items(
            scan.id,
            &[
                item("/a.txt", 0, false, "metadata"),
                item("/docs/r.bin", 0, false, "metadata"),
                item("/docs/gone.bin", 0, true, "metadata"),
                item("/docs/deep/x.dat", 0, false, "metadata"),
                item("/docs0/sibling.txt", 0, false, "metadata"), // must not leak into /docs
                item("/docs/50%/_weird.txt", 0, true, "metadata"), // LIKE metacharacters in paths
                item("/docs/r.bin", 4096, false, "metadata"),     // same path, other volume
                item("/$Carved/jpg/jpg_1.jpg", 0, true, "carved"),
            ],
        )
        .unwrap();
        (db, scan.id)
    }

    #[test]
    fn folders_and_files_by_volume() {
        let (db, id) = setup();
        let v0 = Scope::Volume { offset: 0 };
        let root = db.list_folders(id, v0, "/", ItemView::AllFiles).unwrap();
        let names: Vec<_> = root.iter().map(|f| (f.name.as_str(), f.files, f.deleted)).collect();
        assert_eq!(names, vec![("docs", 4, 2), ("docs0", 1, 0)], "carved and other volumes excluded");
        let (files, total) = db.list_files(id, v0, "/", ItemView::AllFiles, Page::default()).unwrap();
        assert_eq!((files.len(), total, files[0].name.as_str()), (1, 1, "a.txt"));

        let docs = db.list_folders(id, v0, "/docs", ItemView::AllFiles).unwrap();
        assert_eq!(docs.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["/docs/50%", "/docs/deep"]);
        let (files, _) = db.list_files(id, v0, "/docs", ItemView::AllFiles, Page::default()).unwrap();
        assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["gone.bin", "r.bin"]);

        let (weird, _) = db.list_files(id, v0, "/docs/50%", ItemView::AllFiles, Page::default()).unwrap();
        assert_eq!(weird[0].name, "_weird.txt");

        let other = db
            .list_files(id, Scope::Volume { offset: 4096 }, "/docs", ItemView::AllFiles, Page::default())
            .unwrap();
        assert_eq!(other.1, 1);
    }

    #[test]
    fn deleted_view_hides_folders_without_deleted_files() {
        let (db, id) = setup();
        let root = db.list_folders(id, Scope::Volume { offset: 0 }, "/", ItemView::Deleted).unwrap();
        assert_eq!(root.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["docs"]);
        let sub = db.list_folders(id, Scope::Volume { offset: 0 }, "/docs", ItemView::Deleted).unwrap();
        assert_eq!(sub.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["50%"]);
    }

    #[test]
    fn folder_selection_is_recursive_and_view_aware() {
        let (db, id) = setup();
        let v0 = Scope::Volume { offset: 0 };
        assert_eq!(db.folder_item_ids(id, v0, "/docs", ItemView::AllFiles).unwrap().len(), 4);
        assert_eq!(db.folder_item_ids(id, v0, "/docs", ItemView::Deleted).unwrap().len(), 2);
        assert_eq!(db.folder_item_ids(id, v0, "/", ItemView::AllFiles).unwrap().len(), 6);
        assert_eq!(db.folder_item_ids(id, Scope::Carved, "/", ItemView::AllFiles).unwrap().len(), 1);
    }

    #[test]
    fn carved_scope() {
        let (db, id) = setup();
        let root = db.list_folders(id, Scope::Carved, "/", ItemView::AllFiles).unwrap();
        assert_eq!(root[0].name, "$Carved");
        let (files, _) =
            db.list_files(id, Scope::Carved, "/$Carved/jpg", ItemView::AllFiles, Page::default()).unwrap();
        assert_eq!(files[0].name, "jpg_1.jpg");
    }

    #[test]
    fn volumes_stored_and_fallback() {
        let (mut db, id) = setup();
        let derived = db.list_scan_volumes(id, ItemView::AllFiles).unwrap();
        assert_eq!(derived.iter().map(|v| (v.offset, v.files)).collect::<Vec<_>>(), vec![(0, 6), (4096, 1)]);
        db.insert_scan_volumes(
            id,
            &[
                NewScanVolume {
                    offset: 0,
                    size: 4096,
                    filesystem: "NTFS".into(),
                    label: Some("DATA".into()),
                    entries: 6,
                    deleted: 2,
                    corrupt: 0,
                    error: None,
                },
                NewScanVolume {
                    offset: 4096,
                    size: 100,
                    filesystem: "FAT32".into(),
                    label: None,
                    entries: 1,
                    deleted: 0,
                    corrupt: 0,
                    error: Some("x".into()),
                },
            ],
        )
        .unwrap();
        let v = db.list_scan_volumes(id, ItemView::Deleted).unwrap();
        assert_eq!(v[0].label.as_deref(), Some("DATA"));
        assert_eq!((v[0].files, v[0].deleted), (2, 2), "counts follow the view");
        assert_eq!(v[1].error.as_deref(), Some("x"));
    }
}
