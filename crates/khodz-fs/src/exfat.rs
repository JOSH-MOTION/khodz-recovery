//! exFAT (Microsoft "exFAT file system specification").

use std::collections::HashSet;

use khodz_core::bytes::{le_u16, le_u32, le_u64, u8_at};
use khodz_core::{CancelToken, Error, Result};
use khodz_io::{BlockSource, CachedSource};

use crate::time::dos_to_ms;
use crate::{Entry, Extent, ExtentQuality, FileSystem, FsInfo, WalkStats, join};

const MAX_DIR_BYTES: u64 = 64 << 20;
const MAX_DEPTH: u32 = 64;

struct Geometry {
    cluster_size: u64,
    fat_offset: u64,
    heap_offset: u64,
    cluster_count: u32,
    root_cluster: u32,
}

impl Geometry {
    fn parse(b: &[u8]) -> Option<Self> {
        if b.get(3..11) != Some(b"EXFAT   ") || b.get(510..512) != Some(&[0x55, 0xAA]) {
            return None;
        }
        let bps_shift = u32::from(u8_at(b, 108)?);
        let spc_shift = u32::from(u8_at(b, 109)?);
        if !(9..=12).contains(&bps_shift) || spc_shift > 25u32.saturating_sub(bps_shift) {
            return None;
        }
        let ss = 1u64 << bps_shift;
        let g = Geometry {
            cluster_size: ss << spc_shift,
            fat_offset: u64::from(le_u32(b, 80)?).checked_mul(ss)?,
            heap_offset: u64::from(le_u32(b, 88)?).checked_mul(ss)?,
            cluster_count: le_u32(b, 92)?,
            root_cluster: le_u32(b, 96)?,
        };
        (g.valid_cluster(g.root_cluster) && u8_at(b, 110)? >= 1).then_some(g)
    }

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && u64::from(c) < u64::from(self.cluster_count) + 2
    }

    fn cluster_offset(&self, c: u32) -> u64 {
        self.heap_offset.saturating_add(u64::from(c.saturating_sub(2)).saturating_mul(self.cluster_size))
    }
}

pub struct ExFat<S> {
    src: CachedSource<S>,
    g: Geometry,
    info: FsInfo,
}

/// One file's directory entry set (File + Stream Extension + File Name entries).
struct EntrySet {
    offset: usize,
    deleted: bool,
    checksum_ok: bool,
    name: String,
    is_dir: bool,
    no_fat_chain: bool,
    first_cluster: u32,
    data_length: u64,
    created_ms: Option<i64>,
    modified_ms: Option<i64>,
}

fn set_checksum(entries: &[u8], restore_in_use: bool) -> u16 {
    let mut sum: u16 = 0;
    for (i, &b) in entries.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        // Deleting clears bit 7 of each entry's type; the checksum was computed with it set.
        let b = if restore_in_use && i.checked_rem(32) == Some(0) { b | 0x80 } else { b };
        sum = (if sum & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(sum >> 1).wrapping_add(u16::from(b));
    }
    sum
}

/// UTF-16 code units at byte offsets 2, 4, … within one 32-byte entry.
fn entry_units(e: &[u8], count: usize) -> impl Iterator<Item = u16> + '_ {
    e.get(2..).unwrap_or_default().chunks_exact(2).take(count).map(|c| u16::from_le_bytes([c[0], c[1]]))
}

