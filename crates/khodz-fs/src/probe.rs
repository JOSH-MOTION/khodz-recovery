//! Boot-record probing for partition analysis: recognise a filesystem's boot record (or volume
//! header) in a single sector, say how large the volume it describes is and where its backup
//! copy lives, and check that the structure it points to is really there.
//!
//! Used to find volumes whose partition-table entry is gone (lost partitions) and to open
//! volumes whose primary boot sector is damaged but whose backup survives.

use khodz_core::Result;
use khodz_core::bytes::{be_u16, be_u32, le_u16, le_u32, le_u64, u8_at};
use khodz_io::BlockSource;

use crate::FsKind;

/// What one boot record says about the volume it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootRecord {
    pub kind: FsKind,
    /// Bytes per sector declared by the filesystem.
    pub sector_size: u32,
    /// Bytes the volume occupies (the extent a partition must cover).
    pub volume_size: u64,
    /// Offset of this record from the volume start when it is the primary copy.
    pub primary_at: u64,
    /// Offset of the backup copy from the volume start, if the filesystem keeps one.
    pub backup_at: Option<u64>,
    /// Bytes to substitute at the volume start when opening from the backup copy.
    pub boot_len: u32,
    check: Check,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Check {
    /// First FAT: byte 0 is the media descriptor, byte 1 is 0xFF.
    FatMedia { offset: u64, media: u8 },
    /// exFAT FAT entry 0 is 0xFFFFFFF8.
    ExFatFat { offset: u64 },
    /// NTFS: an MFT record ("FILE") at the MFT or its mirror.
    NtfsMft { mft: u64, mirror: u64 },
    /// HFS+/APFS: only the signature is checked (detection-only filesystems).
    Signature,
}

/// Recognise a FAT, exFAT or NTFS boot sector, an HFS+/HFSX volume header or an APFS container
/// superblock at the start of `b` (at least 512 bytes). Returns None for anything implausible.
pub fn parse_boot_record(b: &[u8]) -> Option<BootRecord> {
    if b.len() < 512 {
        return None;
    }
    if b[510] == 0x55 && b[511] == 0xAA {
        let oem = &b[3..11];
        if oem == b"NTFS    " {
            return ntfs(b);
        }
        if oem == b"EXFAT   " {
            return exfat(b);
        }
        if let Some(r) = fat(b) {
            return Some(r);
        }
    }
    hfs(b).or_else(|| apfs(b))
}

fn ntfs(b: &[u8]) -> Option<BootRecord> {
    let bps = u32::from(le_u16(b, 11)?);
    if !matches!(bps, 512 | 1024 | 2048 | 4096) {
        return None;
    }
    let spc_raw = u8_at(b, 13)?;
    let spc = if spc_raw > 0x80 {
        1u64.checked_shl(256u32.checked_sub(u32::from(spc_raw))?)?
    } else {
        u64::from(spc_raw)
    };
    if spc == 0 || !spc.is_power_of_two() {
        return None;
    }
    let cluster = u64::from(bps).checked_mul(spc)?;
    let total = le_u64(b, 40)?;
    if total == 0 {
        return None;
    }
    let data_size = total.checked_mul(u64::from(bps))?;
    Some(BootRecord {
        kind: FsKind::Ntfs,
        sector_size: bps,
        // The partition holds `total` sectors plus the backup boot sector after them.
        volume_size: data_size.checked_add(u64::from(bps))?,
        primary_at: 0,
        backup_at: Some(data_size),
        boot_len: bps,
        check: Check::NtfsMft {
            mft: le_u64(b, 48)?.checked_mul(cluster)?,
            mirror: le_u64(b, 56)?.checked_mul(cluster)?,
        },
    })
}

fn exfat(b: &[u8]) -> Option<BootRecord> {
    let bps_shift = u32::from(u8_at(b, 108)?);
    let spc_shift = u32::from(u8_at(b, 109)?);
    if !(9..=12).contains(&bps_shift) || bps_shift.checked_add(spc_shift)? > 25 || u8_at(b, 110)? == 0 {
        return None;
    }
    let bps = 1u32.checked_shl(bps_shift)?;
    let sectors = le_u64(b, 72)?;
    let fat_offset = u64::from(le_u32(b, 80)?);
    if sectors == 0 || fat_offset < 24 || fat_offset >= sectors {
        return None;
    }
    Some(BootRecord {
        kind: FsKind::ExFat,
        sector_size: bps,
        volume_size: sectors.checked_mul(u64::from(bps))?,
        primary_at: 0,
        // The backup boot region starts at sector 12.
        backup_at: Some(12u64.checked_mul(u64::from(bps))?),
        boot_len: bps,
        check: Check::ExFatFat { offset: fat_offset.checked_mul(u64::from(bps))? },
    })
}

fn fat(b: &[u8]) -> Option<BootRecord> {
    let g = crate::fat::Geometry::parse(b)?;
    let backup = if g.kind == FsKind::Fat32 {
        match le_u16(b, 50)? {
            0 | 0xFFFF => None,
            n => Some(u64::from(n).checked_mul(u64::from(g.bytes_per_sector))?),
        }
    } else {
        None
    };
    Some(BootRecord {
        kind: g.kind,
        sector_size: g.bytes_per_sector,
        volume_size: g.volume_size,
        primary_at: 0,
        backup_at: backup,
        boot_len: g.bytes_per_sector,
        check: Check::FatMedia { offset: g.fat_offset, media: u8_at(b, 21)? },
    })
}

