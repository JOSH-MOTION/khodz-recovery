//! Scan jobs: open a source read-only, run the engine, stream results into SQLite in batches.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use khodz_core::{CancelToken, Error};
use khodz_db::{
    FolderEntry, ItemFilter, ItemStats, ItemView, NewScan, NewScanItem, NewScanVolume, Page, ScanItemDetail,
    ScanItemRow, ScanStatus, ScanVolume, Scope, SourceKind,
};
use khodz_engine::{DataState, Origin, ScanItem, ScanMode, ScanProgress, ScanSummary, VolumeSpec};
use khodz_io::{BlockSource, ImageFile};

use crate::commands::CommandError;
use crate::state::AppState;

const BATCH: usize = 2000;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanSource {
    Image { path: String },
    Device { path: String, label: Option<String> },
}

#[derive(Debug, Serialize)]
pub struct ScanOutcome {
    pub scan_id: i64,
    pub status: ScanStatus,
    pub items: u64,
    pub error: Option<String>,
    pub summary: Option<ScanSummary>,
}

pub(crate) fn open_source(
    source: &ScanSource,
) -> Result<(Box<dyn BlockSource>, SourceKind, String, String), CommandError> {
    match source {
        ScanSource::Image { path } => {
            let img = ImageFile::open(path)?;
            let label = img
                .path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            Ok((Box::new(img), SourceKind::Image, path.clone(), label))
        }
        #[cfg(any(windows, target_os = "macos"))]
        ScanSource::Device { path, label } => {
            let dev = khodz_io::device::open_device(path)?;
            Ok((
                Box::new(dev),
                SourceKind::PhysicalDisk,
                path.clone(),
                label.clone().unwrap_or_else(|| path.clone()),
            ))
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        ScanSource::Device { .. } => Err(Error::Unsupported("device scanning on this OS".into()).into()),
    }
}

fn to_row(i: ScanItem) -> NewScanItem {
    NewScanItem {
        path: i.path,
        name: i.name,
        size: i.size,
        is_dir: i.is_dir,
        is_deleted: i.deleted,
        is_system: i.system,
        name_complete: i.name_complete,
        origin: match i.origin {
            Origin::Metadata => "metadata",
            Origin::Carved => "carved",
        }
        .into(),
        extent_quality: i.quality.into(),
        filesystem: i.filesystem,
        file_type: i.file_type,
        created_at: i.created_ms,
        modified_at: i.modified_ms,
        volume_offset: i.volume_offset,
        extents: i.extents.iter().map(|e| (e.offset, e.len)).collect(),
        data_state: match i.data_state {
            DataState::Present => "present",
            DataState::Erased => "erased",
            DataState::Unknown => "unknown",
        }
        .into(),
        carve_status: i.carve_status.map(|s| {
            match s {
                khodz_carve::CarveStatus::Complete => "complete",
                khodz_carve::CarveStatus::Unverified => "unverified",
                khodz_carve::CarveStatus::Partial => "partial",
            }
            .into()
        }),
        note: i.note,
        resident: i.resident,
    }
}

/// Run a scan to completion (or cancellation/failure), persisting results as it goes.
#[cfg(test)]
pub fn run_scan(
    state: &AppState,
    source: &ScanSource,
    mode: ScanMode,
    cancel: &CancelToken,
    on_progress: impl FnMut(ScanProgress),
) -> Result<ScanOutcome, CommandError> {
    run_scan_volumes(state, source, mode, None, cancel, on_progress)
}

/// [`run_scan`] restricted to explicit volumes (e.g. lost partitions found by the search).
pub fn run_scan_volumes(
    state: &AppState,
    source: &ScanSource,
    mode: ScanMode,
    volumes: Option<&[VolumeSpec]>,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<ScanOutcome, CommandError> {
    let (src, kind, path, label) = open_source(source)?;
    if let Some(v) = volumes {
        if v.is_empty() {
            return Err(CommandError {
                kind: "invalid_request",
                message: "no volumes selected to scan".into(),
            });
        }
        if v.iter().any(|s| s.offset >= src.len()) {
            return Err(CommandError {
                kind: "invalid_request",
                message: "a selected volume lies beyond the end of the source; analyse it again".into(),
            });
        }
    }
    let scan = state.db()?.create_scan(&NewScan {
        source_kind: kind,
        source_path: path,
        source_label: Some(label),
        source_size: src.len(),
        scan_mode: match mode {
            ScanMode::Quick => khodz_db::ScanMode::Quick,
            ScanMode::Deep => khodz_db::ScanMode::Deep,
        },
    })?;

    let mut batch: Vec<NewScanItem> = Vec::with_capacity(BATCH);
    let mut stored = 0u64;
    let mut last_emit: Option<(Instant, khodz_engine::Phase)> = None;
    let flush = |batch: &mut Vec<NewScanItem>, stored: &mut u64| -> khodz_core::Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut db = state.db().map_err(|e| Error::InvalidData(e.message))?;
        db.insert_scan_items(scan.id, batch).map_err(|e| Error::InvalidData(e.to_string()))?;
        *stored = stored.saturating_add(batch.len() as u64);
        db.update_scan_items_found(scan.id, *stored).map_err(|e| Error::InvalidData(e.to_string()))?;
        batch.clear();
        Ok(())
    };

    let result = khodz_engine::scan_volumes(
        &*src,
        mode,
        volumes,
        cancel,
        &mut |p| {
            let now = Instant::now();
            let due = match last_emit {
                None => true,
                Some((t, phase)) => phase != p.phase || now.duration_since(t) >= Duration::from_millis(100),
            };
            if due || p.bytes_done >= p.bytes_total {
                last_emit = Some((now, p.phase));
                on_progress(p);
            }
        },
        &mut |item| {
            batch.push(to_row(item));
            if batch.len() >= BATCH { flush(&mut batch, &mut stored) } else { Ok(()) }
        },
    );
    // Keep everything found so far, whatever the outcome: partial results from a failing or
    // cancelled scan are still valuable.
    let flushed = flush(&mut batch, &mut stored);

    let (status, error, summary) = match (result, flushed) {
        (Ok(s), Ok(())) => (ScanStatus::Completed, None, Some(s)),
        (Err(Error::Cancelled), _) => (ScanStatus::Cancelled, None, None),
        (Err(e @ Error::DeviceLost(_)), _) => (ScanStatus::DeviceLost, Some(e.to_string()), None),
        (Err(e), _) | (Ok(_), Err(e)) => (ScanStatus::Failed, Some(e.to_string()), None),
    };
    let mut db = state.db()?;
    if let Some(s) = &summary {
        let volumes: Vec<NewScanVolume> = s
            .volumes
            .iter()
            .map(|v| NewScanVolume {
                offset: v.offset,
                size: v.size,
                filesystem: v.filesystem.clone(),
                label: v.label.clone(),
                entries: v.entries,
                deleted: v.deleted,
                corrupt: v.corrupt,
                error: v.error.clone(),
            })
            .collect();
        db.insert_scan_volumes(scan.id, &volumes)?;
    }
    db.finish_scan(scan.id, status, error.as_deref())?;
    Ok(ScanOutcome { scan_id: scan.id, status, items: stored, error, summary })
}

/// Maximum bytes returned for one preview request.
const PREVIEW_MAX: usize = 32 << 20;

/// Re-open a scan's source read-only, refusing if it is no longer the same size as when it
/// was scanned (a different or modified medium would make every stored extent wrong).
pub fn open_source_for_scan(state: &AppState, scan_id: i64) -> Result<Box<dyn BlockSource>, CommandError> {
    let scan = state.db()?.get_scan(scan_id)?;
    let source = match scan.source_kind {
        SourceKind::Image => ScanSource::Image { path: scan.source_path.clone() },
        _ => ScanSource::Device { path: scan.source_path.clone(), label: None },
    };
    let (src, _, _, _) = open_source(&source)?;
    if src.len() != scan.source_size {
        return Err(CommandError {
            kind: "source_changed",
            message: format!(
                "{} is now {} bytes but was {} bytes when scanned; rescan it",
                scan.source_path,
                src.len(),
                scan.source_size
            ),
        });
    }
    Ok(src)
}

/// Read part of a scan item's data from its (read-only) source.
pub fn read_item_impl(
    state: &AppState,
    item_id: i64,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, CommandError> {
    let detail = state.db()?.get_scan_item(item_id)?;
    if detail.row.is_dir {
        return Err(CommandError { kind: "invalid_request", message: "folders have no data".into() });
    }
    let src = open_source_for_scan(state, detail.scan_id)?;
    let extents: Vec<khodz_engine::AbsExtent> =
        detail.extents.iter().map(|&(offset, len)| khodz_engine::AbsExtent { offset, len }).collect();
    Ok(khodz_engine::read_item_range(
        &*src,
        &extents,
        detail.resident.as_deref(),
        detail.row.size,
        offset,
        len.min(PREVIEW_MAX),
    )?)
}

#[tauri::command]
pub async fn read_item_data(
    app: AppHandle,
    item_id: i64,
    offset: u64,
    len: usize,
) -> Result<tauri::ipc::Response, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        read_item_impl(&state, item_id, offset, len).map(tauri::ipc::Response::new)
    })
    .await
    .map_err(|e| CommandError::internal(e.to_string()))?
}

