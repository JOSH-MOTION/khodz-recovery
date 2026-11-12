//! FAT12 / FAT16 / FAT32 (Microsoft FAT specification, "FAT: General Overview of On-Disk Format").

use std::collections::HashSet;

use khodz_core::bytes::{le_u16, le_u32, u8_at};
use khodz_core::{CancelToken, Error, Result};
use khodz_io::{BlockSource, CachedSource};

use crate::time::dos_to_ms;
use crate::{Entry, Extent, ExtentQuality, FileSystem, FsInfo, FsKind, WalkStats, join};

/// FAT directories may hold at most 65,536 entries (2 MiB).
const MAX_DIR_BYTES: u64 = 65_536 * 32;
const MAX_DEPTH: u32 = 64;
/// Byte offsets of the 13 UTF-16 characters in a long-file-name entry.
const LFN_CHAR_OFFSETS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

#[derive(Debug, Clone)]
pub struct Geometry {
    pub kind: FsKind,
    pub bytes_per_sector: u32,
    pub cluster_size: u64,
    pub fat_offset: u64,
    pub root_dir_offset: u64,
    pub root_dir_bytes: u64,
    pub root_cluster: u32,
    pub data_offset: u64,
    pub cluster_count: u32,
    pub volume_size: u64,
}

impl Geometry {
    /// Parse and validate a FAT boot sector. Returns None unless every field is plausible.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.get(510..512) != Some(&[0x55, 0xAA]) || !matches!(b.first(), Some(0xEB) | Some(0xE9)) {
            return None;
        }
        let bps = u32::from(le_u16(b, 11)?);
        let spc = u32::from(u8_at(b, 13)?);
        let reserved = u64::from(le_u16(b, 14)?);
        let nfats = u64::from(u8_at(b, 16)?);
        let root_entries = u64::from(le_u16(b, 17)?);
        let total = match le_u16(b, 19)? {
            0 => u64::from(le_u32(b, 32)?),
            n => u64::from(n),
        };
        let media = u8_at(b, 21)?;
        let fatsz16 = u64::from(le_u16(b, 22)?);
        let fatsz = if fatsz16 != 0 { fatsz16 } else { u64::from(le_u32(b, 36)?) };

        if !matches!(bps, 512 | 1024 | 2048 | 4096)
            || !spc.is_power_of_two()
            || spc > 128
            || reserved == 0
            || !(1..=2).contains(&nfats)
            || !(media == 0xF0 || media >= 0xF8)
            || fatsz == 0
            || total == 0
        {
            return None;
        }
        let bps64 = u64::from(bps);
        let root_dir_sectors = root_entries.checked_mul(32)?.div_ceil(bps64);
        let first_data = reserved.checked_add(nfats.checked_mul(fatsz)?)?.checked_add(root_dir_sectors)?;
        let data_sectors = total.checked_sub(first_data)?;
        let cluster_count = u32::try_from(data_sectors.checked_div(u64::from(spc))?).ok()?;
        let kind = if cluster_count < 4085 {
            FsKind::Fat12
        } else if cluster_count < 65_525 {
            FsKind::Fat16
        } else {
            FsKind::Fat32
        };
        let fat32 = kind == FsKind::Fat32;
        if fat32 != (root_entries == 0 && fatsz16 == 0) {
            return None;
        }
        // The FAT must be large enough to describe every cluster.
        let entry_bits: u64 = match kind {
            FsKind::Fat12 => 12,
            FsKind::Fat16 => 16,
            _ => 32,
        };
        if fatsz.checked_mul(bps64)?.checked_mul(8)?
            < (u64::from(cluster_count) + 2).checked_mul(entry_bits)?
        {
            return None;
        }
        let root_cluster = if fat32 { le_u32(b, 44)? } else { 0 };
        if fat32 && (root_cluster < 2 || root_cluster >= cluster_count.checked_add(2)?) {
            return None;
        }
        Some(Geometry {
            kind,
            bytes_per_sector: bps,
            cluster_size: bps64.checked_mul(u64::from(spc))?,
            fat_offset: reserved.checked_mul(bps64)?,
            root_dir_offset: reserved.checked_add(nfats.checked_mul(fatsz)?)?.checked_mul(bps64)?,
            root_dir_bytes: root_dir_sectors.checked_mul(bps64)?,
            root_cluster,
            data_offset: first_data.checked_mul(bps64)?,
            cluster_count,
            volume_size: total.checked_mul(bps64)?,
        })
    }

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && u64::from(c) < u64::from(self.cluster_count) + 2
    }

    fn cluster_offset(&self, c: u32) -> u64 {
        self.data_offset.saturating_add(u64::from(c.saturating_sub(2)).saturating_mul(self.cluster_size))
    }
}

