use khodz_core::Result;
use khodz_core::bytes::{be_u16, be_u32, le_u16, le_u32, le_u64, u8_at};
use khodz_io::BlockSource;

use crate::{FsInfo, FsKind};

fn info(kind: FsKind) -> FsInfo {
    FsInfo {
        kind,
        label: None,
        serial: None,
        cluster_size: None,
        sector_size: None,
        volume_size: None,
        encrypted: false,
        notes: Vec::new(),
    }
}

fn trimmed_ascii(b: &[u8]) -> Option<String> {
    let s: String =
        b.iter().map(|&c| if c.is_ascii_graphic() || c == b' ' { c as char } else { ' ' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() || s == "NO NAME" { None } else { Some(s) }
}

/// Identify the filesystem at the start of `src`. Never fails on unrecognised content: that
/// is reported as [`FsKind::Unknown`].
pub fn detect<S: BlockSource + ?Sized>(src: &S) -> Result<FsInfo> {
    let mut head = vec![0u8; 4096];
    let n = src.read_exact_at(0, &mut head)?;
    head.truncate(n);
    if head.len() < 512 {
        return Ok(info(FsKind::Unknown));
    }

    let oem = head.get(3..11).unwrap_or_default();
    if oem == b"-FVE-FS-" {
        let mut i = info(FsKind::BitLocker);
        i.encrypted = true;
        i.notes.push("BitLocker-encrypted; unlock in Windows to read files".into());
        return Ok(i);
    }
    if oem == b"NTFS    " {
        return Ok(ntfs_info(&head));
    }
    if oem == b"EXFAT   " {
        return Ok(exfat_info(&head));
    }
    if let Some(i) = fat_info(&head) {
        return Ok(i);
    }
    // APFS container superblock: object header (32 bytes) then "NXSB".
    if head.get(32..36) == Some(b"NXSB") {
        let mut i = info(FsKind::Apfs);
        i.cluster_size = le_u32(&head, 36);
        i.volume_size = le_u64(&head, 40).and_then(|b| b.checked_mul(u64::from(i.cluster_size?)));
        i.notes.push("APFS container detected; volume and file walking is not implemented yet".into());
        return Ok(i);
    }
    // HFS+/HFSX volume header at byte 1024 (big-endian).
    if head.len() >= 1536 {
        let vh = &head[1024..];
        let sig = be_u16(vh, 0);
        let kind = match sig {
            Some(0x482B) => Some(FsKind::HfsPlus),
            Some(0x4858) => Some(FsKind::Hfsx),
            _ => None,
        };
        if let Some(kind) = kind {
            let mut i = info(kind);
            i.cluster_size = be_u32(vh, 40);
            i.volume_size = be_u32(vh, 44).and_then(|b| u64::from(b).checked_mul(u64::from(i.cluster_size?)));
            i.notes.push("HFS+ detected; catalog walking is not implemented yet".into());
            return Ok(i);
        }
        if le_u16(&head, 1024 + 56) == Some(0xEF53) {
            let mut i = info(FsKind::Ext);
            i.notes.push("Linux ext2/3/4 is not supported".into());
            return Ok(i);
        }
    }
    Ok(info(FsKind::Unknown))
}

fn ntfs_info(b: &[u8]) -> FsInfo {
    let mut i = info(FsKind::Ntfs);
    let bps = le_u16(b, 11).map(u32::from);
    let spc_raw = u8_at(b, 13).unwrap_or(0);
    // Values above 0x80 encode 2^(256 - v) (very large clusters).
    let spc = if spc_raw > 0x80 {
        1u32.checked_shl(256u32.saturating_sub(u32::from(spc_raw)))
    } else {
        Some(u32::from(spc_raw))
    };
    i.sector_size = bps;
    i.cluster_size = bps.zip(spc).and_then(|(a, b)| a.checked_mul(b));
    i.volume_size = le_u64(b, 40).zip(bps).and_then(|(s, b)| s.checked_mul(u64::from(b)));
    i.serial = le_u64(b, 72).map(|s| format!("{s:016X}"));
    i
}

fn exfat_info(b: &[u8]) -> FsInfo {
    let mut i = info(FsKind::ExFat);
    let bps_shift = u8_at(b, 108).unwrap_or(0);
    let spc_shift = u8_at(b, 109).unwrap_or(0);
    let bps = 1u32.checked_shl(u32::from(bps_shift));
    i.sector_size = bps;
    i.cluster_size = bps.and_then(|s| s.checked_shl(u32::from(spc_shift)));
    i.volume_size = le_u64(b, 72).zip(bps).and_then(|(n, s)| n.checked_mul(u64::from(s)));
    i.serial = le_u32(b, 100).map(|s| format!("{:04X}-{:04X}", s >> 16, s & 0xFFFF));
    i
}

/// FAT is identified by a plausible BPB; the variant follows from the cluster count (per the
/// Microsoft FAT specification), not from the informational type string.
fn fat_info(b: &[u8]) -> Option<FsInfo> {
    let g = crate::fat::Geometry::parse(b)?;
    let mut i = info(g.kind);
    i.sector_size = Some(g.bytes_per_sector);
    i.cluster_size = u32::try_from(g.cluster_size).ok();
    i.volume_size = Some(g.volume_size);
    let (label_off, serial_off): (usize, usize) = if g.kind == FsKind::Fat32 { (71, 67) } else { (43, 39) };
    i.label = b.get(label_off..label_off.saturating_add(11)).and_then(trimmed_ascii);
    i.serial = le_u32(b, serial_off).map(|s| format!("{:04X}-{:04X}", s >> 16, s & 0xFFFF));
    Some(i)
}
