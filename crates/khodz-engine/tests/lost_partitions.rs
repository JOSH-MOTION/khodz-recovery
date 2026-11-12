//! Lost-partition analysis on the Windows-built multi-volume disk (FAT32 primary, NTFS and
//! exFAT logical), with the damage real disks suffer applied to an in-memory copy: an erased
//! partition table, destroyed boot sectors, and both together. Every live file must still come
//! back byte-for-byte from the volumes the analysis finds.
//!
//! Run: `cargo test -p khodz-engine --test lost_partitions -- --ignored --nocapture`
#![allow(clippy::arithmetic_side_effects)]

use std::path::PathBuf;

use khodz_core::CancelToken;
use khodz_engine::partitions::{inspect_layout, search_lost_partitions};
use khodz_engine::*;
use khodz_fs::FsKind;
use khodz_io::MemSource;
use sha2::{Digest, Sha256};

const FAT: u64 = 65_536;
const NTFS: u64 = 42_074_112;
const EXFAT: u64 = 84_082_688;
const VOL: u64 = 41_943_040;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out")
}

fn image() -> Vec<u8> {
    image_named("mbr-multi.img")
}

fn image_named(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).expect("generate fixtures first (see TESTING.md)")
}

fn manifest_volumes() -> Vec<serde_json::Value> {
    manifest_volumes_of("mbr-multi.img")
}

fn manifest_volumes_of(name: &str) -> Vec<serde_json::Value> {
    let t = std::fs::read_to_string(dir().join("manifest.json")).unwrap();
    let m: serde_json::Value = serde_json::from_str(t.trim_start_matches('\u{feff}')).unwrap();
    let img = m["images"].as_array().unwrap().iter().find(|i| i["file"] == name).unwrap().clone();
    img["volumes"].as_array().unwrap().clone()
}

fn hex(d: &[u8]) -> String {
    Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect()
}

fn zero_sector(d: &mut [u8], at: u64) {
    d[at as usize..at as usize + 512].fill(0);
}

/// Scan the given volumes and check every live file of every manifest volume against its hash.
/// Returns the number of files verified.
fn files_match(src: &MemSource, specs: &[VolumeSpec]) -> usize {
    files_match_in(src, specs, &manifest_volumes())
}

fn files_match_in(src: &MemSource, specs: &[VolumeSpec], volumes: &[serde_json::Value]) -> usize {
    let mut items = Vec::new();
    let summary =
        scan_volumes(src, ScanMode::Quick, Some(specs), &CancelToken::new(), &mut |_| {}, &mut |i| {
            items.push(i);
            Ok(())
        })
        .unwrap();
    for v in &summary.volumes {
        assert!(v.error.is_none(), "volume at {}: {:?}", v.offset, v.error);
    }
    let mut verified = 0;
    for vol in volumes {
        let offset = vol["offset"].as_u64().unwrap();
        for f in vol["files"].as_array().unwrap().iter().filter(|f| !f["deleted"].as_bool().unwrap()) {
            let path = format!("/{}", f["path"].as_str().unwrap());
            let item = items
                .iter()
                .find(|i| i.volume_offset == offset && i.path == path && !i.deleted)
                .unwrap_or_else(|| panic!("{offset}{path} not found"));
            let data =
                read_item_range(src, &item.extents, item.resident.as_deref(), item.size, 0, usize::MAX)
                    .unwrap();
            assert_eq!(hex(&data), f["sha256"].as_str().unwrap(), "{offset}{path}");
            verified += 1;
        }
    }
    verified
}