#[tauri::command]
pub fn list_volumes(
    state: State<'_, AppState>,
    scan_id: i64,
    view: ItemView,
) -> Result<Vec<ScanVolume>, CommandError> {
    Ok(state.db()?.list_scan_volumes(scan_id, view)?)
}

#[derive(Debug, Serialize)]
pub struct FolderListing {
    pub folders: Vec<FolderEntry>,
    pub files: Vec<ScanItemRow>,
    pub total_files: u64,
}

#[tauri::command]
pub fn list_folder(
    state: State<'_, AppState>,
    scan_id: i64,
    scope: Scope,
    dir: String,
    view: ItemView,
    page: Option<Page>,
) -> Result<FolderListing, CommandError> {
    let db = state.db()?;
    let page = page.unwrap_or_default();
    // Folders are listed with the first page only.
    let folders = if page.offset == 0 { db.list_folders(scan_id, scope, &dir, view)? } else { Vec::new() };
    let (files, total_files) = db.list_files(scan_id, scope, &dir, view, page)?;
    Ok(FolderListing { folders, files, total_files })
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    source: ScanSource,
    mode: ScanMode,
    volumes: Option<Vec<VolumeSpec>>,
    job_id: String,
    on_progress: Channel<ScanProgress>,
) -> Result<ScanOutcome, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let cancel = state.register_job(&job_id)?;
        let result = run_scan_volumes(&state, &source, mode, volumes.as_deref(), &cancel, |p| {
            let _ = on_progress.send(p);
        });
        state.finish_job(&job_id);
        result
    })
    .await
    .map_err(|e| CommandError::internal(e.to_string()))?
}