fn parse_dir(buf: &[u8], stats: &mut WalkStats) -> (Vec<EntrySet>, Option<String>) {
    let mut out = Vec::new();
    let mut label = None;
    let entries: Vec<&[u8]> = buf.chunks_exact(32).collect();
    let n = entries.len();
    let mut i = 0usize;
    while let Some(&e) = entries.get(i) {
        let t = e[0];
        if t == 0x00 {
            break;
        }
        let in_use = t & 0x80 != 0;
        match t & 0x7F {
            0x03 if in_use => {
                let chars = usize::from(e[1].min(11));
                label = Some(String::from_utf16_lossy(&entry_units(e, chars).collect::<Vec<_>>()));
                i = i.saturating_add(1);
            }
            0x05 => {
                let secondary = usize::from(e[1]);
                let end = i.saturating_add(1).saturating_add(secondary);
                if !(2..=18).contains(&secondary) || end > n {
                    stats.corrupt = stats.corrupt.saturating_add(1);
                    i = i.saturating_add(1);
                    continue;
                }
                let secondaries = &entries[i.saturating_add(1)..end];
                let stream = secondaries[0];
                // Every secondary entry must have the same in-use state and the right type.
                let consistent = stream[0] & 0x7F == 0x40
                    && (stream[0] & 0x80 != 0) == in_use
                    && secondaries[1..].iter().all(|s| s[0] & 0x7F == 0x41 && (s[0] & 0x80 != 0) == in_use);
                if !consistent {
                    stats.corrupt = stats.corrupt.saturating_add(1);
                    i = i.saturating_add(1);
                    continue;
                }
                let name_len = usize::from(stream[3]);
                let units: Vec<u16> =
                    secondaries[1..].iter().flat_map(|s| entry_units(s, 15)).take(name_len).collect();
                let set: Vec<u8> = entries[i..end].concat();
                let stored = le_u16(e, 2).unwrap_or(0);
                out.push(EntrySet {
                    offset: i.saturating_mul(32),
                    deleted: !in_use,
                    checksum_ok: set_checksum(&set, !in_use) == stored,
                    name: String::from_utf16_lossy(&units),
                    is_dir: le_u16(e, 4).unwrap_or(0) & 0x10 != 0,
                    no_fat_chain: stream[1] & 0x02 != 0,
                    first_cluster: le_u32(stream, 20).unwrap_or(0),
                    data_length: le_u64(stream, 24).unwrap_or(0),
                    created_ms: le_u32(e, 8).and_then(|ts| dos_to_ms((ts >> 16) as u16, ts as u16)),
                    modified_ms: le_u32(e, 12).and_then(|ts| dos_to_ms((ts >> 16) as u16, ts as u16)),
                });
                i = end;
            }
            _ => i = i.saturating_add(1),
        }
    }
    (out, label)
}

impl<S: BlockSource> ExFat<S> {
    pub fn open(src: S) -> Result<Self> {
        let mut info = crate::detect(&src)?;
        let mut boot = [0u8; 512];
        src.read_exact_at(0, &mut boot)?;
        let g =
            Geometry::parse(&boot).ok_or_else(|| Error::InvalidData("invalid exFAT boot sector".into()))?;
        let src = CachedSource::new(src, 64 << 10, 128)?;
        let mut fs = Self { src, g, info: info.clone() };
        // The volume label lives in the root directory.
        let (buf, _) = fs.read_chain_or_contiguous(fs.g.root_cluster, false, MAX_DIR_BYTES)?;
        let (_, label) = parse_dir(&buf, &mut WalkStats::default());
        info.label = label.filter(|l| !l.is_empty());
        fs.info = info;
        Ok(fs)
    }

