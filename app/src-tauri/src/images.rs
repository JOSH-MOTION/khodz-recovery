//! Disk-image commands: inspect an image and fingerprint it (SHA-256) with progress and
//! cancellation. All access goes through `khodz_io::ImageFile`, which is read-only.

use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use khodz_core::{CancelToken, ProgressThrottle};
use khodz_io::{BlockSource, ImageFile, sha256_source};

use crate::commands::CommandError;
use crate::state::AppState;

#[derive(Debug, Serialize, PartialEq)]
pub struct ImageInfo {
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    pub sector_size: u32,
    pub modified_ms: Option<i64>,
    /// The file itself is marked read-only by the OS (we open it read-only regardless).
    pub file_read_only: bool,
}

pub fn inspect_image_impl(path: &str) -> Result<ImageInfo, CommandError> {
    let img = ImageFile::open(path)?;
    let meta = std::fs::metadata(path).map_err(khodz_core::Error::from)?;
    Ok(ImageInfo {
        path: path.to_string(),
        file_name: img.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size_bytes: img.len(),
        sector_size: img.sector_size(),
        modified_ms: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok()),
        file_read_only: meta.permissions().readonly(),
    })
}

#[tauri::command]
pub async fn inspect_image(path: String) -> Result<ImageInfo, CommandError> {
    tauri::async_runtime::spawn_blocking(move || inspect_image_impl(&path))
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
}

#[derive(Debug, Serialize)]
pub struct VolumeAnalysis {
    /// Partition number, or None when the filesystem starts at sector 0 (no partition table).
    pub partition: Option<u32>,
    pub offset: u64,
    pub size: u64,
    pub type_name: String,
    pub fs: khodz_fs::FsInfo,
    pub walkable: bool,
}

#[derive(Debug, Serialize)]
pub struct ImageAnalysis {
    pub table: khodz_part::PartitionTable,
    pub volumes: Vec<VolumeAnalysis>,
}

/// Read the partition table and identify the filesystem on every partition. Metadata only:
/// reads a few KiB per partition.
pub fn analyze_image_impl(path: &str) -> Result<ImageAnalysis, CommandError> {
    let img = ImageFile::open(path)?;
    let table = khodz_part::read_partition_table(&img)?;
    let regions: Vec<(Option<u32>, u64, u64, String)> = if table.partitions.is_empty() {
        vec![(None, 0, img.len(), "Whole image".into())]
    } else {
        table
            .partitions
            .iter()
            .filter(|p| !p.is_container())
            .map(|p| (Some(p.number), p.offset, p.size, p.type_name.clone()))
            .collect()
    };
    let mut volumes = Vec::new();
    for (partition, offset, size, type_name) in regions {
        // Partitions that run past the end are clamped so detection still works on what exists.
        let size = size.min(img.len().saturating_sub(offset));
        let fs = match khodz_io::SliceSource::new(&img, offset, size, type_name.clone()) {
            Ok(slice) => {
                // Prefer the opened filesystem's info (it includes labels stored in metadata).
                let detected = khodz_fs::detect(&slice)?;
                if detected.kind.walkable() {
                    khodz_fs::open(&slice).map(|f| f.info().clone()).unwrap_or(detected)
                } else {
                    detected
                }
            }
            Err(_) => continue,
        };
        volumes.push(VolumeAnalysis { partition, offset, size, type_name, walkable: fs.kind.walkable(), fs });
    }
    Ok(ImageAnalysis { table, volumes })
}

#[tauri::command]
pub async fn analyze_image(path: String) -> Result<ImageAnalysis, CommandError> {
    tauri::async_runtime::spawn_blocking(move || analyze_image_impl(&path))
        .await
        .map_err(|e| CommandError::internal(e.to_string()))?
}

#[derive(Debug, Clone, Serialize)]
pub struct JobProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Serialize)]
pub struct FingerprintResult {
    pub sha256: String,
    pub bytes: u64,
    pub unreadable_bytes: u64,
    pub unreadable_ranges: Vec<(u64, u64)>,
    pub elapsed_ms: u64,
}

