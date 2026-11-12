use khodz_core::Result;
use khodz_core::bytes::{le_u32, le_u64, slice, utf16le};
use khodz_io::BlockSource;

use crate::types::gpt_type_name;
use crate::{Guid, Partition, PartitionTable, TableKind};

const MAX_ENTRIES: u32 = 1024;
const MAX_ARRAY_BYTES: u64 = 4 << 20;

#[derive(Debug)]
struct Header {
    my_lba: u64,
    alternate_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: Guid,
    entries_lba: u64,
    entry_count: u32,
    entry_size: u32,
    entries_crc: u32,
}

fn parse_header(buf: &[u8]) -> std::result::Result<Header, String> {
    if buf.get(0..8) != Some(b"EFI PART") {
        return Err("no EFI PART signature".into());
    }
    let hsize = le_u32(buf, 12).ok_or("truncated header")? as usize;
    if !(92..=buf.len()).contains(&hsize) {
        return Err(format!("implausible header size {hsize}"));
    }
    let stored = le_u32(buf, 16).ok_or("truncated header")?;
    let mut copy = buf[..hsize].to_vec();
    copy[16..20].fill(0);
    if crc32fast::hash(&copy) != stored {
        return Err("header CRC mismatch".into());
    }
    let h = Header {
        my_lba: le_u64(buf, 24).ok_or("truncated")?,
        alternate_lba: le_u64(buf, 32).ok_or("truncated")?,
        first_usable: le_u64(buf, 40).ok_or("truncated")?,
        last_usable: le_u64(buf, 48).ok_or("truncated")?,
        disk_guid: Guid::from_slice(&buf[56..]).ok_or("truncated")?,
        entries_lba: le_u64(buf, 72).ok_or("truncated")?,
        entry_count: le_u32(buf, 80).ok_or("truncated")?,
        entry_size: le_u32(buf, 84).ok_or("truncated")?,
        entries_crc: le_u32(buf, 88).ok_or("truncated")?,
    };
    if h.entry_size < 128 || h.entry_size % 8 != 0 || h.entry_count > MAX_ENTRIES {
        return Err(format!("implausible entry array ({} x {} bytes)", h.entry_count, h.entry_size));
    }
    Ok(h)
}

/// Try the header at `lba` with sector size `ss`. Returns the header, the verified entry array
/// and a list of problems found.
fn load<S: BlockSource + ?Sized>(
    src: &S,
    ss: u64,
    lba: u64,
) -> Result<std::result::Result<(Header, Vec<u8>), String>> {
    let mut buf = vec![0u8; ss as usize];
    let Some(off) = lba.checked_mul(ss) else { return Ok(Err("header offset overflow".into())) };
    if src.read_exact_at(off, &mut buf)? < buf.len() {
        return Ok(Err("header beyond end of source".into()));
    }
    let h = match parse_header(&buf) {
        Ok(h) => h,
        Err(e) => return Ok(Err(e)),
    };
    if h.my_lba != lba {
        return Ok(Err(format!("header claims to be at LBA {} but was read at {lba}", h.my_lba)));
    }
    let bytes = u64::from(h.entry_count).saturating_mul(u64::from(h.entry_size));
    if bytes > MAX_ARRAY_BYTES {
        return Ok(Err("entry array too large".into()));
    }
    let mut arr = vec![0u8; bytes as usize];
    let Some(aoff) = h.entries_lba.checked_mul(ss) else { return Ok(Err("entry offset overflow".into())) };
    if src.read_exact_at(aoff, &mut arr)? < arr.len() {
        return Ok(Err("entry array beyond end of source".into()));
    }
    if crc32fast::hash(&arr) != h.entries_crc {
        return Ok(Err("entry array CRC mismatch".into()));
    }
    Ok(Ok((h, arr)))
}

/// Returns `None` when no GPT header is present at all (at 512- or 4096-byte sectors).
pub fn read<S: BlockSource + ?Sized>(src: &S) -> Result<Option<PartitionTable>> {
    for ss in [512u64, 4096] {
        let mut probe = [0u8; 8];
        if src.read_exact_at(ss, &mut probe)? < 8 || &probe != b"EFI PART" {
            // The primary may be destroyed while the backup survives; check the last sector.
            let last = src.len().checked_div(ss).unwrap_or(0).saturating_sub(1);
            let mut tail = [0u8; 8];
            if last == 0 || src.read_exact_at(last.saturating_mul(ss), &mut tail)? < 8 || &tail != b"EFI PART"
            {
                continue;
            }
        }
        let mut warnings = Vec::new();
        let primary = load(src, ss, 1)?;
        let chosen = match primary {
            Ok(v) => {
                // Verify the backup too, so a damaged backup is reported before it is needed,
                // and compare it: two valid but different tables mean the disk was edited by
                // something that updated only one copy, and the user should know which is used.
                match load(src, ss, v.0.alternate_lba)? {
                    Err(e) => warnings.push(format!("backup GPT header problem: {e}")),
                    Ok((bh, barr)) => {
                        if barr != v.1 {
                            warnings.push(
                                "primary and backup GPT partition entries differ; the primary is used".into(),
                            );
                        } else if bh.disk_guid != v.0.disk_guid
                            || bh.first_usable != v.0.first_usable
                            || bh.last_usable != v.0.last_usable
                        {
                            warnings
                                .push("primary and backup GPT headers disagree; the primary is used".into());
                        }
                    }
                }
                Some(v)
            }
            Err(e) => {
                warnings.push(format!("primary GPT header unusable ({e}); using backup"));
                let last = src.len().checked_div(ss).unwrap_or(0).saturating_sub(1);
                match load(src, ss, last)? {
                    Ok(v) => Some(v),
                    Err(e2) => {
                        warnings.push(format!("backup GPT header unusable ({e2})"));
                        None
                    }
                }
            }
        };
        let Some((h, arr)) = chosen else {
            return Ok(Some(PartitionTable {
                kind: TableKind::Gpt,
                sector_size: ss as u32,
                disk_signature: None,
                disk_guid: None,
                partitions: Vec::new(),
                warnings,
            }));
        };
        let mut partitions = Vec::new();
        for i in 0..h.entry_count {
            let Some(e) = slice(&arr, (i as usize).saturating_mul(h.entry_size as usize), 128) else { break };
            let Some(type_guid) = Guid::from_slice(e) else { continue };
            if type_guid.is_zero() {
                continue;
            }
            let first = le_u64(e, 32).unwrap_or(0);
            let last = le_u64(e, 40).unwrap_or(0);
            if last < first {
                warnings.push(format!("entry {} has end before start; skipped", i.saturating_add(1)));
                continue;
            }
            if first < h.first_usable || last > h.last_usable {
                warnings.push(format!("entry {} lies outside the usable area", i.saturating_add(1)));
            }
            let name = utf16le(&e[56..128]);
            partitions.push(Partition {
                number: i.saturating_add(1),
                offset: first.saturating_mul(ss),
                size: last.saturating_sub(first).saturating_add(1).saturating_mul(ss),
                type_name: gpt_type_name(&type_guid),
                mbr_type: None,
                gpt_type: Some(type_guid),
                gpt_id: Guid::from_slice(&e[16..]),
                name: if name.is_empty() { None } else { Some(name) },
                bootable: false,
                logical: false,
            });
        }
        return Ok(Some(PartitionTable {
            kind: TableKind::Gpt,
            sector_size: ss as u32,
            disk_signature: None,
            disk_guid: Some(h.disk_guid),
            partitions,
            warnings,
        }));
    }
    Ok(None)
}
