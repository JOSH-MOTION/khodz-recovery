//! Partition tables built byte-by-byte from the MBR and UEFI GPT specifications.
#![allow(clippy::arithmetic_side_effects)]

use khodz_io::MemSource;
use khodz_part::*;

const MIB: u64 = 1 << 20;

fn put_mbr_entry(sector: &mut [u8], i: usize, status: u8, kind: u8, lba: u32, sectors: u32) {
    let b = 446 + i * 16;
    sector[b] = status;
    sector[b + 4] = kind;
    sector[b + 8..b + 12].copy_from_slice(&lba.to_le_bytes());
    sector[b + 12..b + 16].copy_from_slice(&sectors.to_le_bytes());
}

fn sign(sector: &mut [u8]) {
    sector[510] = 0x55;
    sector[511] = 0xAA;
}

fn disk(size: u64) -> Vec<u8> {
    vec![0u8; size as usize]
}

#[test]
fn mbr_primary_and_logical_partitions() {
    let mut d = disk(64 * MIB);
    {
        let s0 = &mut d[..512];
        s0[440..444].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        put_mbr_entry(s0, 0, 0x80, 0x0C, 2048, 20480); // FAT32, 10 MiB, bootable
        put_mbr_entry(s0, 1, 0x00, 0x0F, 22528, 100_000); // extended container
        sign(s0);
    }
    // EBR 1 at the start of the extended partition: logical 1 + link to EBR 2.
    let ebr1 = 22528 * 512;
    put_mbr_entry(&mut d[ebr1..ebr1 + 512], 0, 0, 0x07, 2048, 10240);
    put_mbr_entry(&mut d[ebr1..ebr1 + 512], 1, 0, 0x05, 20480, 20480);
    sign(&mut d[ebr1..ebr1 + 512]);
    // EBR 2 (relative to extended start): logical 2, end of chain.
    let ebr2 = (22528 + 20480) * 512;
    put_mbr_entry(&mut d[ebr2..ebr2 + 512], 0, 0, 0x07, 2048, 8192);
    sign(&mut d[ebr2..ebr2 + 512]);

    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert_eq!(t.kind, TableKind::Mbr);
    assert_eq!(t.disk_signature, Some(0xDEADBEEF));
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    let summary: Vec<_> = t.partitions.iter().map(|p| (p.number, p.offset, p.size, p.logical)).collect();
    assert_eq!(
        summary,
        vec![
            (1, 2048 * 512, 20480 * 512, false),
            (2, 22528 * 512, 100_000 * 512, false),
            (5, (22528 + 2048) * 512, 10240 * 512, true),
            (6, (22528 + 20480 + 2048) * 512, 8192 * 512, true),
        ]
    );
    assert!(t.partitions[0].bootable);
    assert_eq!(t.partitions[0].type_name, "FAT32");
    assert!(t.partitions[1].is_container());
}

#[test]
fn ebr_loops_are_detected() {
    let mut d = disk(32 * MIB);
    put_mbr_entry(&mut d[..512], 0, 0, 0x05, 2048, 40000);
    sign(&mut d[..512]);
    let ebr = 2048 * 512;
    put_mbr_entry(&mut d[ebr..ebr + 512], 0, 0, 0x07, 63, 1000);
    put_mbr_entry(&mut d[ebr..ebr + 512], 1, 0, 0x05, 0, 1000); // points back to itself
    sign(&mut d[ebr..ebr + 512]);
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert_eq!(t.partitions.iter().filter(|p| p.logical).count(), 1);
    assert!(t.warnings.iter().any(|w| w.contains("loops")), "{:?}", t.warnings);
}

#[test]
fn partitions_past_the_end_and_overlaps_are_warned_not_dropped() {
    let mut d = disk(8 * MIB);
    put_mbr_entry(&mut d[..512], 0, 0, 0x07, 2048, 10_000);
    put_mbr_entry(&mut d[..512], 1, 0, 0x07, 4096, 100_000);
    sign(&mut d[..512]);
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert_eq!(t.partitions.len(), 2);
    assert!(t.warnings.iter().any(|w| w.contains("beyond the end")));
    assert!(t.warnings.iter().any(|w| w.contains("overlap")));
}

