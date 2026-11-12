//! Partition analysis.
//!
//! * [`inspect_layout`] checks the partition table against what is actually on the disk:
//!   which filesystem each entry holds, whether it fits its entry, whether the entry's type
//!   matches, and — when a primary boot sector is damaged — whether the backup copy can be used.
//! * [`search_lost_partitions`] reads the space no verified volume covers, sector by sector,
//!   looking for filesystem boot records (primary or backup copies) whose structures check out.
//! * [`open_volume`] gives a read-only view of a volume, substituting the backup boot record in
//!   memory when needed. Nothing is ever written to the source.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use khodz_core::{CancelToken, Error, Progress, Result};
use khodz_fs::FsKind;
use khodz_fs::probe::{BootRecord, parse_boot_record};
use khodz_io::{BadRanges, BlockSource, PatchedSource, SliceSource, read_tolerant};
use khodz_part::{PartitionTable, TableKind};

const MIB: u64 = 1 << 20;
/// Search reads this much at a time (bounded memory).
const CHUNK: usize = 4 << 20;
/// Granularity of the search: boot records start on 512-byte sector boundaries.
const STEP: usize = 512;
/// Unallocated gaps up to this size are normal alignment padding and not reported.
const GAP_SLACK: u64 = MIB;

/// A volume to scan: where it is and, if its primary boot record is damaged, where the backup
/// copy used to open it lives (absolute offset on the source).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub offset: u64,
    pub size: u64,
    pub boot_backup: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeOrigin {
    /// Described by the partition table (or the whole disk when there is none).
    Table,
    /// Found by searching for boot records; not in the current table.
    Search,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnalyzedVolume {
    pub origin: VolumeOrigin,
    /// Table entry number (origin = table), or the current entry a found volume overlaps.
    pub partition: Option<u32>,
    pub type_name: Option<String>,
    pub offset: u64,
    pub size: u64,
    pub filesystem: FsKind,
    pub label: Option<String>,
    /// The boot record was found and the structures it points to (FAT, MFT) are present.
    pub verified: bool,
    /// The filesystem's metadata could be opened for scanning.
    pub opens: bool,
    /// Absolute offset of the backup boot record used because the primary is damaged.
    pub boot_backup: Option<u64>,
    /// Informational remarks.
    pub notes: Vec<String>,
    /// Things that are wrong or put recovery at risk.
    pub problems: Vec<String>,
}

impl AnalyzedVolume {
    pub fn spec(&self) -> VolumeSpec {
        VolumeSpec { offset: self.offset, size: self.size, boot_backup: self.boot_backup }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutReport {
    pub disk_size: u64,
    pub table: PartitionTable,
    pub volumes: Vec<AnalyzedVolume>,
    /// Problems with the table as a whole and with individual entries, worst first.
    pub issues: Vec<String>,
    /// Space no partition covers (beyond normal alignment padding).
    pub unallocated: Vec<(u64, u64)>,
}

/// Read-only view of a volume. With `boot_backup`, the first sector is replaced (in memory) by
/// the backup boot record so the filesystem can be opened despite a damaged primary.
pub fn open_volume<'a, S: BlockSource + ?Sized>(
    src: &'a S,
    spec: &VolumeSpec,
) -> Result<Box<dyn BlockSource + 'a>> {
    let size = spec.size.min(src.len().saturating_sub(spec.offset));
    let slice = SliceSource::new(src, spec.offset, size, "volume")?;
    let Some(at) = spec.boot_backup else { return Ok(Box::new(slice)) };
    let mut sector = vec![0u8; 4096];
    let n = src.read_exact_at(at, &mut sector)?;
    sector.truncate(n);
    let rec = parse_boot_record(&sector)
        .filter(BootRecord::can_open_from_backup)
        .ok_or_else(|| Error::InvalidData(format!("no usable backup boot record at byte {at}")))?;
    sector.truncate(rec.boot_len as usize);
    Ok(Box::new(PatchedSource::new(slice, sector)))
}

fn fmt_size(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len().saturating_sub(1) {
        v /= 1024.0;
        u = u.saturating_add(1);
    }
    if u == 0 { format!("{b} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

/// Read the boot record of a volume at `start`, primary first (at 0, or 1024 for HFS+).
fn primary_record<S: BlockSource + ?Sized>(src: &S, start: u64) -> Result<Option<BootRecord>> {
    let mut head = vec![0u8; 2048];
    let n = src.read_exact_at(start, &mut head)?;
    head.truncate(n);
    if let Some(r) = head.get(..512).and_then(parse_boot_record).filter(|r| r.primary_at == 0) {
        return Ok(Some(r));
    }
    Ok(head.get(1024..).and_then(parse_boot_record).filter(|r| r.primary_at == 1024))
}

/// For a region whose primary boot record is missing or broken, look where FAT32, exFAT and
/// NTFS keep their backups. Returns the record and its absolute offset if it verifies.
fn backup_record<S: BlockSource + ?Sized>(
    src: &S,
    start: u64,
    size: u64,
) -> Result<Option<(BootRecord, u64)>> {
    let mut rels: Vec<u64> = vec![6 * 512, 12 * 512, 6 * 4096, 12 * 4096];
    rels.extend([size.checked_sub(512), size.checked_sub(4096)].into_iter().flatten());
    let mut buf = vec![0u8; 512];
    for rel in rels {
        if rel >= size {
            continue;
        }
        let Some(abs) = start.checked_add(rel) else { continue };
        if src.read_exact_at(abs, &mut buf)? < buf.len() {
            continue;
        }
        let Some(r) = parse_boot_record(&buf) else { continue };
        if r.can_open_from_backup() && r.backup_at == Some(rel) && r.verify(src, start)? {
            return Ok(Some((r, abs)));
        }
    }
    Ok(None)
}

/// Filesystems an MBR type byte promises (None = no expectation checked).
fn expected_kinds(mbr_type: u8) -> Option<&'static [FsKind]> {
    use FsKind::*;
    match mbr_type {
        0x01 => Some(&[Fat12]),
        0x04 | 0x06 | 0x0E => Some(&[Fat12, Fat16]),
        0x0B | 0x0C => Some(&[Fat32, Fat16]),
        0x07 => Some(&[Ntfs, ExFat, BitLocker]),
        _ => None,
    }
}

/// Fill in label / opens for a volume whose filesystem is walkable.
fn try_open<S: BlockSource + ?Sized>(src: &S, v: &mut AnalyzedVolume) {
    if !v.filesystem.walkable() {
        return;
    }
    match open_volume(src, &v.spec()).and_then(|s| khodz_fs::open(s).map(|fs| fs.info().label.clone())) {
        Ok(label) => {
            v.opens = true;
            v.label = label.or(v.label.take());
        }
        Err(Error::DeviceLost(_)) | Err(Error::Cancelled) => {}
        Err(e) => v.problems.push(format!("The filesystem could not be opened: {e}")),
    }
}

fn examine_region<S: BlockSource + ?Sized>(
    src: &S,
    partition: Option<u32>,
    type_name: Option<String>,
    mbr_type: Option<u8>,
    offset: u64,
    size: u64,
) -> Result<AnalyzedVolume> {
    let mut v = AnalyzedVolume {
        origin: VolumeOrigin::Table,
        partition,
        type_name,
        offset,
        size,
        filesystem: FsKind::Unknown,
        label: None,
        verified: false,
        opens: false,
        boot_backup: None,
        notes: Vec::new(),
        problems: Vec::new(),
    };
    let mut rec = primary_record(src, offset)?;
    if let Some(r) = &rec {
        v.filesystem = r.kind;
        v.verified = r.verify(src, offset)?;
        if !v.verified {
            v.problems.push(format!(
                "A {} boot record is present but the structures it points to are missing or damaged.",
                r.kind.display_name()
            ));
        }
    }
    if !v.verified {
        if let Some((r, at)) = backup_record(src, offset, size)? {
            v.problems.push(format!(
                "The primary boot sector is damaged; the intact backup copy at byte {at} is used to read this volume."
            ));
            v.filesystem = r.kind;
            v.verified = true;
            v.boot_backup = Some(at);
            rec = Some(r);
        }
    }
    if rec.is_none() {
        // Encrypted or unsupported filesystems are still identified by the detector.
        let slice = SliceSource::new(src, offset, size, "partition")?;
        let info = khodz_fs::detect(&slice)?;
        v.filesystem = info.kind;
        v.label = info.label;
        v.notes.extend(info.notes);
        let raw_by_design = v.type_name.as_deref().is_some_and(|t| {
            [
                "Microsoft reserved",
                "Windows LDM metadata",
                "Windows LDM data",
                "Storage Spaces",
                "BIOS boot",
                "Linux swap",
                "Linux LVM",
                "Linux RAID",
                "Extended",
            ]
            .contains(&t)
        });
        if raw_by_design {
            v.notes.push("This partition type holds no filesystem by design.".into());
        } else if info.kind == FsKind::Unknown {
            v.problems.push(
                "No recognisable filesystem: unformatted, overwritten, or a type Khodz does not support."
                    .into(),
            );
        }
    }
    if let Some(r) = &rec {
        if r.volume_size > size.saturating_add(u64::from(r.sector_size)) {
            v.problems.push(format!(
                "The filesystem says it is {} but its partition entry is only {}: the entry may have been \
                 shrunk or damaged, and files near the end may be missed.",
                fmt_size(r.volume_size),
                fmt_size(size)
            ));
        }
    }
    if let (Some(t), Some(kinds)) = (mbr_type, mbr_type.and_then(expected_kinds)) {
        if v.filesystem != FsKind::Unknown && !kinds.contains(&v.filesystem) {
            v.problems.push(format!(
                "The partition type ({t:#04x}) does not match the {} filesystem found in it.",
                v.filesystem.display_name()
            ));
        }
    }
    try_open(src, &mut v);
    Ok(v)
}

fn gaps(total: u64, mut used: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    used.sort_unstable();
    let mut out = Vec::new();
    let mut cursor = 0u64;
    for (start, end) in used {
        if start > cursor {
            out.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if cursor < total {
        out.push((cursor, total));
    }
    out
}

/// Check the partition table against the disk contents. Reads a few KiB per partition.
pub fn inspect_layout<S: BlockSource + ?Sized>(src: &S) -> Result<LayoutReport> {
    let total = src.len();
    let table = khodz_part::read_partition_table(src)?;
    let mut issues: Vec<String> = table.warnings.clone();
    let mut volumes = Vec::new();

    if table.partitions.is_empty() {
        let mut v = examine_region(src, None, None, None, 0, total)?;
        if table.kind == TableKind::Unknown && !v.verified {
            v.problems.clear();
            // The plain-language message below says the same as the parser's raw wording.
            issues.retain(|w| !w.starts_with("no 0x55AA boot signature"));
            issues.push(
                "No partition table and no filesystem at the start of the disk. The table may have been \
                 erased — search for lost partitions."
                    .into(),
            );
        }
        if v.filesystem != FsKind::Unknown || table.kind == TableKind::None {
            volumes.push(v);
        }
    } else {
        for p in table.partitions.iter().filter(|p| !p.is_container()) {
            if p.offset >= total {
                issues.push(format!(
                    "Partition {} starts beyond the end of the disk; it cannot be read.",
                    p.number
                ));
                continue;
            }
            let size = p.size.min(total.saturating_sub(p.offset));
            let v =
                examine_region(src, Some(p.number), Some(p.type_name.clone()), p.mbr_type, p.offset, size)?;
            for n in &v.problems {
                issues.push(format!("Partition {}: {n}", p.number));
            }
            volumes.push(v);
        }
    }

    let used: Vec<(u64, u64)> = table
        .partitions
        .iter()
        .filter(|p| !p.is_container())
        .map(|p| (p.offset, p.offset.saturating_add(p.size).min(total)))
        .chain(volumes.iter().map(|v| (v.offset, v.offset.saturating_add(v.size))))
        .collect();
    let unallocated =
        gaps(total, used).into_iter().filter(|(a, b)| b.saturating_sub(*a) > GAP_SLACK).collect();
    Ok(LayoutReport { disk_size: total, table, volumes, issues, unallocated })
}

/// Byte ranges the lost-partition search must read: everything not covered by a verified
/// volume of the current layout.
pub fn search_ranges(layout: &LayoutReport) -> Vec<Range<u64>> {
    let covered: Vec<(u64, u64)> = layout
        .volumes
        .iter()
        .filter(|v| v.verified)
        .map(|v| (v.offset, v.offset.saturating_add(v.size).min(layout.disk_size)))
        .collect();
    gaps(layout.disk_size, covered).into_iter().map(|(a, b)| a..b).collect()
}

struct Search<'a, S: ?Sized> {
    src: &'a S,
    layout: &'a LayoutReport,
    found: Vec<AnalyzedVolume>,
}

impl<S: BlockSource + ?Sized> Search<'_, S> {
    fn known(&self, start: u64) -> bool {
        self.found.iter().any(|v| v.offset == start)
            || self.layout.volumes.iter().any(|v| v.verified && v.offset == start)
    }

    /// Evaluate a boot record seen at absolute `pos`. Returns the end of a volume found.
    fn consider(&mut self, pos: u64, rec: &BootRecord) -> Result<Option<u64>> {
        // As the primary copy.
        if let Some(start) = pos.checked_sub(rec.primary_at) {
            if !self.known(start) && rec.verify(self.src, start)? {
                return Ok(Some(self.add(start, rec, None)));
            }
        }
        // As a backup copy whose primary is gone (FAT32 / exFAT / NTFS only: the signature-only
        // formats cannot tell a backup from a primary).
        if let (true, Some(start)) =
            (rec.can_open_from_backup(), rec.backup_at.and_then(|b| pos.checked_sub(b)))
        {
            if !self.known(start) {
                let primary_ok = match primary_record(self.src, start)? {
                    Some(p) => p.kind == rec.kind && p.verify(self.src, start)?,
                    None => false,
                };
                if !primary_ok && rec.verify(self.src, start)? {
                    return Ok(Some(self.add(start, rec, Some(pos))));
                }
            }
        }
        Ok(None)
    }

    fn add(&mut self, start: u64, rec: &BootRecord, backup: Option<u64>) -> u64 {
        let total = self.src.len();
        let end = start.saturating_add(rec.volume_size);
        let mut v = AnalyzedVolume {
            origin: VolumeOrigin::Search,
            partition: None,
            type_name: None,
            offset: start,
            size: end.min(total).saturating_sub(start),
            filesystem: rec.kind,
            label: None,
            verified: true,
            opens: false,
            boot_backup: backup,
            notes: Vec::new(),
            problems: Vec::new(),
        };
        if backup.is_some() {
            v.notes.push("Found through its backup boot record; the primary boot sector is damaged.".into());
        }
        if !rec.kind.walkable() {
            v.notes.push(format!(
                "{} volumes are detected only; their files cannot be listed yet.",
                rec.kind.display_name()
            ));
        }
        if end > total {
            v.problems.push(format!(
                "The volume is {} but only {} of it is on this disk (truncated image or smaller disk).",
                fmt_size(rec.volume_size),
                fmt_size(v.size)
            ));
        }
        for p in self.layout.table.partitions.iter().filter(|p| !p.is_container()) {
            if p.offset < end && start < p.offset.saturating_add(p.size) {
                v.partition = Some(p.number);
                v.problems.push(format!(
                    "Overlaps partition {} of the current table: it predates the current layout and parts of it \
                     may have been overwritten.",
                    p.number
                ));
                break;
            }
        }
        if let Ok(slice) = SliceSource::new(self.src, v.offset, v.size, "found") {
            if let Ok(info) = khodz_fs::detect(&slice) {
                v.label = info.label;
            }
        }
        try_open(self.src, &mut v);
        self.found.push(v);
        end
    }
}

/// Search the space not covered by verified volumes for lost volumes. Reads sequentially in
/// bounded chunks; unreadable sectors are skipped. Cancellable.
pub fn search_lost_partitions<S: BlockSource + ?Sized>(
    src: &S,
    layout: &LayoutReport,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<Vec<AnalyzedVolume>> {
    let ranges = search_ranges(layout);
    let total: u64 = ranges.iter().map(|r| r.end.saturating_sub(r.start)).sum();
    let mut search = Search { src, layout, found: Vec::new() };
    let mut buf = vec![0u8; CHUNK];
    let mut bad = BadRanges::default();
    let mut done = 0u64;
    let step = STEP as u64;

    for range in ranges {
        // Boot records are sector aligned on the disk, not relative to the range.
        let mut pos = range.start.div_ceil(step).saturating_mul(step);
        while pos < range.end {
            cancel.check()?;
            let want = usize::try_from(range.end.saturating_sub(pos)).unwrap_or(CHUNK).min(CHUNK);
            let n = read_tolerant(src, pos, &mut buf[..want], &mut bad)?;
            if n == 0 {
                break;
            }
            let mut skip_to = None;
            let mut i = 0usize;
            while i.saturating_add(STEP) <= n {
                if let Some(rec) = parse_boot_record(&buf[i..i.saturating_add(STEP)]) {
                    let at = pos.saturating_add(i as u64);
                    if let Some(end) = search.consider(at, &rec)? {
                        // Do not search inside a volume just found.
                        if end > at.saturating_add(step) {
                            skip_to = Some(end.div_ceil(step).saturating_mul(step));
                            break;
                        }
                    }
                }
                i = i.saturating_add(STEP);
            }
            let next = match skip_to {
                Some(s) if s > pos => s.min(range.end),
                _ => pos.saturating_add(n as u64),
            };
            done = done.saturating_add(next.saturating_sub(pos));
            on_progress(Progress {
                bytes_done: done.min(total),
                bytes_total: total,
                items_found: search.found.len() as u64,
            });
            pos = next;
        }
    }
    on_progress(Progress { bytes_done: total, bytes_total: total, items_found: search.found.len() as u64 });
    Ok(search.found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaps_merge_overlaps() {
        assert_eq!(gaps(100, vec![(10, 20), (15, 30), (50, 60)]), vec![(0, 10), (30, 50), (60, 100)]);
        assert_eq!(gaps(100, vec![(0, 100)]), vec![]);
        assert_eq!(gaps(100, vec![]), vec![(0, 100)]);
    }

    #[test]
    fn blank_disk_reports_missing_table_and_searches_everything() {
        let src = khodz_io::MemSource::new(vec![0u8; 8 << 20], 512);
        let layout = inspect_layout(&src).unwrap();
        assert!(layout.volumes.is_empty());
        assert_eq!(layout.issues.len(), 1, "{:?}", layout.issues);
        assert!(layout.issues[0].contains("search for lost partitions"));
        assert_eq!(search_ranges(&layout), vec![0..8 << 20]);
        let mut last = None;
        let found = search_lost_partitions(&src, &layout, &CancelToken::new(), |p| last = Some(p)).unwrap();
        assert!(found.is_empty());
        assert_eq!(last.map(|p| p.bytes_done), Some(8 << 20));
    }

    #[test]
    fn search_is_cancellable() {
        let src = khodz_io::MemSource::new(vec![0u8; 8 << 20], 512);
        let layout = inspect_layout(&src).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(search_lost_partitions(&src, &layout, &cancel, |_| {}), Err(Error::Cancelled)));
    }
}
