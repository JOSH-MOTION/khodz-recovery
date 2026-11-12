//! Filesystem detection and metadata walking.
//!
//! * [`detect`] identifies FAT12/16/32, exFAT, NTFS, HFS+/HFSX, APFS, BitLocker and ext*.
//! * [`open`] returns a [`FileSystem`] for FAT12/16/32, exFAT and NTFS that walks *all*
//!   directory metadata — live entries and the deleted entries still present in it — reporting
//!   each file's extents and how trustworthy they are ([`ExtentQuality`]).
//! * HFS+ and APFS are detected only; walking them is not implemented yet
//!   (see SUPPORTED_FILESYSTEMS.md).
//!
//! All on-disk values are treated as hostile: reads are bounds-checked, chains and trees have
//! hard limits and loop detection, and malformed structures are skipped and counted rather
//! than trusted.

// Overflow-checked arithmetic is enforced for production code; test fixtures build buffers freely.
#![cfg_attr(test, allow(clippy::arithmetic_side_effects))]

mod detect;
mod exfat;
mod fat;
mod ntfs;
pub mod probe;
mod time;

use serde::Serialize;

use khodz_core::{CancelToken, Error, Result};
use khodz_io::BlockSource;

pub use detect::detect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FsKind {
    Fat12,
    Fat16,
    Fat32,
    ExFat,
    Ntfs,
    HfsPlus,
    Hfsx,
    Apfs,
    BitLocker,
    Ext,
    Unknown,
}

impl FsKind {
    pub fn display_name(self) -> &'static str {
        match self {
            FsKind::Fat12 => "FAT12",
            FsKind::Fat16 => "FAT16",
            FsKind::Fat32 => "FAT32",
            FsKind::ExFat => "exFAT",
            FsKind::Ntfs => "NTFS",
            FsKind::HfsPlus => "HFS+",
            FsKind::Hfsx => "HFSX",
            FsKind::Apfs => "APFS",
            FsKind::BitLocker => "BitLocker (encrypted)",
            FsKind::Ext => "ext2/3/4",
            FsKind::Unknown => "Unknown",
        }
    }

    /// Whether [`open`] can walk this filesystem's metadata.
    pub fn walkable(self) -> bool {
        matches!(self, FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 | FsKind::ExFat | FsKind::Ntfs)
    }
}

/// What a filesystem's boot sector / superblock says about it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FsInfo {
    pub kind: FsKind,
    pub label: Option<String>,
    pub serial: Option<String>,
    pub cluster_size: Option<u32>,
    pub sector_size: Option<u32>,
    pub volume_size: Option<u64>,
    pub encrypted: bool,
    pub notes: Vec<String>,
}

/// How far a file's extents can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtentQuality {
    /// Extents come from intact allocation metadata (FAT chain, exFAT NoFatChain, NTFS runs).
    Exact,
    /// Only the start and size survive (deleted FAT files); contiguity is assumed.
    ContiguityAssumed,
    /// Metadata is incomplete (truncated chain, attribute list not followed).
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Extent {
    /// Byte offset within the volume; `None` for a sparse (unallocated, reads-as-zero) run.
    pub offset: Option<u64>,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    /// Stable within one walk: directory-entry byte offset (FAT/exFAT) or MFT record (NTFS).
    pub id: u64,
    /// `/`-separated path from the volume root, e.g. `/docs/report.bin`.
    pub path: String,
    pub name: String,
    /// False when part of the name is lost (deleted FAT 8.3 names lose their first character).
    pub name_complete: bool,
    pub is_dir: bool,
    pub size: u64,
    pub deleted: bool,
    /// Filesystem metadata files (`$MFT`, …).
    pub system: bool,
    /// Milliseconds since the Unix epoch. FAT times are local time without a zone.
    pub created_ms: Option<i64>,
    pub modified_ms: Option<i64>,
    pub extents: Vec<Extent>,
    /// Small NTFS files live inside the MFT record itself.
    pub resident: Option<Vec<u8>>,
    pub quality: ExtentQuality,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WalkStats {
    pub entries: u64,
    pub deleted: u64,
    /// Structures that failed validation and were skipped.
    pub corrupt: u64,
}

pub trait FileSystem: Send + Sync {
    fn info(&self) -> &FsInfo;

    /// The volume being read.
    fn source(&self) -> &dyn BlockSource;

    /// Visit every entry found in the metadata, live and deleted. Stops early if `visit`
    /// returns an error or `cancel` fires.
    fn walk(&self, cancel: &CancelToken, visit: &mut dyn FnMut(Entry) -> Result<()>) -> Result<WalkStats>;

    /// Read up to `limit` bytes of an entry's data by following its extents.
    fn read_data(&self, entry: &Entry, limit: u64) -> Result<Vec<u8>> {
        read_entry_data(self.source(), entry, limit)
    }
}

/// Assemble an entry's bytes from its resident data or extents (sparse runs read as zeros).
pub fn read_entry_data(src: &dyn BlockSource, entry: &Entry, limit: u64) -> Result<Vec<u8>> {
    let want = entry.size.min(limit);
    let cap = usize::try_from(want).map_err(|_| Error::InvalidData("entry too large to buffer".into()))?;
    if let Some(r) = &entry.resident {
        return Ok(r[..r.len().min(cap)].to_vec());
    }
    let mut out = Vec::with_capacity(cap);
    for ext in &entry.extents {
        if out.len() >= cap {
            break;
        }
        let n = usize::try_from(ext.len).unwrap_or(usize::MAX).min(cap.saturating_sub(out.len()));
        let start = out.len();
        out.resize(start.saturating_add(n), 0);
        if let Some(off) = ext.offset {
            let got = src.read_exact_at(off, &mut out[start..])?;
            out.truncate(start.saturating_add(got));
            if got < n {
                break;
            }
        }
    }
    Ok(out)
}

/// Open a walkable filesystem found at the start of `src` (a volume or partition slice).
pub fn open<'a, S: BlockSource + 'a>(src: S) -> Result<Box<dyn FileSystem + 'a>> {
    let info = detect(&src)?;
    match info.kind {
        FsKind::Fat12 | FsKind::Fat16 | FsKind::Fat32 => Ok(Box::new(fat::Fat::open(src)?)),
        FsKind::ExFat => Ok(Box::new(exfat::ExFat::open(src)?)),
        FsKind::Ntfs => Ok(Box::new(ntfs::Ntfs::open(src)?)),
        FsKind::BitLocker => {
            Err(Error::Unsupported("BitLocker-encrypted volume: unlock it in Windows first".into()))
        }
        other => {
            Err(Error::Unsupported(format!("{} metadata walking is not implemented", other.display_name())))
        }
    }
}

/// Join a parent path and a name into a `/`-separated path.
pub(crate) fn join(parent: &str, name: &str) -> String {
    if parent == "/" { format!("/{name}") } else { format!("{parent}/{name}") }
}
