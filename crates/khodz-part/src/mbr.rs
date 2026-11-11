use std::collections::HashSet;

use khodz_core::Result;
use khodz_core::bytes::{le_u32, u8_at};
use khodz_io::BlockSource;

use crate::types::{is_extended, mbr_type_name};
use crate::{Partition, PartitionTable, TableKind};

/// MBR partition tables address sectors in 512-byte units on virtually all media, including
/// 512e drives. (4Kn MBR disks exist but are rare; GPT is used there.)
const MBR_SECTOR: u64 = 512;
/// Upper bound on logical partitions followed through the EBR chain.
const MAX_LOGICAL: u32 = 128;

#[derive(Debug, Clone, Copy)]
struct RawEntry {
    status: u8,
    kind: u8,
    lba: u32,
    sectors: u32,
}

fn entry(sector: &[u8], i: usize) -> Option<RawEntry> {
    let base = 446usize.checked_add(i.checked_mul(16)?)?;
    Some(RawEntry {
        status: u8_at(sector, base)?,
        kind: u8_at(sector, base.checked_add(4)?)?,
        lba: le_u32(sector, base.checked_add(8)?)?,
        sectors: le_u32(sector, base.checked_add(12)?)?,
    })
}

pub fn has_protective_entry(sector0: &[u8]) -> bool {
    (0..4).filter_map(|i| entry(sector0, i)).any(|e| e.kind == 0xEE)
}

/// A filesystem boot sector (superfloppy) also ends in 0x55AA; tell it apart from an MBR.
pub fn looks_like_volume_boot_record(s: &[u8]) -> bool {
    let oem = s.get(3..11).unwrap_or_default();
    if oem == b"NTFS    " || oem == b"EXFAT   " || oem == b"-FVE-FS-" {
        return true;
    }
    let jump = matches!(s.first(), Some(0xEB) | Some(0xE9));
    let fat_label = s.get(82..87) == Some(b"FAT32") || s.get(54..57) == Some(b"FAT");
    // An MBR's entry status bytes are 0x00 or 0x80; a VBR has BPB data there.
    let bad_status = (0..4).filter_map(|i| entry(s, i)).any(|e| e.status != 0 && e.status != 0x80);
    jump && (fat_label || bad_status)
}

fn partition(number: u32, e: RawEntry, base_lba: u64, logical: bool) -> Partition {
    Partition {
        number,
        offset: base_lba.saturating_add(u64::from(e.lba)).saturating_mul(MBR_SECTOR),
        size: u64::from(e.sectors).saturating_mul(MBR_SECTOR),
        type_name: mbr_type_name(e.kind).to_string(),
        mbr_type: Some(e.kind),
        gpt_type: None,
        gpt_id: None,
        name: None,
        bootable: e.status == 0x80,
        logical,
    }
}

pub fn read<S: BlockSource + ?Sized>(src: &S, sector0: &[u8]) -> Result<PartitionTable> {
    let mut table = PartitionTable {
        kind: TableKind::Mbr,
        sector_size: 512,
        disk_signature: le_u32(sector0, 440),
        disk_guid: None,
        partitions: Vec::new(),
        warnings: Vec::new(),
    };
    let mut extended: Option<u64> = None;

    for i in 0..4u32 {
        let Some(e) = entry(sector0, i as usize) else { continue };
        if e.kind == 0 || e.sectors == 0 {
            continue;
        }
        if e.status != 0 && e.status != 0x80 {
            table.warnings.push(format!(
                "entry {} has invalid status byte {:#04x}",
                i.saturating_add(1),
                e.status
            ));
        }
        if is_extended(e.kind) {
            if extended.is_some() {
                table.warnings.push("more than one extended partition; only the first is followed".into());
            } else {
                extended = Some(u64::from(e.lba));
            }
            // The container itself is not listed as a volume, but it is reported for layout views.
        }
        table.partitions.push(partition(i.saturating_add(1), e, 0, false));
    }

    if let Some(ext_lba) = extended {
        read_logical(src, ext_lba, &mut table)?;
    }
    Ok(table)
}

/// Follow the EBR chain. Each EBR's first entry is a logical partition relative to that EBR;
/// the second points to the next EBR relative to the start of the extended partition.
fn read_logical<S: BlockSource + ?Sized>(src: &S, ext_lba: u64, table: &mut PartitionTable) -> Result<()> {
    let mut seen = HashSet::new();
    let mut ebr_lba = ext_lba;
    let mut number = 5u32;
    let mut buf = [0u8; 512];

    loop {
        if number.saturating_sub(5) >= MAX_LOGICAL {
            table.warnings.push(format!("EBR chain longer than {MAX_LOGICAL} entries; stopped"));
            break;
        }
        if !seen.insert(ebr_lba) {
            table.warnings.push(format!("EBR chain loops back to sector {ebr_lba}; stopped"));
            break;
        }
        let Some(off) = ebr_lba.checked_mul(MBR_SECTOR) else { break };
        if src.read_exact_at(off, &mut buf)? < 512 || buf[510] != 0x55 || buf[511] != 0xAA {
            table.warnings.push(format!("invalid EBR at sector {ebr_lba}; chain stopped"));
            break;
        }
        let (Some(first), Some(next)) = (entry(&buf, 0), entry(&buf, 1)) else { break };
        if first.kind != 0 && first.sectors != 0 {
            table.partitions.push(partition(number, first, ebr_lba, true));
            number = number.saturating_add(1);
        }
        if next.kind == 0 || next.sectors == 0 || !is_extended(next.kind) {
            break;
        }
        ebr_lba = ext_lba.saturating_add(u64::from(next.lba));
    }
    Ok(())
}
