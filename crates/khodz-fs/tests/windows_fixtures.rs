//! Verifies partition and filesystem parsing against disk images that Windows itself
//! partitioned, formatted and populated (fixtures/windows/make-fixtures.ps1).
//!
//! Run: `cargo test -p khodz-fs --test windows_fixtures -- --ignored --nocapture`
#![allow(clippy::arithmetic_side_effects)]

use std::collections::HashMap;
use std::path::PathBuf;

use khodz_core::CancelToken;
use khodz_fs::{Entry, ExtentQuality, FsKind};
use khodz_io::{BlockSource, ImageFile, SliceSource, sha256_source};
use khodz_part::{TableKind, read_partition_table};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out")
}

fn manifest() -> Value {
    let path = fixtures_dir().join("manifest.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!("{} missing: run fixtures/windows/make-fixtures.ps1 as Administrator", path.display())
    });
    serde_json::from_str(text.trim_start_matches('\u{feff}')).expect("manifest is valid JSON")
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn expected_kind(fs: &str) -> FsKind {
    match fs {
        "fat32" => FsKind::Fat32,
        "exfat" => FsKind::ExFat,
        "ntfs" => FsKind::Ntfs,
        other => panic!("unexpected fs {other}"),
    }
}

/// Deleted 8.3-only names lose their first character; compare the rest.
fn name_matches(entry: &Entry, expected_path: &str) -> bool {
    if entry.name_complete {
        return entry.path == expected_path;
    }
    let (ep, xp) = (entry.path.to_lowercase(), expected_path.to_lowercase());
    let (Some((edir, ename)), Some((xdir, xname))) = (ep.rsplit_once('/'), xp.rsplit_once('/')) else {
        return false;
    };
    edir == xdir && ename.len() == xname.len() && ename.get(1..) == xname.get(1..)
}

fn as_vec(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()], // PowerShell serialises 1-element arrays as objects
    }
}

