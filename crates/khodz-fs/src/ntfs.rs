//! NTFS: enumerate every MFT record (in-use and deleted) and rebuild paths from `$FILE_NAME`
//! parent references. Directory indexes are not used, so damaged index B-trees cannot hide files.
//!
//! Structure layouts follow the publicly documented NTFS on-disk format (as used by the Linux
//! `ntfs3` driver and libfsntfs documentation).

use std::collections::HashMap;

use khodz_core::bytes::{le_u16, le_u32, le_u64, u8_at, utf16le};
use khodz_core::{CancelToken, Error, Result};
use khodz_io::{BlockSource, CachedSource};

use crate::time::filetime_to_ms;
use crate::{Entry, Extent, ExtentQuality, FileSystem, FsInfo, WalkStats};

const ROOT_RECORD: u64 = 5;
const EXTEND_RECORD: u64 = 11;
const MAX_RUNS: usize = 1 << 20;
const MAX_DEPTH: usize = 256;
/// Fixups are applied per 512-byte stride regardless of the volume's sector size.
const FIXUP_STRIDE: usize = 512;

const AT_STANDARD_INFORMATION: u32 = 0x10;
const AT_ATTRIBUTE_LIST: u32 = 0x20;
const AT_FILE_NAME: u32 = 0x30;
const AT_VOLUME_NAME: u32 = 0x60;
const AT_DATA: u32 = 0x80;
const AT_END: u32 = 0xFFFF_FFFF;

/// A decoded data run: `lcn == None` for a sparse run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    pub lcn: Option<u64>,
    pub clusters: u64,
}

/// Decode an NTFS runlist (mapping pairs).
pub fn decode_runs(b: &[u8]) -> std::result::Result<Vec<Run>, String> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut lcn: i64 = 0;
    loop {
        let header = *b.get(pos).ok_or("runlist ends without terminator")?;
        if header == 0 {
            return Ok(runs);
        }
        let len_size = usize::from(header & 0x0F);
        let off_size = usize::from(header >> 4);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return Err(format!("invalid run header {header:#04x}"));
        }
        let start = pos.checked_add(1).ok_or("overflow")?;
        let len_bytes =
            b.get(start..start.checked_add(len_size).ok_or("overflow")?).ok_or("truncated run")?;
        let clusters = len_bytes.iter().rev().fold(0u64, |acc, &x| (acc << 8) | u64::from(x));
        let off_start = start.saturating_add(len_size);
        let off_bytes =
            b.get(off_start..off_start.checked_add(off_size).ok_or("overflow")?).ok_or("truncated run")?;
        if clusters == 0 {
            return Err("zero-length run".into());
        }
        if off_size == 0 {
            runs.push(Run { lcn: None, clusters });
        } else {
            let mut delta = off_bytes.iter().rev().fold(0i64, |acc, &x| (acc << 8) | i64::from(x));
            let shift = 64u32.saturating_sub(u32::try_from(off_size.saturating_mul(8)).unwrap_or(64));
            if shift < 64 {
                delta = (delta << shift) >> shift; // sign-extend
            }
            lcn = lcn.checked_add(delta).ok_or("LCN overflow")?;
            let abs = u64::try_from(lcn).map_err(|_| "negative LCN".to_string())?;
            runs.push(Run { lcn: Some(abs), clusters });
        }
        if runs.len() > MAX_RUNS {
            return Err("too many runs".into());
        }
        pos = off_start.saturating_add(off_size);
    }
}

/// Apply the update-sequence fixups in place. Returns false for a torn/corrupt record.
fn apply_fixups(rec: &mut [u8]) -> bool {
    let (Some(usa_off), Some(usa_count)) = (le_u16(rec, 4), le_u16(rec, 6)) else { return false };
    let (usa_off, usa_count) = (usize::from(usa_off), usize::from(usa_count));
    if usa_count < 2 || usa_off.saturating_add(usa_count.saturating_mul(2)) > rec.len() {
        return false;
    }
    // Bounds: usa_off + 2*usa_count <= len was checked above.
    let usn = [rec[usa_off], rec[usa_off.saturating_add(1)]];
    for i in 1..usa_count {
        let end = i.saturating_mul(FIXUP_STRIDE);
        if end > rec.len() || end < 2 {
            return false;
        }
        let tail = end.saturating_sub(2);
        if rec[tail..end] != usn {
            return false;
        }
        let fix = usa_off.saturating_add(i.saturating_mul(2));
        let (a, b) = (rec[fix], rec[fix.saturating_add(1)]);
        rec[tail] = a;
        rec[tail.saturating_add(1)] = b;
    }
    true
}