pub fn fingerprint_impl(
    path: &str,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(JobProgress),
) -> Result<FingerprintResult, CommandError> {
    let img = ImageFile::open(path)?;
    let started = Instant::now();
    let mut throttle = ProgressThrottle::new(Duration::from_millis(100));
    let fp = sha256_source(&img, cancel, |p| {
        if throttle.should_emit(Instant::now(), &p) {
            on_progress(JobProgress { bytes_done: p.bytes_done, bytes_total: p.bytes_total });
        }
    })?;
    Ok(FingerprintResult {
        sha256: fp.sha256,
        bytes: fp.bytes,
        unreadable_bytes: fp.unreadable_bytes,
        unreadable_ranges: fp.unreadable_ranges,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

#[tauri::command]
pub async fn fingerprint_image(
    state: State<'_, AppState>,
    path: String,
    job_id: String,
    on_progress: Channel<JobProgress>,
) -> Result<FingerprintResult, CommandError> {
    let cancel = state.register_job(&job_id)?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        fingerprint_impl(&path, &cancel, |p| {
            // A closed channel (window gone) is not an error for the job itself.
            let _ = on_progress.send(p);
        })
    })
    .await
    .map_err(|e| CommandError::internal(e.to_string()));
    state.finish_job(&job_id);
    result?
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, job_id: String) -> Result<bool, CommandError> {
    state.cancel_job(&job_id)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn temp_image(len: usize) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(&vec![0xAB; len]).unwrap();
        f
    }

    #[test]
    fn inspects_real_file() {
        let f = temp_image(1 << 20);
        let info = inspect_image_impl(f.path().to_str().unwrap()).unwrap();
        assert_eq!(info.size_bytes, 1 << 20);
        assert_eq!(info.sector_size, 512);
        assert!(info.modified_ms.is_some());
        assert!(!info.file_name.is_empty());
    }

    #[test]
    fn missing_file_is_a_clean_error() {
        let err = inspect_image_impl("Z:/definitely/missing.img").unwrap_err();
        assert_eq!(err.kind, "invalid_data");
    }

    #[test]
    fn fingerprint_reports_progress_and_hash() {
        let f = temp_image(3 << 20);
        let mut events = Vec::new();
        let r =
            fingerprint_impl(f.path().to_str().unwrap(), &CancelToken::new(), |p| events.push(p)).unwrap();
        assert_eq!(r.bytes, 3 << 20);
        assert_eq!(r.sha256.len(), 64);
        assert_eq!(events.last().map(|p| p.bytes_done), Some(3 << 20), "final progress always sent");
    }

    #[test]
    fn blank_image_analysis_reports_unknown_honestly() {
        let f = temp_image(1 << 20);
        let a = analyze_image_impl(f.path().to_str().unwrap()).unwrap();
        assert_eq!(a.table.kind, khodz_part::TableKind::Unknown);
        assert_eq!(a.volumes.len(), 1);
        assert_eq!(a.volumes[0].fs.kind, khodz_fs::FsKind::Unknown);
        assert!(!a.volumes[0].walkable);
    }

    /// Uses the Windows-built fixture (fixtures/windows/make-fixtures.ps1).
    #[test]
    #[ignore = "needs fixtures/out/mbr-multi.img"]
    fn analyzes_windows_built_multi_partition_image() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/out/mbr-multi.img");
        let a = analyze_image_impl(p).unwrap();
        assert_eq!(a.table.kind, khodz_part::TableKind::Mbr);
        let summary: Vec<_> = a
            .volumes
            .iter()
            .map(|v| (v.partition, v.fs.kind, v.fs.label.clone().unwrap_or_default()))
            .collect();
        use khodz_fs::FsKind::*;
        assert_eq!(
            summary,
            vec![
                (Some(1), Fat32, "PFAT".into()),
                (Some(5), Ntfs, "LNTFS".into()),
                (Some(6), ExFat, "LEXFAT".into())
            ]
        );
    }

    #[test]
    fn fingerprint_can_be_cancelled() {
        let f = temp_image(4 << 20);
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = fingerprint_impl(f.path().to_str().unwrap(), &cancel, |_| {}).unwrap_err();
        assert_eq!(err.kind, "cancelled");
    }
}