#[test]
#[ignore = "needs fixtures: run fixtures/windows/make-fixtures.ps1 as Administrator"]
fn windows_built_images_parse_exactly() {
    let m = manifest();
    let mut checked_files = 0;
    let mut checked_deleted = 0;
    for img in as_vec(&m["images"]) {
        let file = img["file"].as_str().unwrap();
        let path = fixtures_dir().join(file);
        let before = std::fs::read(&path).map(|b| hex(&b)).unwrap();
        let src = ImageFile::open(&path).unwrap();
        println!("== {file} ({} bytes)", src.len());

        // ---- partition table ----
        let table = read_partition_table(&src).unwrap();
        let style = img["partition_style"].as_str().unwrap();
        assert_eq!(table.kind, if style == "gpt" { TableKind::Gpt } else { TableKind::Mbr }, "{file}");
        assert!(table.warnings.is_empty(), "{file}: {:?}", table.warnings);
        let mut ours: Vec<(u64, u64)> =
            table.partitions.iter().filter(|p| !p.is_container()).map(|p| (p.offset, p.size)).collect();
        let mut theirs: Vec<(u64, u64)> = as_vec(&img["partitions"])
            .iter()
            .filter(|p| !p["is_extended"].as_bool().unwrap_or(false))
            .map(|p| (p["offset"].as_u64().unwrap(), p["size"].as_u64().unwrap()))
            .collect();
        ours.sort_unstable();
        theirs.sort_unstable();
        assert_eq!(ours, theirs, "{file}: partition layout differs from Windows");
        if style == "mbr" {
            assert_eq!(table.disk_signature.map(u64::from), img["disk_signature"].as_u64(), "{file}");
        } else {
            let guid =
                img["disk_guid"].as_str().unwrap().trim_matches(|c| c == '{' || c == '}').to_uppercase();
            assert_eq!(table.disk_guid.map(|g| g.to_string()), Some(guid), "{file}");
        }

        // ---- filesystems ----
        for vol in as_vec(&img["volumes"]) {
            let offset = vol["offset"].as_u64().unwrap();
            let size = vol["size"].as_u64().unwrap();
            let label = vol["label"].as_str().unwrap();
            let slice = SliceSource::new(&src, offset, size, label).unwrap();
            let info = khodz_fs::detect(&slice).unwrap();
            assert_eq!(info.kind, expected_kind(vol["filesystem"].as_str().unwrap()), "{file}/{label}");

            let fs = khodz_fs::open(&slice).unwrap();
            assert_eq!(fs.info().label.as_deref(), Some(label), "{file}: volume label");
            let mut entries: Vec<Entry> = Vec::new();
            let stats = fs
                .walk(&CancelToken::new(), &mut |e| {
                    entries.push(e);
                    Ok(())
                })
                .unwrap();
            assert_eq!(stats.corrupt, 0, "{file}/{label}: corrupt structures on a fresh volume");
            let live: HashMap<&str, &Entry> =
                entries.iter().filter(|e| !e.deleted).map(|e| (e.path.as_str(), e)).collect();
            let mut fragmented_extents = Vec::new();

            for f in as_vec(&vol["files"]) {
                let fpath = format!("/{}", f["path"].as_str().unwrap());
                let fsize = f["size"].as_u64().unwrap();
                let fhash = f["sha256"].as_str().unwrap();
                if f["deleted"].as_bool().unwrap() {
                    let kind = info.kind;
                    // NTFS pending deletes (\$Extend\$Deleted) keep the data but lose the name:
                    // identify them by size and content instead.
                    let by_content = || {
                        entries.iter().find(|e| {
                            e.deleted
                                && !e.name_complete
                                && e.path.starts_with("/$Extend/$Deleted/")
                                && e.size == fsize
                                && fs.read_data(e, fsize).map(|d| hex(&d) == fhash).unwrap_or(false)
                        })
                    };
                    let found = entries.iter().find(|e| e.deleted && !e.is_dir && name_matches(e, &fpath));
                    let found = found.or_else(|| if kind == FsKind::Ntfs { by_content() } else { None });
                    let Some(found) = found else {
                        // On FAT/exFAT a directory's listing lives in its own clusters. When the
                        // directory is deleted those clusters are freed and Windows TRIMs them
                        // (as on an SSD), so its children are undiscoverable by any tool.
                        // NTFS keeps the records in the MFT, so it must always find them.
                        let in_deleted_dir = fpath.starts_with("/deleted-dir/");
                        assert!(
                            in_deleted_dir && kind != FsKind::Ntfs,
                            "{file}/{label}: deleted {fpath} not discovered"
                        );
                        println!("   deleted {fpath}: not discoverable (parent directory clusters erased)");
                        continue;
                    };
                    assert_eq!(found.size, fsize, "{file}/{label}: deleted {fpath} size");
                    let data = fs.read_data(found, fsize).unwrap_or_default();
                    let state = if hex(&data) == fhash {
                        "intact"
                    } else if data.iter().all(|&b| b == 0) {
                        "erased (TRIM)"
                    } else {
                        "overwritten/other"
                    };
                    println!(
                        "   deleted {fpath}: found as {} ({:?}, name complete: {}, data: {state})",
                        found.path, found.quality, found.name_complete
                    );
                    checked_deleted += 1;
                    continue;
                }
                let e =
                    live.get(fpath.as_str()).unwrap_or_else(|| panic!("{file}/{label}: {fpath} not found"));
                assert_eq!(e.size, fsize, "{file}/{label}: {fpath} size");
                assert_eq!(e.quality, ExtentQuality::Exact, "{file}/{label}: {fpath}");
                let data = fs.read_data(e, u64::MAX).unwrap();
                assert_eq!(hex(&data), fhash, "{file}/{label}: {fpath} content");
                if f["fragmented"].as_bool().unwrap_or(false) {
                    fragmented_extents.push((fpath.clone(), e.extents.len()));
                }
                checked_files += 1;
            }
            // The deleted directory itself must be discovered too.
            assert!(
                entries.iter().any(|e| e.deleted && e.is_dir && e.path.eq_ignore_ascii_case("/deleted-dir")),
                "{file}/{label}: deleted directory not discovered"
            );
            println!(
                "   {label}: {:?} {} entries ({} deleted), fragmented file extents: {fragmented_extents:?}",
                info.kind, stats.entries, stats.deleted
            );
        }
        drop(src);
        let after = std::fs::read(&path).map(|b| hex(&b)).unwrap();
        assert_eq!(before, after, "{file} modified!");
        // Cross-check: our streaming fingerprint agrees with a whole-file hash.
        let fp = sha256_source(&ImageFile::open(&path).unwrap(), &CancelToken::new(), |_| {}).unwrap();
        assert_eq!(fp.sha256, after);
    }
    println!("verified {checked_files} live files byte-for-byte and {checked_deleted} deleted files");
    assert!(checked_files > 0 && checked_deleted > 0);
}