struct Attr<'a> {
    kind: u32,
    named: bool,
    resident: Option<&'a [u8]>,
    /// Non-resident: (runlist bytes, data size).
    nonresident: Option<(&'a [u8], u64)>,
    start_vcn: u64,
}

fn attributes(rec: &[u8]) -> Vec<Attr<'_>> {
    let mut out = Vec::new();
    let used = le_u32(rec, 24).map_or(0, |u| usize::try_from(u).unwrap_or(0)).min(rec.len());
    let mut off = le_u16(rec, 20).map_or(usize::MAX, usize::from);
    while off.saturating_add(16) <= used {
        let Some(kind) = le_u32(rec, off) else { break };
        if kind == AT_END {
            break;
        }
        let len = le_u32(rec, off.saturating_add(4)).map_or(0, |l| usize::try_from(l).unwrap_or(0));
        let end = off.saturating_add(len);
        if len < 16 || end > used {
            break;
        }
        let a = &rec[off..end];
        let named = u8_at(a, 9).unwrap_or(0) != 0;
        let nonres = u8_at(a, 8).unwrap_or(0) != 0;
        let mut attr = Attr { kind, named, resident: None, nonresident: None, start_vcn: 0 };
        if nonres {
            let rl = le_u16(a, 32).map_or(usize::MAX, usize::from);
            if let (Some(runs), Some(size), Some(vcn)) = (a.get(rl..), le_u64(a, 48), le_u64(a, 16)) {
                attr.nonresident = Some((runs, size));
                attr.start_vcn = vcn;
            }
        } else {
            let vlen = le_u32(a, 16).map_or(0, |l| usize::try_from(l).unwrap_or(0));
            let voff = le_u16(a, 20).map_or(usize::MAX, usize::from);
            attr.resident = a.get(voff..voff.saturating_add(vlen));
        }
        out.push(attr);
        off = off.saturating_add(len);
    }
    out
}

/// What we keep per MFT record for path reconstruction and entry emission.
struct Node {
    in_use: bool,
    is_dir: bool,
    seq: u16,
    parent: u64,
    parent_seq: u16,
    name: String,
    size: u64,
    created_ms: Option<i64>,
    modified_ms: Option<i64>,
    extents: Vec<Extent>,
    resident: Option<Vec<u8>>,
    quality: ExtentQuality,
}

pub struct Ntfs<S> {
    src: CachedSource<S>,
    info: FsInfo,
    cluster_size: u64,
    record_size: usize,
    mft_runs: Vec<Run>,
    record_count: u64,
}