pub struct Fat<S> {
    src: CachedSource<S>,
    g: Geometry,
    info: FsInfo,
}

/// Where a file's data is believed to be, and how far that can be trusted.
struct Layout {
    extents: Vec<Extent>,
    quality: ExtentQuality,
    overwritten: Option<u64>,
    alt: Option<Vec<Extent>>,
}

enum DirLoc {
    FixedRoot,
    Chain(u32),
    /// A deleted directory: its chain is gone, so only the first cluster is read.
    FirstCluster(u32),
}

struct RawEnt {
    offset: usize,
    name: String,
    name_complete: bool,
    is_dir: bool,
    first_cluster: u32,
    size: u32,
    deleted: bool,
    created_ms: Option<i64>,
    modified_ms: Option<i64>,
}

impl<S: BlockSource> Fat<S> {
    pub fn open(src: S) -> Result<Self> {
        let mut info = crate::detect(&src)?;
        let mut boot = [0u8; 512];
        src.read_exact_at(0, &mut boot)?;
        let g = Geometry::parse(&boot).ok_or_else(|| Error::InvalidData("invalid FAT boot sector".into()))?;
        let src = CachedSource::new(src, 64 << 10, 128)?;
        let mut fs = Self { src, g, info: info.clone() };
        // Windows keeps the authoritative label in a root-directory entry (the boot sector
        // copy often just says "NO NAME").
        let root =
            if fs.g.kind == FsKind::Fat32 { DirLoc::Chain(fs.g.root_cluster) } else { DirLoc::FixedRoot };
        let (buf, _) = fs.read_dir(&root)?;
        if let Some(label) = root_label(&buf) {
            info.label = Some(label);
        }
        fs.info = info;
        Ok(fs)
    }

    fn fat_entry(&self, c: u32) -> Result<u32> {
        let c64 = u64::from(c);
        let mut b = [0u8; 4];
        Ok(match self.g.kind {
            FsKind::Fat32 => {
                self.src.read_exact_at(self.g.fat_offset.saturating_add(c64.saturating_mul(4)), &mut b)?;
                u32::from_le_bytes(b) & 0x0FFF_FFFF
            }
            FsKind::Fat16 => {
                self.src
                    .read_exact_at(self.g.fat_offset.saturating_add(c64.saturating_mul(2)), &mut b[..2])?;
                u32::from(u16::from_le_bytes([b[0], b[1]]))
            }
            _ => {
                self.src.read_exact_at(
                    self.g.fat_offset.saturating_add(c64.saturating_add(c64 / 2)),
                    &mut b[..2],
                )?;
                let v = u32::from(u16::from_le_bytes([b[0], b[1]]));
                if c & 1 == 1 { v >> 4 } else { v & 0x0FFF }
            }
        })
    }

    fn is_eoc(&self, v: u32) -> bool {
        match self.g.kind {
            FsKind::Fat32 => v >= 0x0FFF_FFF8,
            FsKind::Fat16 => v >= 0xFFF8,
            _ => v >= 0x0FF8,
        }
    }

    /// Follow a cluster chain for at most `limit` clusters. Returns the clusters and whether the
    /// chain ended cleanly (end-of-chain marker) within the limit.
    fn chain(&self, start: u32, limit: u64) -> Result<(Vec<u32>, bool)> {
        let mut out = Vec::new();
        let mut c = start;
        while self.g.valid_cluster(c) {
            out.push(c);
            if out.len() as u64 >= limit {
                return Ok((out, false));
            }
            let next = self.fat_entry(c)?;
            if self.is_eoc(next) {
                return Ok((out, true));
            }
            c = next;
        }
        Ok((out, false))
    }

