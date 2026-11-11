//! Scan orchestration.
//!
//! * **Quick scan:** partition table → every volume → filesystem metadata walk (live and
//!   deleted entries). Fast; finds what the filesystem still describes.
//! * **Deep scan:** quick scan, then signature carving across the whole source. Offsets already
//!   explained by filesystem metadata are skipped so the same file is not listed twice.
//!
//! The engine performs no persistence: items are streamed to a caller-supplied sink, which the
//! app batches into SQLite. Everything is read-only and cancellable.

#![cfg_attr(test, allow(clippy::arithmetic_side_effects))]

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use khodz_carve::{CarveStats, CarveStatus, carve};
use khodz_core::{CancelToken, Error, Progress, Result};
use khodz_fs::{Entry, ExtentQuality, FsKind};
use khodz_io::{BlockSource, SliceSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    Quick,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Partitions,
    Metadata,
    Carving,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanProgress {
    pub phase: Phase,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub items_found: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Metadata,
    Carved,
}

/// Whether a deleted file's data still appears to be on the medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataState {
    Present,
    /// The start of the data reads back as zeros (TRIM / wiped).
    Erased,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AbsExtent {
    /// Absolute byte offset on the scanned source; None = sparse.
    pub offset: Option<u64>,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanItem {
    pub origin: Origin,
    pub path: String,
    pub name: String,
    pub name_complete: bool,
    pub is_dir: bool,
    pub size: u64,
    pub deleted: bool,
    pub system: bool,
    pub created_ms: Option<i64>,
    pub modified_ms: Option<i64>,
    /// Filesystem display name ("NTFS") for metadata items.
    pub filesystem: Option<String>,
    pub volume_offset: u64,
    /// Lower-case extension / carved type.
    pub file_type: Option<String>,
    /// `exact`, `contiguity_assumed`, `partial` or `unknown`.
    pub quality: &'static str,
    pub extents: Vec<AbsExtent>,
    pub resident: Option<Vec<u8>>,
    pub data_state: DataState,
    pub carve_status: Option<CarveStatus>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct VolumeSummary {
    pub offset: u64,
    pub size: u64,
    pub filesystem: String,
    pub label: Option<String>,
    pub entries: u64,
    pub deleted: u64,
    pub corrupt: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ScanSummary {
    pub volumes: Vec<VolumeSummary>,
    pub items: u64,
    pub deleted: u64,
    pub erased: u64,
    pub carved: u64,
    pub carve: Option<CarveStats>,
}

fn quality_str(q: ExtentQuality) -> &'static str {
    match q {
        ExtentQuality::Exact => "exact",
        ExtentQuality::ContiguityAssumed => "contiguity_assumed",
        ExtentQuality::Partial => "partial",
    }
}

fn extension(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty() && ext.len() <= 8).then(|| ext.to_lowercase())
}

/// Sample the start of a deleted file's data: all zeros means it was erased.
fn data_state<S: BlockSource + ?Sized>(src: &S, item: &ScanItem) -> Result<DataState> {
    if !item.deleted || item.is_dir || item.size == 0 || item.resident.is_some() {
        return Ok(DataState::Present);
    }
    let Some(off) = item.extents.iter().find_map(|e| e.offset) else { return Ok(DataState::Unknown) };
    let n = usize::try_from(item.size.min(4096)).unwrap_or(4096);
    let mut buf = vec![0u8; n];
    let got = match src.read_exact_at(off, &mut buf) {
        Ok(g) => g,
        Err(Error::MediaError { .. }) => return Ok(DataState::Unknown),
        Err(e) => return Err(e),
    };
    Ok(if got > 0 && buf[..got].iter().all(|&b| b == 0) { DataState::Erased } else { DataState::Present })
}

fn metadata_item(e: Entry, vol_offset: u64, fs: FsKind) -> ScanItem {
    let extents = e
        .extents
        .iter()
        .map(|x| AbsExtent { offset: x.offset.map(|o| o.saturating_add(vol_offset)), len: x.len })
        .collect();
    ScanItem {
        origin: Origin::Metadata,
        file_type: if e.is_dir { None } else { extension(&e.name) },
        path: e.path,
        name: e.name,
        name_complete: e.name_complete,
        is_dir: e.is_dir,
        size: e.size,
        deleted: e.deleted,
        system: e.system,
        created_ms: e.created_ms,
        modified_ms: e.modified_ms,
        filesystem: Some(fs.display_name().to_string()),
        volume_offset: vol_offset,
        quality: quality_str(e.quality),
        extents,
        resident: e.resident,
        data_state: DataState::Present,
        carve_status: None,
        note: None,
    }
}

/// Run a scan. `on_item` receives every item found; returning an error aborts the scan.
pub fn scan<S: BlockSource + ?Sized>(
    src: &S,
    mode: ScanMode,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(ScanProgress),
    on_item: &mut dyn FnMut(ScanItem) -> Result<()>,
) -> Result<ScanSummary> {
    let mut summary = ScanSummary::default();
    let total = src.len();
    on_progress(ScanProgress {
        phase: Phase::Partitions,
        bytes_done: 0,
        bytes_total: total,
        items_found: 0,
        detail: None,
    });

    let table = khodz_part::read_partition_table(src)?;
    let regions: Vec<(u64, u64)> = if table.partitions.is_empty() {
        vec![(0, total)]
    } else {
        table
            .partitions
            .iter()
            .filter(|p| !p.is_container())
            .filter(|p| p.offset < total)
            .map(|p| (p.offset, p.size.min(total.saturating_sub(p.offset))))
            .collect()
    };

    // Absolute start offsets explained by filesystem metadata (skipped by the carver).
    let mut known: HashSet<u64> = HashSet::new();
    let mut found = 0u64;

    for (vol_offset, vol_size) in regions {
        cancel.check()?;
        let slice = SliceSource::new(src, vol_offset, vol_size, "volume")?;
        let info = khodz_fs::detect(&slice)?;
        let mut vs = VolumeSummary {
            offset: vol_offset,
            size: vol_size,
            filesystem: info.kind.display_name().to_string(),
            label: info.label.clone(),
            ..VolumeSummary::default()
        };
        on_progress(ScanProgress {
            phase: Phase::Metadata,
            bytes_done: vol_offset,
            bytes_total: total,
            items_found: found,
            detail: Some(
                format!("{} {}", vs.filesystem, vs.label.clone().unwrap_or_default()).trim().to_string(),
            ),
        });
        if info.kind.walkable() {
            match khodz_fs::open(&slice) {
                Ok(fs) => {
                    vs.label = fs.info().label.clone().or(vs.label);
                    let kind = info.kind;
                    let walked = fs.walk(cancel, &mut |entry| {
                        let mut item = metadata_item(entry, vol_offset, kind);
                        item.data_state = data_state(src, &item)?;
                        if item.data_state == DataState::Erased {
                            item.note = Some("data erased (zeros), likely TRIM".into());
                            summary.erased = summary.erased.saturating_add(1);
                        }
                        if let Some(first) = item.extents.first().and_then(|e| e.offset) {
                            known.insert(first);
                        }
                        if item.deleted {
                            summary.deleted = summary.deleted.saturating_add(1);
                        }
                        found = found.saturating_add(1);
                        on_item(item)?;
                        if found % 2048 == 0 {
                            on_progress(ScanProgress {
                                phase: Phase::Metadata,
                                bytes_done: vol_offset,
                                bytes_total: total,
                                items_found: found,
                                detail: None,
                            });
                        }
                        Ok(())
                    });
                    match walked {
                        Ok(stats) => {
                            vs.entries = stats.entries;
                            vs.deleted = stats.deleted;
                            vs.corrupt = stats.corrupt;
                        }
                        Err(Error::Cancelled) => return Err(Error::Cancelled),
                        Err(e @ Error::DeviceLost(_)) => return Err(e),
                        Err(e) => vs.error = Some(e.to_string()),
                    }
                }
                Err(e) => vs.error = Some(e.to_string()),
            }
        }
        summary.volumes.push(vs);
    }

    if mode == ScanMode::Deep {
        let step = u64::from(src.sector_size()).max(512);
        let skip = |o: u64| known.contains(&o);
        let stats = carve(
            src,
            0..total,
            step,
            cancel,
            &skip,
            |p: Progress| {
                on_progress(ScanProgress {
                    phase: Phase::Carving,
                    bytes_done: p.bytes_done,
                    bytes_total: p.bytes_total,
                    items_found: found.saturating_add(p.items_found),
                    detail: None,
                })
            },
            |c| {
                let ext = c.kind.extension();
                let name = format!("{ext}_{:012X}.{ext}", c.offset);
                summary.carved = summary.carved.saturating_add(1);
                on_item(ScanItem {
                    origin: Origin::Carved,
                    path: format!("/$Carved/{ext}/{name}"),
                    name,
                    name_complete: false,
                    is_dir: false,
                    size: c.len,
                    deleted: true,
                    system: false,
                    created_ms: None,
                    modified_ms: None,
                    filesystem: None,
                    volume_offset: 0,
                    file_type: Some(ext.to_string()),
                    quality: match c.status {
                        CarveStatus::Complete => "exact",
                        CarveStatus::Unverified => "contiguity_assumed",
                        CarveStatus::Partial => "partial",
                    },
                    extents: vec![AbsExtent { offset: Some(c.offset), len: c.len }],
                    resident: None,
                    data_state: DataState::Present,
                    carve_status: Some(c.status),
                    note: c.note,
                })
            },
        )?;
        found = found.saturating_add(stats.found);
        summary.carve = Some(stats);
    }
    summary.items = found;
    on_progress(ScanProgress {
        phase: Phase::Carving,
        bytes_done: total,
        bytes_total: total,
        items_found: found,
        detail: None,
    });
    Ok(summary)
}
