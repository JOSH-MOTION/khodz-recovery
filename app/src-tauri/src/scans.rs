//! Scan jobs: open a source read-only, run the engine, stream results into SQLite in batches.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use khodz_core::{CancelToken, Error};
use khodz_db::{
    ItemFilter, ItemStats, NewScan, NewScanItem, Page, ScanItemDetail, ScanItemRow, ScanStatus, SourceKind,
};
use khodz_engine::{DataState, Origin, ScanItem, ScanMode, ScanProgress, ScanSummary};
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

fn open_source(
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
pub fn run_scan(
    state: &AppState,
    source: &ScanSource,
    mode: ScanMode,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(ScanProgress),
) -> Result<ScanOutcome, CommandError> {
    let (src, kind, path, label) = open_source(source)?;
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

    let result = khodz_engine::scan(
        &*src,
        mode,
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
    let db = state.db()?;
    db.finish_scan(scan.id, status, error.as_deref())?;
    Ok(ScanOutcome { scan_id: scan.id, status, items: stored, error, summary })
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    source: ScanSource,
    mode: ScanMode,
    job_id: String,
    on_progress: Channel<ScanProgress>,
) -> Result<ScanOutcome, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let cancel = state.register_job(&job_id)?;
        let result = run_scan(&state, &source, mode, &cancel, |p| {
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
    use khodz_db::ItemView;

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
    }
}