fn summary(found: &[AnalyzedVolume]) -> Vec<(u64, FsKind, bool, bool)> {
    let mut s: Vec<_> =
        found.iter().map(|v| (v.offset, v.filesystem, v.boot_backup.is_some(), v.opens)).collect();
    s.sort_unstable_by_key(|x| x.0);
    s
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn intact_disk_reports_no_problems_and_nothing_lost() {
    let src = MemSource::new(image(), 512);
    let layout = inspect_layout(&src).unwrap();
    assert!(layout.issues.is_empty(), "{:?}", layout.issues);
    assert_eq!(layout.volumes.len(), 3);
    assert!(layout.volumes.iter().all(|v| v.verified && v.opens && v.boot_backup.is_none()));
    let found = search_lost_partitions(&src, &layout, &CancelToken::new(), |_| {}).unwrap();
    assert!(found.is_empty(), "{found:?}");
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn erased_partition_table_is_rebuilt_from_boot_records() {
    let mut d = image();
    zero_sector(&mut d, 0); // MBR gone (EBRs remain but are no longer reachable)
    let src = MemSource::new(d, 512);
    let layout = inspect_layout(&src).unwrap();
    assert!(layout.volumes.is_empty());
    assert!(layout.issues.iter().any(|i| i.contains("search for lost partitions")), "{:?}", layout.issues);

    let found = search_lost_partitions(&src, &layout, &CancelToken::new(), |_| {}).unwrap();
    assert_eq!(
        summary(&found),
        vec![
            (FAT, FsKind::Fat32, false, true),
            (NTFS, FsKind::Ntfs, false, true),
            (EXFAT, FsKind::ExFat, false, true)
        ]
    );
    let labels: Vec<_> = found.iter().map(|v| v.label.clone().unwrap_or_default()).collect();
    assert!(
        labels.contains(&"PFAT".into())
            && labels.contains(&"LNTFS".into())
            && labels.contains(&"LEXFAT".into())
    );
    assert!(found.iter().all(|v| v.size >= VOL - 512 && v.size <= VOL));
    let specs: Vec<_> = found.iter().map(AnalyzedVolume::spec).collect();
    println!("table erased: {} files verified", files_match(&src, &specs));
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn table_and_every_boot_sector_destroyed_are_recovered_from_backups() {
    let mut d = image();
    for at in [0, FAT, NTFS, EXFAT] {
        zero_sector(&mut d, at);
    }
    let src = MemSource::new(d, 512);
    let layout = inspect_layout(&src).unwrap();
    let found = search_lost_partitions(&src, &layout, &CancelToken::new(), |_| {}).unwrap();
    assert_eq!(
        summary(&found),
        vec![
            (FAT, FsKind::Fat32, true, true),
            (NTFS, FsKind::Ntfs, true, true),
            (EXFAT, FsKind::ExFat, true, true)
        ],
        "{found:#?}"
    );
    assert!(found.iter().all(|v| v.notes.iter().any(|n| n.contains("backup boot record"))));
    let specs: Vec<_> = found.iter().map(AnalyzedVolume::spec).collect();
    println!("table + boot sectors destroyed: {} files verified", files_match(&src, &specs));
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn damaged_boot_sector_with_intact_table_uses_the_backup_automatically() {
    let mut d = image();
    zero_sector(&mut d, NTFS);
    zero_sector(&mut d, FAT);
    let src = MemSource::new(d, 512);
    let layout = inspect_layout(&src).unwrap();
    let by_offset = |o: u64| layout.volumes.iter().find(|v| v.offset == o).unwrap();
    assert!(by_offset(NTFS).boot_backup.is_some() && by_offset(NTFS).opens);
    assert!(by_offset(FAT).boot_backup.is_some() && by_offset(FAT).opens);
    assert!(by_offset(EXFAT).boot_backup.is_none());
    assert!(
        layout.issues.iter().any(|i| i.starts_with("Partition 5:") && i.contains("backup")),
        "{:?}",
        layout.issues
    );
    // A plain scan (no explicit volumes) now reads all three volumes.
    let mut items = Vec::new();
    let s = scan(&src, ScanMode::Quick, &CancelToken::new(), &mut |_| {}, &mut |i| {
        items.push(i);
        Ok(())
    })
    .unwrap();
    assert!(s.volumes.iter().all(|v| v.error.is_none() && v.entries > 0), "{:?}", s.volumes);
    let specs: Vec<_> = layout.volumes.iter().map(AnalyzedVolume::spec).collect();
    println!("boot sectors destroyed, table intact: {} files verified", files_match(&src, &specs));
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn deleted_partition_entry_is_found_and_marked_as_lost() {
    let mut d = image();
    // Delete the primary FAT32 entry from the MBR (entry 1), as a "delete partition" would.
    d[446..462].fill(0);
    let src = MemSource::new(d, 512);
    let layout = inspect_layout(&src).unwrap();
    assert_eq!(layout.volumes.len(), 2, "logical volumes remain");
    assert!(layout.unallocated.iter().any(|&(a, b)| a <= FAT && b >= FAT + VOL), "{:?}", layout.unallocated);
    let found = search_lost_partitions(&src, &layout, &CancelToken::new(), |_| {}).unwrap();
    assert_eq!(summary(&found), vec![(FAT, FsKind::Fat32, false, true)]);
    assert_eq!(found[0].origin, VolumeOrigin::Search);
    assert_eq!(found[0].label.as_deref(), Some("PFAT"));
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn shrunk_and_mistyped_entries_are_reported() {
    let mut d = image();
    // Entry 1: FAT32 volume, shrink it to half its size and claim it is NTFS (0x07).
    d[446 + 4] = 0x07;
    let sectors = u32::from_le_bytes(d[446 + 12..446 + 16].try_into().unwrap());
    d[446 + 12..446 + 16].copy_from_slice(&(sectors / 2).to_le_bytes());
    let src = MemSource::new(d, 512);
    let layout = inspect_layout(&src).unwrap();
    let issues = layout.issues.join("\n");
    assert!(issues.contains("Partition 1: The filesystem says it is 40.0 MiB"), "{issues}");
    assert!(issues.contains("does not match the FAT32 filesystem"), "{issues}");
}

#[test]
#[ignore = "needs fixtures/out/gpt-ntfs.img"]
fn gpt_disk_wiped_like_diskpart_clean_is_recovered() {
    let mut d = image_named("gpt-ntfs.img");
    // `clean` overwrites the protective MBR, the primary GPT (LBA 0-33) and the backup GPT
    // (last 33 sectors). The volume itself is untouched.
    let len = d.len();
    d[..34 * 512].fill(0);
    d[len - 33 * 512..].fill(0);
    let src = MemSource::new(d, 512);
    let layout = inspect_layout(&src).unwrap();
    assert_eq!(layout.table.kind, khodz_part::TableKind::Unknown);
    let found = search_lost_partitions(&src, &layout, &CancelToken::new(), |_| {}).unwrap();
    let volumes = manifest_volumes_of("gpt-ntfs.img");
    let expected: Vec<u64> = volumes.iter().map(|v| v["offset"].as_u64().unwrap()).collect();
    let got: Vec<u64> = found.iter().filter(|v| v.filesystem == FsKind::Ntfs).map(|v| v.offset).collect();
    assert_eq!(got, expected, "{found:#?}");
    let specs: Vec<_> = found.iter().map(AnalyzedVolume::spec).collect();
    println!("GPT wiped: {} files verified", files_match_in(&src, &specs, &volumes));
}

#[test]
#[ignore = "needs fixtures/out/gpt-ntfs.img"]
fn intact_windows_gpt_disk_has_no_false_alarms() {
    let src = MemSource::new(image_named("gpt-ntfs.img"), 512);
    let layout = inspect_layout(&src).unwrap();
    assert!(layout.issues.is_empty(), "{:?}", layout.issues);
    assert!(layout.volumes.iter().any(|v| v.filesystem == FsKind::Ntfs && v.opens));
    let found = search_lost_partitions(&src, &layout, &CancelToken::new(), |_| {}).unwrap();
    assert!(found.is_empty(), "{found:#?}");
}
