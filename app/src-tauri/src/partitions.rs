//! Partition analysis commands: check a source's partition table against its contents, and
//! search unclaimed space for lost volumes (cancellable job with progress). Read-only.

use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

use khodz_core::{CancelToken, ProgressThrottle};
use khodz_engine::partitions::{inspect_layout, search_lost_partitions, search_ranges};
use khodz_engine::{AnalyzedVolume, LayoutReport};

use crate::commands::CommandError;
use crate::scans::{ScanSource, open_source};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct SearchProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub found: u64,
}

#[derive(Debug, Serialize)]
pub struct SearchOutcome {
    pub layout: LayoutReport,
    /// Volumes found that the current table does not describe.
    pub found: Vec<AnalyzedVolume>,
    /// Bytes the search had to read (space not covered by verified volumes).
    pub searched_bytes: u64,
}

pub fn inspect_impl(source: &ScanSource) -> Result<LayoutReport, CommandError> {
    let (src, ..) = open_source(source)?;
    Ok(inspect_layout(&*src)?)
}

pub fn search_impl(
    source: &ScanSource,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(SearchProgress),
) -> Result<SearchOutcome, CommandError> {
    let (src, ..) = open_source(source)?;
    let layout = inspect_layout(&*src)?;
    let searched_bytes = search_ranges(&layout).iter().map(|r| r.end.saturating_sub(r.start)).sum();
    let mut throttle = ProgressThrottle::new(Duration::from_millis(100));
    let found = search_lost_partitions(&*src, &layout, cancel, |p| {
        if throttle.should_emit(Instant::now(), &p) {
            on_progress(SearchProgress {
                bytes_done: p.bytes_done,
                bytes_total: p.bytes_total,
                found: p.items_found,
            });
        }
    })?;
    Ok(SearchOutcome { layout, found, searched_bytes })
}

#[tauri::command]
pub async fn inspect_partitions(source: ScanSource) -> Result<LayoutReport, CommandError> {
    tauri::async_runtime::spawn_blocking(move || inspect_impl(&source))
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
}

#[tauri::command]
pub async fn search_partitions(
    app: AppHandle,
    source: ScanSource,
    job_id: String,
    on_progress: Channel<SearchProgress>,
) -> Result<SearchOutcome, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let cancel = state.register_job(&job_id)?;
        let result = search_impl(&source, &cancel, |p| {
            let _ = on_progress.send(p);
        });
        state.finish_job(&job_id);
        result
    })
    .await
    .map_err(|e| CommandError::internal(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use khodz_engine::{ScanMode, VolumeSpec};

    use super::*;
    use crate::scans::run_scan_volumes;

    fn state() -> AppState {
        AppState::new(khodz_db::Database::open_in_memory().unwrap(), "memory".into())
    }

    #[test]
    fn blank_image_is_reported_and_searched_completely() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&vec![0u8; 4 << 20]).unwrap();
        let src = ScanSource::Image { path: f.path().to_string_lossy().into() };
        let layout = inspect_impl(&src).unwrap();
        assert!(layout.volumes.is_empty());
        let mut last = None;
        let out = search_impl(&src, &CancelToken::new(), |p| last = Some(p)).unwrap();
        assert!(out.found.is_empty());
        assert_eq!(out.searched_bytes, 4 << 20);
        assert_eq!(last.map(|p| p.bytes_done), Some(4 << 20));
    }

    #[test]
    fn search_can_be_cancelled_and_missing_sources_fail_cleanly() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&vec![0u8; 1 << 20]).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let err =
            search_impl(&ScanSource::Image { path: f.path().to_string_lossy().into() }, &cancel, |_| {})
                .unwrap_err();
        assert_eq!(err.kind, "cancelled");
        let err = inspect_impl(&ScanSource::Image { path: "Z:/missing.img".into() }).unwrap_err();
        assert_eq!(err.kind, "invalid_data");
    }

    #[test]
    fn scanning_with_no_volumes_selected_is_refused() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&vec![0u8; 1 << 20]).unwrap();
        let s = state();
        let src = ScanSource::Image { path: f.path().to_string_lossy().into() };
        let err =
            run_scan_volumes(&s, &src, ScanMode::Quick, Some(&[]), &CancelToken::new(), |_| {}).unwrap_err();
        assert_eq!(err.kind, "invalid_request");
        let beyond = [VolumeSpec { offset: 2 << 20, size: 512, boot_backup: None }];
        let err = run_scan_volumes(&s, &src, ScanMode::Quick, Some(&beyond), &CancelToken::new(), |_| {})
            .unwrap_err();
        assert_eq!(err.kind, "invalid_request");
        assert!(s.db().unwrap().list_scans(khodz_db::Page::default()).unwrap().is_empty(), "no history row");
    }

    /// The whole path through the command layer: a copy of the Windows-built disk with its MBR
    /// erased → search → scan the found volumes → items stored per volume; source unchanged.
    #[test]
    #[ignore = "needs fixtures/out/mbr-multi.img"]
    fn lost_volumes_are_found_and_scanned_into_history() {
        let orig = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/out/mbr-multi.img");
        let mut data = std::fs::read(orig).unwrap();
        data[..512].fill(0);
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&data).unwrap();
        f.flush().unwrap();
        let before = std::fs::read(f.path()).unwrap();
        let src = ScanSource::Image { path: f.path().to_string_lossy().into() };

        let out = search_impl(&src, &CancelToken::new(), |_| {}).unwrap();
        assert!(out.layout.issues.iter().any(|i| i.contains("lost partitions")));
        let labels: Vec<_> = out.found.iter().map(|v| v.label.clone().unwrap_or_default()).collect();
        assert_eq!(labels, ["PFAT", "LNTFS", "LEXFAT"]);

        let s = state();
        let specs: Vec<_> = out.found.iter().map(AnalyzedVolume::spec).collect();
        let scan =
            run_scan_volumes(&s, &src, ScanMode::Quick, Some(&specs), &CancelToken::new(), |_| {}).unwrap();
        assert_eq!(scan.status, khodz_db::ScanStatus::Completed);
        let vols = s.db().unwrap().list_scan_volumes(scan.scan_id, khodz_db::ItemView::AllFiles).unwrap();
        let names: Vec<_> = vols.iter().map(|v| v.label.clone().unwrap_or_default()).collect();
        assert_eq!(names, ["PFAT", "LNTFS", "LEXFAT"]);
        assert!(vols.iter().all(|v| v.files > 0));
        assert_eq!(std::fs::read(f.path()).unwrap(), before, "source modified");
    }
}