#[test]
fn superfloppy_and_blank_disks() {
    let mut fat = disk(MIB);
    fat[0] = 0xEB;
    fat[3..11].copy_from_slice(b"MSDOS5.0");
    fat[82..90].copy_from_slice(b"FAT32   ");
    sign(&mut fat[..512]);
    assert_eq!(read_partition_table(&MemSource::new(fat, 512)).unwrap().kind, TableKind::None);

    let mut ntfs = disk(MIB);
    ntfs[3..11].copy_from_slice(b"NTFS    ");
    sign(&mut ntfs[..512]);
    assert_eq!(read_partition_table(&MemSource::new(ntfs, 512)).unwrap().kind, TableKind::None);

    let blank = read_partition_table(&MemSource::new(disk(MIB), 512)).unwrap();
    assert_eq!(blank.kind, TableKind::Unknown);
    assert!(blank.partitions.is_empty());

    let tiny = read_partition_table(&MemSource::new(vec![0; 100], 512)).unwrap();
    assert_eq!(tiny.kind, TableKind::Unknown);
}

// ---------- GPT ----------

const BASIC_DATA: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";
const EFI: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";

struct GptSpec {
    ss: u64,
    parts: Vec<(&'static str, u64, u64, &'static str)>, // type, first LBA, last LBA, name
}