    fn fat_entry(&self, c: u32) -> Result<u32> {
        let mut b = [0u8; 4];
        self.src.read_exact_at(self.g.fat_offset.saturating_add(u64::from(c).saturating_mul(4)), &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn chain(&self, start: u32, limit: u64) -> Result<(Vec<u32>, bool)> {
        let mut out = Vec::new();
        let mut c = start;
        while self.g.valid_cluster(c) {
            out.push(c);
            if out.len() as u64 >= limit {
                return Ok((out, false));
            }
            let next = self.fat_entry(c)?;
            if next == 0xFFFF_FFFF {
                return Ok((out, true));
            }
            c = next;
        }
        Ok((out, false))
    }

    /// Read up to `max` bytes of a directory's data.
    fn read_chain_or_contiguous(
        &self,
        first: u32,
        contiguous: bool,
        max: u64,
    ) -> Result<(Vec<u8>, Vec<u64>)> {
        let cs = self.g.cluster_size;
        let limit = max.checked_div(cs).unwrap_or(1).max(1);
        let clusters: Vec<u32> = if contiguous {
            (0..limit)
                .map_while(|k| first.checked_add(u32::try_from(k).ok()?))
                .filter(|&c| self.g.valid_cluster(c))
                .collect()
        } else {
            self.chain(first, limit)?.0
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

    fn extents(&self, s: &EntrySet) -> Result<(Vec<Extent>, ExtentQuality)> {
        let size = s.data_length;
        if size == 0 {
            return Ok((Vec::new(), ExtentQuality::Exact));
        }
        if !self.g.valid_cluster(s.first_cluster) {
            return Ok((Vec::new(), ExtentQuality::Partial));
        }
        let cs = self.g.cluster_size;
        let needed = size.div_ceil(cs);
        let fits = u64::from(s.first_cluster).saturating_add(needed) <= u64::from(self.g.cluster_count) + 2;
        if s.no_fat_chain || s.deleted {
            if !fits {
                return Ok((Vec::new(), ExtentQuality::Partial));
            }
            // NoFatChain means the allocation *is* contiguous; for deleted FAT-chained files the
            // chain may be stale, so contiguity is only assumed.
            let q = if s.no_fat_chain || needed == 1 {
                ExtentQuality::Exact
            } else {
                ExtentQuality::ContiguityAssumed
            };
            return Ok((vec![Extent { offset: Some(self.g.cluster_offset(s.first_cluster)), len: size }], q));
        }
        let (clusters, _) = self.chain(s.first_cluster, needed)?;
        let mut out: Vec<Extent> = Vec::new();
        let mut remaining = size;
        let mut prev: Option<u32> = None;
        for &c in &clusters {
            let len = cs.min(remaining);
            remaining = remaining.saturating_sub(len);
            match (prev, out.last_mut()) {
                (Some(p), Some(last)) if p.checked_add(1) == Some(c) => {
                    last.len = last.len.saturating_add(len)
                }
                _ => out.push(Extent { offset: Some(self.g.cluster_offset(c)), len }),
            }
            prev = Some(c);
            if remaining == 0 {
                break;
            }
        }
        let q = if remaining == 0 { ExtentQuality::Exact } else { ExtentQuality::Partial };
        Ok((out, q))
    }
}

impl<S: BlockSource> FileSystem for ExFat<S> {
    fn info(&self) -> &FsInfo {
        &self.info
    }

    fn source(&self) -> &dyn BlockSource {
        &self.src
    }

    fn walk(&self, cancel: &CancelToken, visit: &mut dyn FnMut(Entry) -> Result<()>) -> Result<WalkStats> {
        let mut stats = WalkStats::default();
        let mut visited = HashSet::from([self.g.root_cluster]);
        // (first cluster, contiguous, byte limit, path, deleted context, depth)
        let mut stack = vec![(self.g.root_cluster, false, MAX_DIR_BYTES, "/".to_string(), false, 0u32)];
        let cs = self.g.cluster_size.max(1);

        while let Some((first, contiguous, limit, path, deleted_ctx, depth)) = stack.pop() {
            cancel.check()?;
            let (buf, positions) = self.read_chain_or_contiguous(first, contiguous, limit)?;
            let (sets, _) = parse_dir(&buf, &mut stats);
            for s in sets {
                if !s.checksum_ok {
                    stats.corrupt = stats.corrupt.saturating_add(1);
                    if !s.deleted {
                        continue;
                    }
                    // Deleted sets with a bad checksum were partially overwritten: still worth
                    // reporting, but never trusted for directory traversal.
                }
                let deleted = s.deleted || deleted_ctx;
                let child = join(&path, &s.name);
                if s.is_dir
                    && s.checksum_ok
                    && depth < MAX_DEPTH
                    && self.g.valid_cluster(s.first_cluster)
                    && visited.insert(s.first_cluster)
                {
                    let limit = s.data_length.clamp(cs, MAX_DIR_BYTES);
                    let contiguous = s.no_fat_chain || s.deleted;
                    stack.push((
                        s.first_cluster,
                        contiguous,
                        limit,
                        child.clone(),
                        deleted,
                        depth.saturating_add(1),
                    ));
                }
                let (extents, quality) =
                    if s.is_dir { (Vec::new(), ExtentQuality::Exact) } else { self.extents(&s)? };
                let off = s.offset as u64;
                let id = positions
                    .get(usize::try_from(off.checked_div(cs).unwrap_or(0)).unwrap_or(usize::MAX))
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(off.checked_rem(cs).unwrap_or(0));
                stats.entries = stats.entries.saturating_add(1);
                if deleted {
                    stats.deleted = stats.deleted.saturating_add(1);
                }
                visit(Entry {
                    id,
                    path: child,
                    name: s.name,
                    name_complete: true,
                    is_dir: s.is_dir,
                    size: if s.is_dir { 0 } else { s.data_length },
                    deleted,
                    system: false,
                    created_ms: s.created_ms,
                    modified_ms: s.modified_ms,
                    extents,
                    resident: None,
                    quality,
                    overwritten: None,
                    alt_extents: None,
                })?;
            }
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_set(name: &str, in_use: bool, first: u32, len: u64, no_fat_chain: bool) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_entries = units.len().div_ceil(15);
        let mut set = vec![0u8; 32 * (2 + name_entries)];
        set[0] = 0x85;
        set[1] = (1 + name_entries) as u8;
        set[32] = 0xC0;
        set[33] = 0x01 | if no_fat_chain { 0x02 } else { 0 };
        set[35] = units.len() as u8;
        set[32 + 20..32 + 24].copy_from_slice(&first.to_le_bytes());
        set[32 + 24..32 + 32].copy_from_slice(&len.to_le_bytes());
        for (k, u) in units.iter().enumerate() {
            let e = 64 + (k / 15) * 32;
            set[e] = 0xC1;
            let o = e + 2 + (k % 15) * 2;
            set[o..o + 2].copy_from_slice(&u.to_le_bytes());
        }
        let sum = set_checksum(&set, false);
        set[2..4].copy_from_slice(&sum.to_le_bytes());
        if !in_use {
            for e in set.chunks_exact_mut(32) {
                e[0] &= 0x7F;
            }
        }
        set
    }

    #[test]
    fn parses_live_and_deleted_entry_sets_with_checksums() {
        let mut dir = file_set("A long exFAT name that needs two name entries.bin", true, 5, 70_000, true);
        dir.extend(file_set("deleted.txt", false, 9, 10, false));
        let mut stats = WalkStats::default();
        let (sets, _) = parse_dir(&dir, &mut stats);
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].name, "A long exFAT name that needs two name entries.bin");
        assert!(sets[0].checksum_ok && !sets[0].deleted && sets[0].no_fat_chain);
        assert_eq!(
            (sets[1].name.as_str(), sets[1].deleted, sets[1].checksum_ok),
            ("deleted.txt", true, true)
        );
        assert_eq!(stats.corrupt, 0);
    }

    #[test]
    fn corrupted_sets_are_flagged() {
        let mut dir = file_set("x.bin", true, 5, 10, false);
        dir[64 + 2] ^= 1; // flip a name character: checksum must fail
        let (sets, _) = parse_dir(&dir, &mut WalkStats::default());
        assert!(!sets[0].checksum_ok);

        let mut broken = file_set("y.bin", true, 5, 10, false);
        broken[32] = 0xC1; // stream extension replaced by a name entry
        let mut stats = WalkStats::default();
        assert!(parse_dir(&broken, &mut stats).0.is_empty());
        assert_eq!(stats.corrupt, 1);
    }
}