impl<S: BlockSource> Ntfs<S> {
    pub fn open(src: S) -> Result<Self> {
        let mut info = crate::detect(&src)?;
        let mut boot = [0u8; 512];
        src.read_exact_at(0, &mut boot)?;
        let bad = |m: &str| Error::InvalidData(format!("NTFS boot sector: {m}"));
        let cluster_size = u64::from(info.cluster_size.ok_or_else(|| bad("cluster size"))?);
        if cluster_size == 0 || !cluster_size.is_power_of_two() || cluster_size > (2 << 20) {
            return Err(bad("implausible cluster size"));
        }
        let mft_lcn = le_u64(&boot, 48).ok_or_else(|| bad("MFT location"))?;
        let cpr = u8_at(&boot, 64).ok_or_else(|| bad("record size"))? as i8;
        let record_size = if cpr < 0 {
            1usize.checked_shl(u32::from(cpr.unsigned_abs())).ok_or_else(|| bad("record size"))?
        } else {
            usize::try_from(u64::from(cpr.unsigned_abs()).saturating_mul(cluster_size)).unwrap_or(0)
        };
        if !(512..=65536).contains(&record_size) || !record_size.is_power_of_two() {
            return Err(bad("implausible MFT record size"));
        }
        let src = CachedSource::new(src, 256 << 10, 64)?;

        // Record 0 ($MFT) describes where the rest of the MFT lives.
        let mut rec0 = vec![0u8; record_size];
        let mft_off = mft_lcn.checked_mul(cluster_size).ok_or_else(|| bad("MFT offset"))?;
        src.read_exact_at(mft_off, &mut rec0)?;
        if &rec0[0..4] != b"FILE" || !apply_fixups(&mut rec0) {
            return Err(Error::InvalidData(
                "$MFT record 0 is damaged (try the mirror; not implemented yet)".into(),
            ));
        }
        let (runs, size) = attributes(&rec0)
            .into_iter()
            .find(|a| a.kind == AT_DATA && !a.named)
            .and_then(|a| a.nonresident)
            .ok_or_else(|| Error::InvalidData("$MFT has no $DATA runs".into()))?;
        let mft_runs = decode_runs(runs).map_err(|e| Error::InvalidData(format!("$MFT runlist: {e}")))?;

        let mut fs = Self {
            src,
            info: info.clone(),
            cluster_size,
            record_size,
            mft_runs,
            record_count: size.checked_div(record_size as u64).unwrap_or(0),
        };
        // Volume label from $Volume (record 3).
        if let Ok(Some(rec3)) = fs.read_record(3) {
            info.label = attributes(&rec3)
                .iter()
                .find(|a| a.kind == AT_VOLUME_NAME)
                .and_then(|a| a.resident)
                .map(utf16le)
                .filter(|s| !s.is_empty());
        }
        fs.info = info;
        Ok(fs)
    }

    /// Byte offset of MFT record `n`, following the $MFT's own runs.
    fn record_offset(&self, n: u64) -> Option<u64> {
        let mut byte = n.checked_mul(self.record_size as u64)?;
        for r in &self.mft_runs {
            let run_bytes = r.clusters.checked_mul(self.cluster_size)?;
            if byte < run_bytes {
                return r.lcn?.checked_mul(self.cluster_size)?.checked_add(byte);
            }
            byte = byte.saturating_sub(run_bytes);
        }
        None
    }

    /// Read and fix up record `n`. `Ok(None)` for unused (never-initialised) or torn records.
    fn read_record(&self, n: u64) -> Result<Option<Vec<u8>>> {
        let Some(off) = self.record_offset(n) else { return Ok(None) };
        let mut rec = vec![0u8; self.record_size];
        if self.src.read_exact_at(off, &mut rec)? < rec.len()
            || &rec[0..4] != b"FILE"
            || !apply_fixups(&mut rec)
        {
            return Ok(None);
        }
        Ok(Some(rec))
    }

    fn runs_to_extents(&self, runs: &[Run], size: u64) -> Vec<Extent> {
        let mut out = Vec::new();
        let mut remaining = size;
        for r in runs {
            if remaining == 0 {
                break;
            }
            let len = r.clusters.saturating_mul(self.cluster_size).min(remaining);
            remaining = remaining.saturating_sub(len);
            out.push(Extent { offset: r.lcn.map(|l| l.saturating_mul(self.cluster_size)), len });
        }
        out
    }