#[derive(Debug, Serialize)]
pub struct ItemPage {
    pub items: Vec<ScanItemRow>,
    pub total: u64,
}

#[tauri::command]
pub fn list_scan_items(
    state: State<'_, AppState>,
    scan_id: i64,
    filter: Option<ItemFilter>,
    page: Option<Page>,
) -> Result<ItemPage, CommandError> {
    let filter = filter.unwrap_or_default();
    let db = state.db()?;
    Ok(ItemPage {
        items: db.list_scan_items(scan_id, &filter, page.unwrap_or_default())?,
        total: db.count_scan_items(scan_id, &filter)?,
    })
}

#[tauri::command]
pub fn scan_item_stats(state: State<'_, AppState>, scan_id: i64) -> Result<ItemStats, CommandError> {
    Ok(state.db()?.scan_item_stats(scan_id)?)
}

#[tauri::command]
pub fn get_scan_item(state: State<'_, AppState>, id: i64) -> Result<ScanItemDetail, CommandError> {
    Ok(state.db()?.get_scan_item(id)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new(khodz_db::Database::open_in_memory().unwrap(), "memory".into())
    }

    #[test]
    fn missing_source_creates_no_scan_record() {
        let s = state();
        let r = run_scan(
            &s,
            &ScanSource::Image { path: "Z:/nope.img".into() },
            ScanMode::Quick,
            &CancelToken::new(),
            |_| {},
        );
        assert!(r.is_err());
        assert!(s.db().unwrap().list_scans(Page::default()).unwrap().is_empty());
    }

    #[test]
    fn cancelled_scan_is_recorded_as_cancelled() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), vec![0u8; 1 << 20]).unwrap();
        let s = state();
        let cancel = CancelToken::new();
        cancel.cancel();
        let out = run_scan(
            &s,
            &ScanSource::Image { path: f.path().to_string_lossy().into() },
            ScanMode::Deep,
            &cancel,
            |_| {},
        )
        .unwrap();
        assert_eq!(out.status, ScanStatus::Cancelled);
        let rec = s.db().unwrap().get_scan(out.scan_id).unwrap();
        assert_eq!(rec.status, ScanStatus::Cancelled);
        assert!(rec.finished_at.is_some());
    }

    #[test]
    #[ignore = "needs fixtures/out/carving.img and mbr-multi.img"]
    fn deep_and_quick_scans_persist_results() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/out/");
        let s = state();
        let mut progress = Vec::new();
        let out = run_scan(
            &s,
            &ScanSource::Image { path: format!("{dir}carving.img") },
            ScanMode::Deep,
            &CancelToken::new(),
            |p| progress.push(p),
        )
        .unwrap();
        assert_eq!(out.status, ScanStatus::Completed);
        let db = s.db().unwrap();
        let stats = db.scan_item_stats(out.scan_id).unwrap();
        assert_eq!(stats.carved_complete, 6);
        assert_eq!(db.get_scan(out.scan_id).unwrap().items_found, out.items);
        assert!(!progress.is_empty() && progress.len() < 200, "progress throttled: {}", progress.len());
        drop(db);

        let out = run_scan(
            &s,
            &ScanSource::Image { path: format!("{dir}mbr-multi.img") },
            ScanMode::Quick,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let db = s.db().unwrap();
        let deleted = ItemFilter { view: ItemView::Deleted, search: None };
        let rows = db.list_scan_items(out.scan_id, &deleted, Page::default()).unwrap();
        assert!(rows.iter().any(|r| r.name == "to-delete-1.bin" && r.data_state == "erased"));
        let search = ItemFilter { view: ItemView::AllFiles, search: Some("report.bin".into()) };
        assert_eq!(db.count_scan_items(out.scan_id, &search).unwrap(), 3, "one per volume");
        let vols = db.list_scan_volumes(out.scan_id, ItemView::AllFiles).unwrap();
        let labels: Vec<_> = vols.iter().map(|v| v.label.clone().unwrap_or_default()).collect();
        assert_eq!(labels, vec!["PFAT", "LNTFS", "LEXFAT"], "volume summaries stored");
    }

    /// End to end: scan, then read every verified carved file and every live file back through
    /// the preview path and compare with the generator's SHA-256 manifest.
    #[test]
    #[ignore = "needs fixtures/out (both generators)"]
    fn preview_bytes_match_original_files() {
        use sha2::{Digest, Sha256};
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/out/");
        let hex = |d: &[u8]| Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect::<String>();
        let s = state();

        let out = run_scan(
            &s,
            &ScanSource::Image { path: format!("{dir}carving.img") },
            ScanMode::Deep,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(format!("{dir}carving-manifest.json"))
                .unwrap()
                .trim_start_matches('\u{feff}'),
        )
        .unwrap();
        let carved = ItemFilter { view: ItemView::Carved, search: None };
        let rows =
            s.db().unwrap().list_scan_items(out.scan_id, &carved, Page { limit: 500, offset: 0 }).unwrap();
        let mut checked = 0;
        for f in manifest["files"].as_array().unwrap().iter().filter(|f| f["expect"] == "complete") {
            let off = f["offset"].as_u64().unwrap();
            let row = rows.iter().find(|r| r.name.contains(&format!("{off:012X}"))).expect("carved item");
            let data = read_item_impl(&s, row.id, 0, usize::MAX).unwrap();
            assert_eq!(hex(&data), f["sha256"].as_str().unwrap(), "{}", f["name"]);
            checked += 1;
        }
        assert_eq!(checked, 5);

        let out = run_scan(
            &s,
            &ScanSource::Image { path: format!("{dir}gpt-ntfs.img") },
            ScanMode::Quick,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let fs_manifest: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(format!("{dir}manifest.json")).unwrap().trim_start_matches('\u{feff}'),
        )
        .unwrap();
        let ntfs =
            fs_manifest["images"].as_array().unwrap().iter().find(|i| i["file"] == "gpt-ntfs.img").unwrap();
        let files = &ntfs["volumes"]["files"];
        let files = files
            .as_array()
            .cloned()
            .unwrap_or_else(|| ntfs["volumes"][0]["files"].as_array().unwrap().clone());
        let db_rows = s
            .db()
            .unwrap()
            .list_scan_items(out.scan_id, &ItemFilter::default(), Page { limit: 500, offset: 0 })
            .unwrap();
        let mut live = 0;
        for f in files.iter().filter(|f| f["deleted"] == false) {
            let path = format!("/{}", f["path"].as_str().unwrap());
            let row = db_rows.iter().find(|r| r.path == path).unwrap_or_else(|| panic!("{path}"));
            let data = read_item_impl(&s, row.id, 0, usize::MAX).unwrap();
            assert_eq!(hex(&data), f["sha256"].as_str().unwrap(), "{path}");
            // Windowed reads agree with the full read.
            if data.len() > 5000 {
                assert_eq!(read_item_impl(&s, row.id, 1234, 3000).unwrap(), data[1234..4234].to_vec());
            }
            live += 1;
        }
        assert!(live >= 8, "{live}");
    }
}
