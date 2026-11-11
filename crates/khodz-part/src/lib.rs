//! Partition table parsing: MBR (with the EBR chain of logical partitions) and GPT (header and
//! entry-array CRCs verified, backup header used when the primary is damaged).
//!
//! Parsers never trust on-disk values: every offset is bounds-checked against the source, chain
//! walks have hard limits and cycle detection, and inconsistencies are reported as warnings
//! rather than silently "fixed".

#![cfg_attr(test, allow(clippy::arithmetic_side_effects))]

mod gpt;
mod guid;
mod mbr;
mod types;

use serde::Serialize;

use khodz_core::Result;
use khodz_io::BlockSource;

pub use guid::Guid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableKind {
    Mbr,
    Gpt,
    /// No partition table: a filesystem starts at sector 0 ("superfloppy", common on USB sticks
    /// and in volume images).
    None,
    /// Nothing recognisable at the start of the disk.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Partition {
    /// 1-based position as the OS numbers it (MBR: 1–4 primary, 5+ logical; GPT: entry index + 1).
    pub number: u32,
    pub offset: u64,
    pub size: u64,
    /// Human-readable type, e.g. "Microsoft basic data", "FAT32 (LBA)".
    pub type_name: String,
    pub mbr_type: Option<u8>,
    pub gpt_type: Option<Guid>,
    pub gpt_id: Option<Guid>,
    pub name: Option<String>,
    pub bootable: bool,
    pub logical: bool,
}

impl Partition {
    /// An MBR extended partition: a container for logical partitions, not a volume itself.
    pub fn is_container(&self) -> bool {
        self.mbr_type.is_some_and(types::is_extended)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PartitionTable {
    pub kind: TableKind,
    /// Sector size the table was found with. For GPT this is detected (512 or 4096).
    pub sector_size: u32,
    pub disk_signature: Option<u32>,
    pub disk_guid: Option<Guid>,
    pub partitions: Vec<Partition>,
    pub warnings: Vec<String>,
}

/// Read the partition table at the start of `src`.
pub fn read_partition_table<S: BlockSource + ?Sized>(src: &S) -> Result<PartitionTable> {
    let mut sector0 = [0u8; 512];
    if src.read_exact_at(0, &mut sector0)? < 512 {
        return Ok(empty(TableKind::Unknown, src.sector_size(), "source smaller than one sector"));
    }

    let signature = sector0[510] == 0x55 && sector0[511] == 0xAA;
    let protective = signature && mbr::has_protective_entry(&sector0);

    // GPT may exist even if the protective MBR is damaged; check whenever the header is present.
    if let Some(mut table) = gpt::read(src)? {
        if !protective {
            table.warnings.push("GPT found without a protective MBR".into());
        }
        check_layout(&mut table, src.len());
        return Ok(table);
    }
    if protective {
        return Ok(empty(
            TableKind::Unknown,
            src.sector_size(),
            "protective MBR present but no valid GPT header",
        ));
    }
    if !signature {
        return Ok(empty(TableKind::Unknown, src.sector_size(), "no 0x55AA boot signature in sector 0"));
    }
    if mbr::looks_like_volume_boot_record(&sector0) {
        return Ok(empty(TableKind::None, src.sector_size(), "sector 0 is a filesystem boot sector"));
    }
    let mut table = mbr::read(src, &sector0)?;
    check_layout(&mut table, src.len());
    Ok(table)
}

fn empty(kind: TableKind, sector_size: u32, warning: &str) -> PartitionTable {
    PartitionTable {
        kind,
        sector_size,
        disk_signature: None,
        disk_guid: None,
        partitions: Vec::new(),
        warnings: if warning.is_empty() { Vec::new() } else { vec![warning.to_string()] },
    }
}

/// Flag partitions extending past the end of the source and overlapping partitions. Such
/// partitions are kept (they may be exactly what the user is trying to recover) but warned about.
fn check_layout(table: &mut PartitionTable, disk_len: u64) {
    for p in &table.partitions {
        if p.offset.saturating_add(p.size) > disk_len {
            table.warnings.push(format!("partition {} extends beyond the end of the disk", p.number));
        }
    }
    let mut sorted: Vec<&Partition> = table.partitions.iter().filter(|p| !p.is_container()).collect();
    sorted.sort_by_key(|p| p.offset);
    for w in sorted.windows(2) {
        if w[0].offset.saturating_add(w[0].size) > w[1].offset {
            table.warnings.push(format!("partitions {} and {} overlap", w[0].number, w[1].number));
        }
    }
}