    fn parse_node(&self, rec: &[u8]) -> Option<Node> {
        let flags = le_u16(rec, 22)?;
        let base = le_u64(rec, 32)?;
        if base & 0x0000_FFFF_FFFF_FFFF != 0 {
            return None; // extension record; its attributes belong to another record
        }
        let attrs = attributes(rec);
        // Prefer the Win32 name over the DOS 8.3 alias; POSIX names are also accepted.
        let name_attr = attrs
            .iter()
            .filter(|a| a.kind == AT_FILE_NAME)
            .filter_map(|a| a.resident)
            .filter(|v| v.len() >= 66)
            .min_by_key(|v| match v[65] {
                1 | 3 => 0,
                0 => 1,
                _ => 2,
            })?;
        let name_len = usize::from(name_attr[64]);
        let name = utf16le(name_attr.get(66..66usize.saturating_add(name_len.saturating_mul(2)))?);
        let parent_ref = le_u64(name_attr, 0)?;
        let si = attrs.iter().find(|a| a.kind == AT_STANDARD_INFORMATION).and_then(|a| a.resident);
        let is_dir = flags & 0x02 != 0;

        let data = attrs.iter().find(|a| a.kind == AT_DATA && !a.named);
        let has_list = attrs.iter().any(|a| a.kind == AT_ATTRIBUTE_LIST);
        let (size, extents, resident, quality) = match data {
            Some(Attr { resident: Some(v), .. }) => {
                (v.len() as u64, Vec::new(), Some(v.to_vec()), ExtentQuality::Exact)
            }
            Some(Attr { nonresident: Some((rl, size)), start_vcn, .. }) => match decode_runs(rl) {
                // A non-zero starting VCN means the rest of the runs live in extension records.
                Ok(runs) if *start_vcn == 0 => {
                    let ext = self.runs_to_extents(&runs, *size);
                    let covered: u64 = ext.iter().map(|e| e.len).sum();
                    let q = if covered >= *size && !has_list {
                        ExtentQuality::Exact
                    } else {
                        ExtentQuality::Partial
                    };
                    (*size, ext, None, q)
                }
                _ => (*size, Vec::new(), None, ExtentQuality::Partial),
            },
            // No $DATA in the base record: it is in an extension record via $ATTRIBUTE_LIST.
            _ if has_list && !is_dir => (0, Vec::new(), None, ExtentQuality::Partial),
            _ => (0, Vec::new(), None, ExtentQuality::Exact),
        };
        Some(Node {
            in_use: flags & 0x01 != 0,
            is_dir,
            seq: le_u16(rec, 16)?,
            parent: parent_ref & 0x0000_FFFF_FFFF_FFFF,
            parent_seq: (parent_ref >> 48) as u16,
            name,
            size,
            created_ms: si.and_then(|v| le_u64(v, 0)).and_then(filetime_to_ms),
            modified_ms: si.and_then(|v| le_u64(v, 8)).and_then(filetime_to_ms),
            extents,
            resident,
            quality,
        })
    }
}

/// Resolve a record's path from parent links. Deleted files whose parent was reused or lost go
/// under `/$Orphan`.
fn resolve_path(nodes: &HashMap<u64, Node>, cache: &mut HashMap<u64, String>, n: u64) -> String {
    let mut chain: Vec<String> = Vec::new();
    let mut cur = n;
    let mut orphan = false;
    for _ in 0..MAX_DEPTH {
        if cur == ROOT_RECORD {
            break;
        }
        if let Some(p) = cache.get(&cur) {
            let mut path = p.clone();
            for name in chain.iter().rev() {
                path = crate::join(&path, name);
            }
            return path;
        }
        let Some(node) = nodes.get(&cur) else {
            orphan = true;
            break;
        };
        chain.push(node.name.clone());
        let parent_ok = nodes.get(&node.parent).is_some_and(|p| p.seq == node.parent_seq || !p.in_use)
            || node.parent == ROOT_RECORD;
        if !parent_ok {
            orphan = true;
            break;
        }
        cur = node.parent;
    }
    let mut path = if orphan { "/$Orphan".to_string() } else { "/".to_string() };
    for name in chain.iter().rev() {
        path = crate::join(&path, name);
    }
    if nodes.get(&n).is_some_and(|x| x.is_dir) {
        cache.insert(n, path.clone());
    }
    path
}

impl<S: BlockSource> FileSystem for Ntfs<S> {
    fn info(&self) -> &FsInfo {
        &self.info
    }

    fn source(&self) -> &dyn BlockSource {
        &self.src
    }