    fn read_dir(&self, loc: &DirLoc) -> Result<(Vec<u8>, Vec<u64>)> {
        let cs = self.g.cluster_size;
        let clusters: Vec<u32> = match *loc {
            DirLoc::FixedRoot => {
                let len = usize::try_from(self.g.root_dir_bytes.min(MAX_DIR_BYTES)).unwrap_or(0);
                let mut buf = vec![0u8; len];
                let n = self.src.read_exact_at(self.g.root_dir_offset, &mut buf)?;
                buf.truncate(n);
                // One pseudo-"cluster" covering the whole fixed root.
                return Ok((buf, vec![self.g.root_dir_offset]));
            }
            DirLoc::Chain(c) => self.chain(c, MAX_DIR_BYTES.checked_div(cs).unwrap_or(1).max(1))?.0,
            DirLoc::FirstCluster(c) => vec![c],
        };
        let mut buf = Vec::new();
        let mut positions = Vec::new();
        for c in clusters {
            let off = self.g.cluster_offset(c);
            let start = buf.len();
            buf.resize(start.saturating_add(usize::try_from(cs).unwrap_or(0)), 0);
            let n = self.src.read_exact_at(off, &mut buf[start..])?;
            buf.truncate(start.saturating_add(n));
            positions.push(off);
        }
        Ok((buf, positions))
    }

    fn entry_abs_offset(&self, loc: &DirLoc, positions: &[u64], off: usize) -> u64 {
        match loc {
            DirLoc::FixedRoot => self.g.root_dir_offset.saturating_add(off as u64),
            _ => {
                let cs = self.g.cluster_size.max(1);
                let off = off as u64;
                let idx = usize::try_from(off.checked_div(cs).unwrap_or(0)).unwrap_or(usize::MAX);
                positions.get(idx).copied().unwrap_or(0).saturating_add(off.checked_rem(cs).unwrap_or(0))
            }
        }
    }

    fn file_extents(&self, e: &RawEnt, in_deleted_dir: bool) -> Result<Layout> {
        let size = u64::from(e.size);
        let simple = |extents, quality| Layout { extents, quality, overwritten: None, alt: None };
        if size == 0 {
            return Ok(simple(Vec::new(), ExtentQuality::Exact));
        }
        if !self.g.valid_cluster(e.first_cluster) {
            return Ok(simple(Vec::new(), ExtentQuality::Partial));
        }
        let cs = self.g.cluster_size;
        let needed = size.div_ceil(cs);
        if !e.deleted {
            let (clusters, clean) = self.chain(e.first_cluster, needed.saturating_add(1))?;
            let own_chain = clusters.len() as u64 == needed && clean;
            // A live entry inside a deleted folder: its chain was usually freed with the folder
            // (or reused by another file). Only a chain of exactly this file's length that ends
            // cleanly is still its own; otherwise locate it like a deleted file.
            if !in_deleted_dir || own_chain {
                let complete = clusters.len() as u64 >= needed;
                return Ok(simple(
                    runs(
                        &clusters[..clusters.len().min(usize::try_from(needed).unwrap_or(usize::MAX))],
                        &self.g,
                        size,
                    ),
                    if complete { ExtentQuality::Exact } else { ExtentQuality::Partial },
                ));
            }
        }
        // The chain is gone: assume the file was stored contiguously from its first cluster.
        // A live entry in a deleted folder still records its full first cluster.
        let resolved = if e.deleted { self.resolve_deleted_start(e)? } else { Some((e.first_cluster, true)) };
        let Some((start, unambiguous)) = resolved else {
            return Ok(simple(Vec::new(), ExtentQuality::Partial));
        };
        let end = u64::from(start).saturating_add(needed);
        if end > u64::from(self.g.cluster_count) + 2 {
            return Ok(simple(Vec::new(), ExtentQuality::Partial));
        }
        let quality = match (unambiguous, needed) {
            (false, _) => ExtentQuality::Partial,
            (true, 1) => ExtentQuality::Exact,
            (true, _) => ExtentQuality::ContiguityAssumed,
        };
        // Clusters in that range now allocated to other files are overwritten — unless this
        // file was written around them (fragmented), so also build the layout that skips them.
        let mut allocated = 0u64;
        for c in u64::from(start)..end {
            if self.fat_entry(u32::try_from(c).unwrap_or(0))? != 0 {
                allocated = allocated.saturating_add(1);
            }
        }
        let alt = if allocated > 0 { self.skip_allocated(start, needed, size)? } else { None };
        Ok(Layout {
            extents: vec![Extent { offset: Some(self.g.cluster_offset(start)), len: size }],
            quality,
            overwritten: Some(allocated.saturating_mul(cs).min(size)),
            alt,
        })
    }