fn hfs(b: &[u8]) -> Option<BootRecord> {
    let kind = match (be_u16(b, 0)?, be_u16(b, 2)?) {
        (0x482B, 4) => FsKind::HfsPlus,
        (0x4858, 5) => FsKind::Hfsx,
        _ => return None,
    };
    let block = be_u32(b, 40)?;
    let blocks = u64::from(be_u32(b, 44)?);
    if block < 512 || !block.is_power_of_two() || blocks == 0 {
        return None;
    }
    let size = blocks.checked_mul(u64::from(block))?;
    if size < 4096 {
        return None;
    }
    Some(BootRecord {
        kind,
        sector_size: 512,
        volume_size: size,
        primary_at: 1024,
        backup_at: Some(size.checked_sub(1024)?),
        boot_len: 0,
        check: Check::Signature,
    })
}

fn apfs(b: &[u8]) -> Option<BootRecord> {
    if b.get(32..36)? != b"NXSB" {
        return None;
    }
    let block = le_u32(b, 36)?;
    let blocks = le_u64(b, 40)?;
    if !(4096..=65536).contains(&block) || !block.is_power_of_two() || blocks == 0 {
        return None;
    }
    Some(BootRecord {
        kind: FsKind::Apfs,
        sector_size: 512,
        volume_size: blocks.checked_mul(u64::from(block))?,
        primary_at: 0,
        backup_at: None,
        boot_len: 0,
        check: Check::Signature,
    })
}

impl BootRecord {
    /// Whether the structures this record points to exist in a volume starting at `start`
    /// of `src`. Detection-only filesystems are checked by their signature alone.
    pub fn verify<S: BlockSource + ?Sized>(&self, src: &S, start: u64) -> Result<bool> {
        let read = |rel: u64, buf: &mut [u8]| -> Result<bool> {
            let Some(off) = start.checked_add(rel) else { return Ok(false) };
            Ok(src.read_exact_at(off, buf)? == buf.len())
        };
        match self.check {
            Check::FatMedia { offset, media } => {
                let mut b = [0u8; 2];
                Ok(read(offset, &mut b)? && b[0] == media && b[1] == 0xFF)
            }
            Check::ExFatFat { offset } => {
                let mut b = [0u8; 4];
                Ok(read(offset, &mut b)? && u32::from_le_bytes(b) == 0xFFFF_FFF8)
            }
            Check::NtfsMft { mft, mirror } => {
                let mut b = [0u8; 4];
                for at in [mft, mirror] {
                    if read(at, &mut b)? && &b == b"FILE" {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Check::Signature => Ok(true),
        }
    }

    /// Whether the volume could be opened from its backup boot record (the walkers read only
    /// the first sector).
    pub fn can_open_from_backup(&self) -> bool {
        self.boot_len > 0 && self.backup_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use khodz_io::MemSource;

    fn ntfs_boot(total: u64) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[0..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
        b[3..11].copy_from_slice(b"NTFS    ");
        b[11..13].copy_from_slice(&512u16.to_le_bytes());
        b[13] = 8;
        b[40..48].copy_from_slice(&total.to_le_bytes());
        b[48..56].copy_from_slice(&4u64.to_le_bytes());
        b[56..64].copy_from_slice(&2u64.to_le_bytes());
        b[510] = 0x55;
        b[511] = 0xAA;
        b
    }

    #[test]
    fn ntfs_record_size_backup_and_mft_check() {
        let r = parse_boot_record(&ntfs_boot(2047)).unwrap();
        assert_eq!(r.kind, FsKind::Ntfs);
        assert_eq!(r.volume_size, 2048 * 512);
        assert_eq!(r.backup_at, Some(2047 * 512));
        let mut disk = vec![0u8; 2 << 20];
        let start = 1 << 20;
        assert!(!r.verify(&MemSource::new(disk.clone(), 512), start as u64).unwrap(), "no MFT yet");
        // MFT mirror at cluster 2 (8 KiB) is enough when the MFT itself is damaged.
        disk[start + 2 * 4096..start + 2 * 4096 + 4].copy_from_slice(b"FILE");
        assert!(r.verify(&MemSource::new(disk, 512), start as u64).unwrap());
    }

    #[test]
    fn rejects_noise_and_partial_signatures() {
        assert!(parse_boot_record(&[0u8; 512]).is_none());
        assert!(parse_boot_record(&[0u8; 100]).is_none());
        let mut b = ntfs_boot(0);
        assert!(parse_boot_record(&b).is_none(), "zero sectors");
        b = ntfs_boot(100);
        b[11] = 0x10; // 528-byte sectors
        assert!(parse_boot_record(&b).is_none());
        let mut nx = vec![0u8; 512];
        nx[32..36].copy_from_slice(b"NXSB");
        nx[36..40].copy_from_slice(&100u32.to_le_bytes());
        nx[40..48].copy_from_slice(&10u64.to_le_bytes());
        assert!(parse_boot_record(&nx).is_none(), "implausible APFS block size");
    }

    #[test]
    fn hfs_header_sits_1024_bytes_in() {
        let mut h = vec![0u8; 512];
        h[0..2].copy_from_slice(&0x482Bu16.to_be_bytes());
        h[2..4].copy_from_slice(&4u16.to_be_bytes());
        h[40..44].copy_from_slice(&4096u32.to_be_bytes());
        h[44..48].copy_from_slice(&256u32.to_be_bytes());
        let r = parse_boot_record(&h).unwrap();
        assert_eq!((r.kind, r.primary_at, r.volume_size), (FsKind::HfsPlus, 1024, 1 << 20));
        assert_eq!(r.backup_at, Some((1 << 20) - 1024));
        assert!(!r.can_open_from_backup());
    }
}