    fn walk(&self, cancel: &CancelToken, visit: &mut dyn FnMut(Entry) -> Result<()>) -> Result<WalkStats> {
        let mut stats = WalkStats::default();
        let mut nodes: HashMap<u64, Node> = HashMap::new();
        for n in 0..self.record_count {
            if n % 4096 == 0 {
                cancel.check()?;
            }
            let Some(rec) = self.read_record(n)? else { continue };
            match self.parse_node(&rec) {
                Some(node) => {
                    nodes.insert(n, node);
                }
                None => {
                    // Extension records and nameless reserved records (MFT 12–23 are marked
                    // in use without names) are normal. A base record that *has* a $FILE_NAME we
                    // could not decode is damage.
                    let base = le_u64(&rec, 32).unwrap_or(0) & 0x0000_FFFF_FFFF_FFFF;
                    if base == 0 && attributes(&rec).iter().any(|a| a.kind == AT_FILE_NAME) {
                        stats.corrupt = stats.corrupt.saturating_add(1);
                    }
                }
            }
        }

        // Windows 10 1903+ deletes with POSIX semantics: a file deleted while another handle is
        // open is renamed into \$Extend\$Deleted (record name replaced by an ID) and only freed
        // when the last handle closes. Such records are still "in use" but are deleted files,
        // with intact data and lost names.
        let pending_dir = nodes
            .iter()
            .find(|(_, n)| n.is_dir && n.parent == EXTEND_RECORD && n.name == "$Deleted")
            .map(|(id, _)| *id);

        let mut cache: HashMap<u64, String> = HashMap::from([(ROOT_RECORD, "/".to_string())]);
        let mut ids: Vec<u64> = nodes.keys().copied().collect();
        ids.sort_unstable();
        for n in ids {
            if n == ROOT_RECORD {
                continue;
            }
            cancel.check()?;
            let path = resolve_path(&nodes, &mut cache, n);
            let Some(node) = nodes.get(&n) else { continue };
            let pending_delete = pending_dir.is_some_and(|d| node.parent == d && Some(n) != pending_dir);
            let deleted = !node.in_use || pending_delete;
            let system = !pending_delete
                && (n < 16
                    || (node.parent == ROOT_RECORD && node.name.starts_with('$'))
                    || path.starts_with("/$Extend"));
            stats.entries = stats.entries.saturating_add(1);
            if deleted {
                stats.deleted = stats.deleted.saturating_add(1);
            }
            visit(Entry {
                id: n,
                path,
                name: node.name.clone(),
                name_complete: !pending_delete,
                is_dir: node.is_dir,
                size: if node.is_dir { 0 } else { node.size },
                deleted,
                system,
                created_ms: node.created_ms,
                modified_ms: node.modified_ms,
                extents: node.extents.clone(),
                resident: node.resident.clone(),
                quality: node.quality,
            })?;
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_runlists_including_negative_and_sparse_runs() {
        // 0x21: 1-byte length, 2-byte offset. Run 1: 0x18 clusters at LCN 0x5634.
        // Run 2 (0x11): 1-byte length 0x10, offset -0x10 (0xF0) relative => 0x5624.
        // Run 3 (0x01): sparse run of 4 clusters.
        let rl = [0x21, 0x18, 0x34, 0x56, 0x11, 0x10, 0xF0, 0x01, 0x04, 0x00];
        assert_eq!(
            decode_runs(&rl).unwrap(),
            vec![
                Run { lcn: Some(0x5634), clusters: 0x18 },
                Run { lcn: Some(0x5624), clusters: 0x10 },
                Run { lcn: None, clusters: 4 },
            ]
        );
    }

    #[test]
    fn rejects_hostile_runlists() {
        assert!(decode_runs(&[0x21, 0x18]).is_err(), "truncated");
        assert!(decode_runs(&[0x91, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err(), "9-byte offset");
        assert!(decode_runs(&[0x11, 0x05, 0x80, 0x00]).is_err(), "negative LCN");
        assert!(decode_runs(&[0x11, 0x05]).is_err(), "no terminator");
        assert!(decode_runs(&[]).is_err());
    }

    #[test]
    fn fixups_detect_torn_records() {
        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(b"FILE");
        rec[4..6].copy_from_slice(&48u16.to_le_bytes());
        rec[6..8].copy_from_slice(&3u16.to_le_bytes());
        rec[48..50].copy_from_slice(&[0xAB, 0xCD]); // USN
        rec[50..52].copy_from_slice(&[1, 2]); // original bytes of sector 1 end
        rec[52..54].copy_from_slice(&[3, 4]); // original bytes of sector 2 end
        rec[510..512].copy_from_slice(&[0xAB, 0xCD]);
        rec[1022..1024].copy_from_slice(&[0xAB, 0xCD]);
        let mut good = rec.clone();
        assert!(apply_fixups(&mut good));
        assert_eq!(&good[510..512], &[1, 2]);
        assert_eq!(&good[1022..1024], &[3, 4]);

        rec[1022] = 0; // sector 2 written but sector 1 not: torn
        assert!(!apply_fixups(&mut rec));
    }
}