    /// Free clusters from `start` onwards (skipping allocated ones) until `needed` are found.
    fn skip_allocated(&self, start: u32, needed: u64, size: u64) -> Result<Option<Vec<Extent>>> {
        let mut clusters: Vec<u32> = Vec::new();
        let mut c = start;
        while (clusters.len() as u64) < needed {
            if !self.g.valid_cluster(c) {
                return Ok(None);
            }
            if self.fat_entry(c)? == 0 {
                clusters.push(c);
            }
            c = match c.checked_add(1) {
                Some(n) => n,
                None => return Ok(None),
            };
        }
        Ok(Some(runs(&clusters, &self.g, size)))
    }

    /// A deleted directory's first cluster is trusted only if it still starts with "." and "..",
    /// and (FAT32) the "." entry records exactly this cluster.
    fn looks_like_dir_start(&self, c: u32) -> Result<bool> {
        let mut b = [0u8; 64];
        self.src.read_exact_at(self.g.cluster_offset(c), &mut b)?;
        if &b[0..11] != b".          " || &b[32..43] != b"..         " {
            return Ok(false);
        }
        if self.g.kind != FsKind::Fat32 {
            return Ok(true);
        }
        let own = (u32::from(le_u16(&b, 20).unwrap_or(0)) << 16) | u32::from(le_u16(&b, 26).unwrap_or(0));
        Ok(own == c)
    }

    /// Windows clears the high 16 bits of a deleted FAT32 entry's first cluster. Reconstruct it:
    /// directories are verified through their "." entry; files must land on clusters that are
    /// all free (deleted data is freed). Returns the cluster and whether it is unambiguous.
    fn resolve_deleted_start(&self, raw: &RawEnt) -> Result<Option<(u32, bool)>> {
        let lo = raw.first_cluster & 0xFFFF;
        if self.g.kind != FsKind::Fat32 || raw.first_cluster >> 16 != 0 {
            return Ok(self.g.valid_cluster(raw.first_cluster).then_some((raw.first_cluster, true)));
        }
        let max_hi = (self.g.cluster_count.saturating_add(1)) >> 16;
        let candidates: Vec<u32> =
            (0..=max_hi).map(|hi| (hi << 16) | lo).filter(|&c| self.g.valid_cluster(c)).collect();
        if raw.is_dir {
            for &c in &candidates {
                if self.looks_like_dir_start(c)? {
                    return Ok(Some((c, true)));
                }
            }
            return Ok(None);
        }
        let needed = u64::from(raw.size).div_ceil(self.g.cluster_size).clamp(1, 4096);
        // Score every candidate start: its first cluster must be free (it was freed with the
        // file); prefer one whose data is not blank, then the fewest clusters other files now
        // hold in the range. A fragmented file's true start can include such clusters, so
        // "every cluster free" alone would pick an empty region elsewhere instead.
        let mut scored: Vec<(u32, bool, u64)> = Vec::new();
        let mut head = vec![0u8; 512];
        for &c in &candidates {
            if self.fat_entry(c)? != 0 {
                continue;
            }
            let mut allocated = 0u64;
            let mut fits = true;
            for k in 0..needed {
                let Some(cc) =
                    u32::try_from(u64::from(c).saturating_add(k)).ok().filter(|&x| self.g.valid_cluster(x))
                else {
                    fits = false;
                    break;
                };
                if self.fat_entry(cc)? != 0 {
                    allocated = allocated.saturating_add(1);
                }
            }
            if !fits {
                continue;
            }
            let n = self.src.read_exact_at(self.g.cluster_offset(c), &mut head)?;
            let blank = head[..n].iter().all(|&b| b == 0);
            scored.push((c, blank, allocated));
        }
        let best = scored.iter().min_by_key(|&&(_, blank, allocated)| (blank, allocated)).copied();
        Ok(best.map(|(c, blank, allocated)| {
            let rivals = scored.iter().filter(|&&(o, b, a)| o != c && b == blank && a == allocated).count();
            // Unambiguous when the winner holds data and no other candidate scores as well.
            (c, !blank && rivals == 0)
        }))
    }
}

