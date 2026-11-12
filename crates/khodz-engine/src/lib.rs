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
use khodz_io::BlockSource;

pub mod partitions;

pub use partitions::{AnalyzedVolume, LayoutReport, VolumeOrigin, VolumeSpec};

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
    /// Deleted files: share (0–100) of the assumed location that other files now occupy.
    pub overwritten_pct: Option<u8>,
    /// Deleted files of checkable formats: whether the data at that location is really the file.
    pub content_check: Option<ContentCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentCheck {
    /// The data passed the format's structural checks (for videos: frames checked against the
    /// movie index).
    Verified,
    /// The data at the assumed location is not (all) this file.
    Damaged,
}

/// Formats whose content can be checked structurally (see `khodz_carve::validate_at`).
fn checkable(ext: &str) -> bool {
    matches!(
        ext,
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "pdf"
            | "zip"
            | "docx"
            | "xlsx"
            | "pptx"
            | "mp4"
            | "mov"
            | "m4v"
            | "3gp"
    )
}

/// Validate a file as recovery would assemble it. `Some((true, note))` = verified,
/// `Some((false, note))` = the data is not this file, `None` = could not decide.
fn check_content<S: BlockSource + ?Sized>(
    src: &S,
    extents: &[AbsExtent],
    size: u64,
    ext: &str,
) -> Result<Option<(bool, String)>> {
    let runs: Vec<(Option<u64>, u64)> = extents.iter().map(|e| (e.offset, e.len)).collect();
    let view = khodz_io::ExtentSource::new(src, &runs, size);
    let result = match khodz_carve::validate_at(&view, 0) {
        Ok(r) => r,
        Err(e @ (Error::DeviceLost(_) | Error::Cancelled)) => return Err(e),
        Err(_) => return Ok(None),
    };
    Ok(match result {
        Some(c) if c.status == CarveStatus::Complete => {
            Some((true, c.note.unwrap_or_else(|| "content verified".into())))
        }
        Some(c) if c.status == CarveStatus::Partial => {
            Some((false, c.note.unwrap_or_else(|| "content breaks part-way".into())))
        }
        Some(_) => None,
        None => Some((false, format!("the data here does not start like a .{ext} file"))),
    })
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
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ScanSummary {
    pub volumes: Vec<VolumeSummary>,
    pub items: u64,
    pub deleted: u64,
    pub erased: u64,
    /// Deleted files whose whole location now belongs to other files.
    pub overwritten: u64,
    /// Deleted files whose content passed its format's checks.
    pub verified: u64,
    pub carved: u64,
    pub carve: Option<CarveStats>,
}

/// Read up to `len` bytes at logical `offset` of an item by following its extents (sparse runs
/// read as zeros; resident data is used directly). Stops early if the extents end before the
/// item's size, so a partial item returns only what is actually located.
pub fn read_item_range<S: BlockSource + ?Sized>(
    src: &S,
    extents: &[AbsExtent],
    resident: Option<&[u8]>,
    size: u64,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>> {
    let end = size.min(offset.saturating_add(len as u64));
    if offset >= end {
        return Ok(Vec::new());
    }
    if let Some(r) = resident {
        let s = usize::try_from(offset).unwrap_or(usize::MAX).min(r.len());
        let e = usize::try_from(end).unwrap_or(usize::MAX).min(r.len());
        return Ok(r[s..e].to_vec());
    }
    let mut out = Vec::with_capacity(usize::try_from(end.saturating_sub(offset)).unwrap_or(0));
    let mut logical = 0u64;
    for ext in extents {
        let ext_end = logical.saturating_add(ext.len);
        let from = offset.max(logical);
        let to = end.min(ext_end);
        if from < to {
            // Extents must be contiguous in logical space: a gap means data we do not have.
            if from != offset.saturating_add(out.len() as u64) {
                break;
            }
            let n = usize::try_from(to.saturating_sub(from)).unwrap_or(0);
            let start = out.len();
            out.resize(start.saturating_add(n), 0);
            if let Some(abs) = ext.offset {
                let got =
                    src.read_exact_at(abs.saturating_add(from.saturating_sub(logical)), &mut out[start..])?;
                if got < n {
                    out.truncate(start.saturating_add(got));
                    break;
                }
            }
        }
        logical = ext_end;
        if logical >= end {
            break;
        }
    }
    Ok(out)
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

fn absolute(extents: &[khodz_fs::Extent], vol_offset: u64) -> Vec<AbsExtent> {
    extents
        .iter()
        .map(|x| AbsExtent { offset: x.offset.map(|o| o.saturating_add(vol_offset)), len: x.len })
        .collect()
}

fn percent(part: u64, whole: u64) -> u8 {
    if whole == 0 {
        return 0;
    }
    // Round up: any overwritten byte must show as at least 1%.
    let p = u128::from(part).saturating_mul(100).div_ceil(u128::from(whole));
    u8::try_from(p.min(100)).unwrap_or(100)
}

fn metadata_item(e: Entry, vol_offset: u64, fs: FsKind) -> ScanItem {
    let extents = absolute(&e.extents, vol_offset);
    let overwritten_pct = e.overwritten.map(|o| percent(o, e.size));
    ScanItem {
        overwritten_pct,
        content_check: None,
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

/// Check a deleted file's content where the filesystem metadata says it is. If that fails and
/// the file could instead have been written around other files (fragmented), check that
/// layout too and keep whichever is really the file.
fn verify_deleted<S: BlockSource + ?Sized>(
    src: &S,
    item: &mut ScanItem,
    alt: Option<Vec<AbsExtent>>,
) -> Result<()> {
    let ext = item.file_type.clone().unwrap_or_default();
    if !checkable(&ext) {
        if item.overwritten_pct == Some(100) {
            item.note = Some("overwritten: other files now occupy all of its space".into());
        }
        return Ok(());
    }
    let primary = check_content(src, &item.extents, item.size, &ext)?;
    if let Some((true, note)) = primary {
        item.content_check = Some(ContentCheck::Verified);
        item.note = Some(note);
        return Ok(());
    }
    if let Some(alt) = alt.filter(|a| !a.is_empty()) {
        if let Some((true, note)) = check_content(src, &alt, item.size, &ext)? {
            item.extents = alt;
            item.overwritten_pct = Some(0);
            item.quality = "contiguity_assumed";
            item.content_check = Some(ContentCheck::Verified);
            item.note = Some(format!("rebuilt around other files (it was fragmented); {note}"));
            return Ok(());
        }
    }
    if let Some((false, note)) = primary {
        item.content_check = Some(ContentCheck::Damaged);
        item.note = Some(match item.overwritten_pct {
            Some(p) if p > 0 => format!("{p}% overwritten by other files; {note}"),
            _ => note,
        });
    } else if item.overwritten_pct == Some(100) {
        item.note = Some("overwritten: other files now occupy all of its space".into());
    }
    Ok(())
}

/// Run a scan of the volumes in the partition table (or the whole source when there is none).
/// `on_item` receives every item found; returning an error aborts the scan.
pub fn scan<S: BlockSource + ?Sized>(
    src: &S,
    mode: ScanMode,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(ScanProgress),
    on_item: &mut dyn FnMut(ScanItem) -> Result<()>,
) -> Result<ScanSummary> {
    scan_volumes(src, mode, None, cancel, on_progress, on_item)
}

/// Like [`scan`], but with an explicit list of volumes (e.g. lost partitions found by
/// [`partitions::search_lost_partitions`]). `None` uses the current partition table; volumes
/// whose primary boot sector is damaged are opened from their backup copy either way.
pub fn scan_volumes<S: BlockSource + ?Sized>(
    src: &S,
    mode: ScanMode,
    volumes: Option<&[VolumeSpec]>,
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

    let regions: Vec<VolumeSpec> = match volumes {
        Some(v) => v.iter().filter(|s| s.offset < total).cloned().collect(),
        None => {
            let layout = partitions::inspect_layout(src)?;
            if layout.volumes.is_empty() {
                vec![VolumeSpec { offset: 0, size: total, boot_backup: None }]
            } else {
                layout.volumes.iter().map(AnalyzedVolume::spec).collect()
            }
        }
    };

    // Absolute start offsets explained by filesystem metadata (skipped by the carver).
    let mut known: HashSet<u64> = HashSet::new();
    let mut found = 0u64;

    for spec in regions {
        cancel.check()?;
        let vol_offset = spec.offset;
        let vol_size = spec.size.min(total.saturating_sub(vol_offset));
        let slice = partitions::open_volume(src, &VolumeSpec { size: vol_size, ..spec.clone() })?;
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
        if let Some(at) = spec.boot_backup {
            vs.notes.push(format!("primary boot sector damaged; opened from the backup at byte {at}"));
        }
        if info.kind.walkable() {
            match khodz_fs::open(&slice) {
                Ok(fs) => {
                    vs.label = fs.info().label.clone().or(vs.label);
                    let kind = info.kind;
                    let walked = fs.walk(cancel, &mut |entry| {
                        let alt = entry.alt_extents.as_deref().map(|a| absolute(a, vol_offset));
                        let mut item = metadata_item(entry, vol_offset, kind);
                        item.data_state = data_state(src, &item)?;
                        if item.data_state == DataState::Erased {
                            item.note = Some("data erased (zeros), likely TRIM".into());
                            summary.erased = summary.erased.saturating_add(1);
                        } else if item.deleted && !item.is_dir && !item.extents.is_empty() {
                            verify_deleted(src, &mut item, alt)?;
                            if item.content_check == Some(ContentCheck::Verified) {
                                summary.verified = summary.verified.saturating_add(1);
                            }
                        }
                        if item.overwritten_pct == Some(100) {
                            summary.overwritten = summary.overwritten.saturating_add(1);
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
                    overwritten_pct: None,
                    content_check: None,
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