fn header(
    ss: u64,
    my: u64,
    alt: u64,
    entries_lba: u64,
    first_usable: u64,
    last_usable: u64,
    arr: &[u8],
) -> Vec<u8> {
    let mut h = vec![0u8; ss as usize];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    h[12..16].copy_from_slice(&92u32.to_le_bytes());
    h[24..32].copy_from_slice(&my.to_le_bytes());
    h[32..40].copy_from_slice(&alt.to_le_bytes());
    h[40..48].copy_from_slice(&first_usable.to_le_bytes());
    h[48..56].copy_from_slice(&last_usable.to_le_bytes());
    h[56..72].copy_from_slice(&Guid::parse("11111111-2222-3333-4444-555555555555").unwrap().0);
    h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
    h[80..84].copy_from_slice(&128u32.to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes());
    h[88..92].copy_from_slice(&crc32fast::hash(arr).to_le_bytes());
    let crc = crc32fast::hash(&h[..92]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

fn build_gpt(spec: &GptSpec, size: u64) -> Vec<u8> {
    let ss = spec.ss;
    let mut d = disk(size);
    let last_lba = size / ss - 1;
    let array_sectors = (128 * 128) / ss;
    let mut arr = vec![0u8; 128 * 128];
    for (i, (t, first, last, name)) in spec.parts.iter().enumerate() {
        let e = &mut arr[i * 128..(i + 1) * 128];
        e[0..16].copy_from_slice(&Guid::parse(t).unwrap().0);
        e[16..32].copy_from_slice(&[i as u8 + 1; 16]);
        e[32..40].copy_from_slice(&first.to_le_bytes());
        e[40..48].copy_from_slice(&last.to_le_bytes());
        for (j, u) in name.encode_utf16().enumerate() {
            e[56 + j * 2..58 + j * 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    let first_usable = 2 + array_sectors;
    let last_usable = last_lba - 1 - array_sectors;
    // Protective MBR.
    put_mbr_entry(&mut d[..512], 0, 0, 0xEE, 1, u32::try_from(last_lba).unwrap_or(u32::MAX));
    sign(&mut d[..512]);
    // Primary.
    let p = header(ss, 1, last_lba, 2, first_usable, last_usable, &arr);
    d[ss as usize..2 * ss as usize].copy_from_slice(&p);
    d[(2 * ss) as usize..(2 * ss) as usize + arr.len()].copy_from_slice(&arr);
    // Backup: array just before the last LBA, header at the last LBA.
    let backup_arr_lba = last_lba - array_sectors;
    let b = header(ss, last_lba, 1, backup_arr_lba, first_usable, last_usable, &arr);
    let ba = (backup_arr_lba * ss) as usize;
    d[ba..ba + arr.len()].copy_from_slice(&arr);
    d[(last_lba * ss) as usize..((last_lba + 1) * ss) as usize].copy_from_slice(&b);
    d
}

fn spec(ss: u64) -> GptSpec {
    let unit = MIB / ss;
    GptSpec {
        ss,
        parts: vec![
            (EFI, unit, 3 * unit - 1, "EFI system partition"),
            (BASIC_DATA, 3 * unit, 12 * unit - 1, "Data café"),
        ],
    }
}

#[test]
fn gpt_512_and_4096_byte_sectors() {
    for ss in [512u64, 4096] {
        let d = build_gpt(&spec(ss), 16 * MIB);
        let t = read_partition_table(&MemSource::new(d, ss as u32)).unwrap();
        assert_eq!(t.kind, TableKind::Gpt, "ss {ss}");
        assert_eq!(t.sector_size, ss as u32, "sector size detected from the header location");
        assert!(t.warnings.is_empty(), "ss {ss}: {:?}", t.warnings);
        assert_eq!(t.disk_guid.unwrap().to_string(), "11111111-2222-3333-4444-555555555555");
        assert_eq!(t.partitions.len(), 2);
        let data = &t.partitions[1];
        assert_eq!((data.number, data.offset, data.size), (2, 3 * MIB, 9 * MIB));
        assert_eq!(data.type_name, "Microsoft basic data");
        assert_eq!(data.name.as_deref(), Some("Data café"));
        assert_eq!(t.partitions[0].type_name, "EFI system");
    }
}

#[test]
fn damaged_primary_header_falls_back_to_backup() {
    let mut d = build_gpt(&spec(512), 16 * MIB);
    d[512 + 40] ^= 0xFF; // corrupt a header field: CRC no longer matches
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert_eq!(t.kind, TableKind::Gpt);
    assert_eq!(t.partitions.len(), 2, "recovered from backup");
    assert!(t.warnings.iter().any(|w| w.contains("primary GPT header unusable")), "{:?}", t.warnings);
}

#[test]
fn destroyed_primary_and_mbr_still_found_via_backup() {
    let mut d = build_gpt(&spec(512), 16 * MIB);
    d[..1024].fill(0); // wipe protective MBR and primary header
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert_eq!(t.kind, TableKind::Gpt);
    assert_eq!(t.partitions.len(), 2);
    assert!(t.warnings.iter().any(|w| w.contains("without a protective MBR")));
}

#[test]
fn corrupted_entry_array_is_detected() {
    let mut d = build_gpt(&spec(512), 16 * MIB);
    d[2 * 512 + 40] ^= 1; // primary entry array byte
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert!(t.warnings.iter().any(|w| w.contains("entry array CRC")), "{:?}", t.warnings);
    assert_eq!(t.partitions.len(), 2, "backup array used");
}

#[test]
fn damaged_backup_is_reported() {
    let mut d = build_gpt(&spec(512), 16 * MIB);
    let last = d.len() - 512;
    d[last + 30] ^= 0xFF;
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert_eq!(t.partitions.len(), 2);
    assert!(t.warnings.iter().any(|w| w.contains("backup GPT header problem")), "{:?}", t.warnings);
}

#[test]
fn valid_but_different_primary_and_backup_are_reported() {
    let ss = 512u64;
    let size = 16 * MIB;
    let mut d = build_gpt(&spec(ss), size);
    // Rewrite the backup with a consistent (correct CRCs) but different entry array, as a tool
    // that updated only one copy would leave it.
    let last_lba = size / ss - 1;
    let array_sectors = (128 * 128) / ss;
    let backup_arr_lba = last_lba - array_sectors;
    let ba = (backup_arr_lba * ss) as usize;
    let mut arr = d[ba..ba + 128 * 128].to_vec();
    arr[128 + 56] = b'X'; // rename partition 2 in the backup only
    let first_usable = 2 + array_sectors;
    let b = header(ss, last_lba, 1, backup_arr_lba, first_usable, last_lba - 1 - array_sectors, &arr);
    d[ba..ba + arr.len()].copy_from_slice(&arr);
    d[(last_lba * ss) as usize..].copy_from_slice(&b);
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert_eq!(t.partitions[1].name.as_deref(), Some("Data café"), "primary is used");
    assert!(t.warnings.iter().any(|w| w.contains("entries differ")), "{:?}", t.warnings);
}

#[test]
fn hostile_header_values_are_rejected_without_panics() {
    let mut d = build_gpt(&spec(512), 16 * MIB);
    // Claim 2^32-1 entries of 2^32-8 bytes and fix up the CRC so only the bounds check stops it.
    d[512 + 80..512 + 84].copy_from_slice(&u32::MAX.to_le_bytes());
    d[512 + 84..512 + 88].copy_from_slice(&(u32::MAX - 7).to_le_bytes());
    d[512 + 16..512 + 20].fill(0);
    let crc = crc32fast::hash(&d[512..512 + 92]);
    d[512 + 16..512 + 20].copy_from_slice(&crc.to_le_bytes());
    let t = read_partition_table(&MemSource::new(d, 512)).unwrap();
    assert!(t.warnings.iter().any(|w| w.contains("implausible")), "{:?}", t.warnings);
}