/// Merge consecutive clusters into extents, truncated to `size` bytes.
fn runs(clusters: &[u32], g: &Geometry, size: u64) -> Vec<Extent> {
    let mut out: Vec<Extent> = Vec::new();
    let mut remaining = size;
    let mut prev: Option<u32> = None;
    for &c in clusters {
        if remaining == 0 {
            break;
        }
        let len = g.cluster_size.min(remaining);
        remaining = remaining.saturating_sub(len);
        match (prev, out.last_mut()) {
            (Some(p), Some(last)) if p.checked_add(1) == Some(c) => last.len = last.len.saturating_add(len),
            _ => out.push(Extent { offset: Some(g.cluster_offset(c)), len }),
        }
        prev = Some(c);
    }
    out
}

/// The live volume-label entry in a root directory (attribute 0x08, not a long-name entry).
fn root_label(buf: &[u8]) -> Option<String> {
    buf.chunks_exact(32)
        .take_while(|e| e[0] != 0)
        .find(|e| e[0] != 0xE5 && e[11] & 0x3F != 0x0F && e[11] & 0x08 != 0 && e[11] & 0x10 == 0)
        .map(|e| String::from_utf8_lossy(&e[0..11]).trim_end().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn sfn_checksum(name: &[u8]) -> u8 {
    name.iter().take(11).fold(0u8, |sum, &c| ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(c))
}

fn short_name(raw: &[u8], case_flags: u8) -> String {
    let lower = |s: &[u8], on: bool| -> String {
        let t = String::from_utf8_lossy(s).trim_end().to_string();
        if on { t.to_lowercase() } else { t }
    };
    let base = lower(&raw[0..8], case_flags & 0x08 != 0);
    let ext = lower(&raw[8..11], case_flags & 0x10 != 0);
    if ext.is_empty() { base } else { format!("{base}.{ext}") }
}

/// Parse a directory buffer into entries, assembling long names (VFAT) and recovering deleted ones.
fn parse_dir(buf: &[u8], fat32: bool, stats: &mut WalkStats) -> Vec<RawEnt> {
    let mut out = Vec::new();
    // LFN fragments in on-disk order (which is last-part-first), their checksum, deleted state.
    let mut lfn: Vec<[u16; 13]> = Vec::new();
    let mut lfn_sum: Option<u8> = None;
    let mut lfn_deleted = false;

    for (i, e) in buf.chunks_exact(32).enumerate() {
        let b0 = e[0];
        if b0 == 0 {
            break;
        }
        let attr = e[11];
        let deleted = b0 == 0xE5;
        if attr & 0x3F == 0x0F {
            let sum = e[13];
            let new_seq = !deleted && b0 & 0x40 != 0;
            if new_seq || lfn_sum != Some(sum) || lfn_deleted != deleted {
                lfn.clear();
            }
            let mut part = [0u16; 13];
            for (slot, &off) in part.iter_mut().zip(LFN_CHAR_OFFSETS.iter()) {
                *slot = le_u16(e, off).unwrap_or(0);
            }
            lfn.push(part);
            lfn_sum = Some(sum);
            lfn_deleted = deleted;
            continue;
        }

        let mut short = [0u8; 11];
        short.copy_from_slice(&e[0..11]);
        let mut first_known = !deleted;
        if short[0] == 0x05 {
            short[0] = 0xE5; // KANJI lead byte escape
        }
        let mut long: Option<String> = None;
        if let Some(sum) = lfn_sum.filter(|_| !lfn.is_empty() && lfn_deleted == deleted) {
            let ok = if deleted {
                // The first byte was overwritten with 0xE5; any value that reproduces the LFN
                // checksum is the original (and restores the 8.3 name's first character).
                (0x20u8..=0xFF)
                    .find(|&c| {
                        short[0] = c;
                        sfn_checksum(&short) == sum
                    })
                    .is_some()
            } else {
                sfn_checksum(&short) == sum
            };
            if ok {
                first_known = true;
                let units: Vec<u16> = lfn
                    .iter()
                    .rev()
                    .flatten()
                    .copied()
                    .take_while(|&u| u != 0)
                    .filter(|&u| u != 0xFFFF)
                    .collect();
                let s = String::from_utf16_lossy(&units);
                if !s.is_empty() {
                    long = Some(s);
                }
            }
        }
        lfn.clear();
        lfn_sum = None;

        if attr & 0x08 != 0 && attr & 0x10 == 0 {
            continue; // volume label
        }
        // "." and ".." are the only 8.3 names that can start with a dot.
        if short[0] == b'.' {
            continue;
        }
        if !first_known {
            short[0] = b'_';
        }
        let name = long.clone().unwrap_or_else(|| short_name(&short, e[12]));
        if name.is_empty() || name.chars().any(|c| c == '\0' || c == '/') {
            stats.corrupt = stats.corrupt.saturating_add(1);
            continue;
        }
        let hi = if fat32 { u32::from(le_u16(e, 20).unwrap_or(0)) } else { 0 };
        out.push(RawEnt {
            offset: i.saturating_mul(32),
            name,
            name_complete: first_known || long.is_some(),
            is_dir: attr & 0x10 != 0,
            first_cluster: (hi << 16) | u32::from(le_u16(e, 26).unwrap_or(0)),
            size: le_u32(e, 28).unwrap_or(0),
            deleted,
            created_ms: dos_to_ms(le_u16(e, 16).unwrap_or(0), le_u16(e, 14).unwrap_or(0)),
            modified_ms: dos_to_ms(le_u16(e, 24).unwrap_or(0), le_u16(e, 22).unwrap_or(0)),
        });
    }
    out
}

impl<S: BlockSource> FileSystem for Fat<S> {
    fn info(&self) -> &FsInfo {
        &self.info
    }

    fn source(&self) -> &dyn BlockSource {
        &self.src
    }

    fn walk(&self, cancel: &CancelToken, visit: &mut dyn FnMut(Entry) -> Result<()>) -> Result<WalkStats> {
        let mut stats = WalkStats::default();
        let fat32 = self.g.kind == FsKind::Fat32;
        let root = if fat32 { DirLoc::Chain(self.g.root_cluster) } else { DirLoc::FixedRoot };
        let mut visited: HashSet<u32> = HashSet::new();
        if fat32 {
            visited.insert(self.g.root_cluster);
        }
        let mut stack = vec![(root, "/".to_string(), false, 0u32)];

        while let Some((loc, path, deleted_ctx, depth)) = stack.pop() {
            cancel.check()?;
            let (buf, positions) = self.read_dir(&loc)?;
            for raw in parse_dir(&buf, fat32, &mut stats) {
                let deleted = raw.deleted || deleted_ctx;
                let child = join(&path, &raw.name);
                let id = self.entry_abs_offset(&loc, &positions, raw.offset);
                if raw.is_dir {
                    let c = if raw.deleted {
                        self.resolve_deleted_start(&raw)?.map_or(0, |(c, _)| c)
                    } else {
                        raw.first_cluster
                    };
                    if self.g.valid_cluster(c) && depth < MAX_DEPTH && !visited.contains(&c) {
                        let next = if raw.deleted || deleted_ctx {
                            self.looks_like_dir_start(c)?.then_some(DirLoc::FirstCluster(c))
                        } else {
                            Some(DirLoc::Chain(c))
                        };
                        if let Some(next) = next {
                            visited.insert(c);
                            stack.push((next, child.clone(), deleted, depth.saturating_add(1)));
                        }
                    }
                }
                let layout = if raw.is_dir {
                    Layout {
                        extents: Vec::new(),
                        quality: ExtentQuality::Exact,
                        overwritten: None,
                        alt: None,
                    }
                } else {
                    self.file_extents(&raw, deleted_ctx)?
                };
                stats.entries = stats.entries.saturating_add(1);
                if deleted {
                    stats.deleted = stats.deleted.saturating_add(1);
                }
                visit(Entry {
                    id,
                    path: child,
                    name: raw.name,
                    name_complete: raw.name_complete,
                    is_dir: raw.is_dir,
                    size: if raw.is_dir { 0 } else { u64::from(raw.size) },
                    deleted,
                    system: false,
                    created_ms: raw.created_ms,
                    modified_ms: raw.modified_ms,
                    extents: layout.extents,
                    resident: None,
                    quality: layout.quality,
                    overwritten: layout.overwritten,
                    alt_extents: layout.alt,
                })?;
            }
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_checksum_matches_spec_example() {
        // Checksum for "FOO     BAR" computed per the FAT specification's algorithm.
        let mut sum = 0u8;
        for &c in b"FOO     BAR" {
            sum = (if sum & 1 != 0 { 0x80u8 } else { 0 }).wrapping_add(sum >> 1).wrapping_add(c);
        }
        assert_eq!(sfn_checksum(b"FOO     BAR"), sum);
    }

    #[test]
    fn short_names_honour_case_flags() {
        assert_eq!(short_name(b"README  TXT", 0), "README.TXT");
        assert_eq!(short_name(b"README  TXT", 0x18), "readme.txt");
        assert_eq!(short_name(b"NOEXT      ", 0), "NOEXT");
    }

    #[test]
    fn deleted_lfn_recovers_full_name() {
        // Build: LFN entry for "hello.txt" + short entry "HELLO   TXT", then mark both deleted.
        let short = *b"HELLO   TXT";
        let sum = sfn_checksum(&short);
        let mut lfn = [0u8; 32];
        lfn[0] = 0x41;
        lfn[11] = 0x0F;
        lfn[13] = sum;
        let chars: Vec<u16> =
            "hello.txt".encode_utf16().chain([0]).chain(std::iter::repeat(0xFFFF)).take(13).collect();
        for (u, &off) in chars.iter().zip(LFN_CHAR_OFFSETS.iter()) {
            lfn[off..off + 2].copy_from_slice(&u.to_le_bytes());
        }
        let mut sfn = [0u8; 32];
        sfn[..11].copy_from_slice(&short);
        sfn[26] = 5;
        sfn[28] = 100;
        let mut dir = [lfn, sfn].concat();
        let mut stats = WalkStats::default();

        let live = parse_dir(&dir, true, &mut stats);
        assert_eq!((live[0].name.as_str(), live[0].deleted), ("hello.txt", false));

        dir[0] = 0xE5;
        dir[32] = 0xE5;
        let del = parse_dir(&dir, true, &mut stats);
        assert_eq!(del[0].name, "hello.txt");
        assert!(del[0].deleted && del[0].name_complete);

        // Without the LFN the first character is unrecoverable and flagged as such.
        let only = parse_dir(&dir[32..], true, &mut stats);
        assert_eq!(only[0].name, "_ELLO.TXT");
        assert!(!only[0].name_complete);
    }

    #[test]
    fn rejects_implausible_boot_sectors() {
        assert!(Geometry::parse(&[0u8; 512]).is_none());
        let mut b = [0u8; 512];
        b[0] = 0xEB;
        b[510] = 0x55;
        b[511] = 0xAA;
        b[11..13].copy_from_slice(&1000u16.to_le_bytes()); // bad bytes/sector
        assert!(Geometry::parse(&b).is_none());
    }
}
