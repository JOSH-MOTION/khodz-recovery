#![allow(clippy::arithmetic_side_effects)]

use khodz_core::{CancelToken, Error};
use khodz_engine::*;
use khodz_io::{ImageFile, MemSource};

fn run(src: &dyn khodz_io::BlockSource, mode: ScanMode) -> (ScanSummary, Vec<ScanItem>, Vec<ScanProgress>) {
    let mut items = Vec::new();
    let mut progress = Vec::new();
    let summary = scan(src, mode, &CancelToken::new(), &mut |p| progress.push(p), &mut |i| {
        items.push(i);
        Ok(())
    })
    .unwrap();
    (summary, items, progress)
}

#[test]
fn blank_media_produces_no_items_and_no_errors() {
    let src = MemSource::new(vec![0u8; 4 << 20], 512);
    let (summary, items, progress) = run(&src, ScanMode::Deep);
    assert!(items.is_empty());
    assert_eq!(summary.volumes.len(), 1);
    assert_eq!(summary.volumes[0].filesystem, "Unknown");
    assert_eq!(progress.last().map(|p| p.bytes_done), Some(4 << 20));
}

#[test]
fn cancellation_is_propagated() {
    let src = MemSource::new(vec![0u8; 8 << 20], 512);
    let cancel = CancelToken::new();
    cancel.cancel();
    let r = scan(&src, ScanMode::Deep, &cancel, &mut |_| {}, &mut |_| Ok(()));
    assert!(matches!(r, Err(Error::Cancelled)));
}

#[test]
fn sink_errors_abort_the_scan() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/out/carving.img");
    let Ok(img) = ImageFile::open(fixture) else { return }; // covered by the ignored tests below
    let r = scan(&img, ScanMode::Deep, &CancelToken::new(), &mut |_| {}, &mut |_| {
        Err(Error::InvalidData("db full".into()))
    });
    assert!(matches!(r, Err(Error::InvalidData(_))));
}

fn fixture(name: &str) -> ImageFile {
    let p = format!("{}/../../fixtures/out/{name}", env!("CARGO_MANIFEST_DIR"));
    ImageFile::open(&p).unwrap_or_else(|_| panic!("{p} missing: generate fixtures (see TESTING.md)"))
}

#[test]
#[ignore = "needs Windows-built fixtures"]
fn quick_scan_of_multi_partition_disk() {
    let img = fixture("mbr-multi.img");
    let (summary, items, _) = run(&img, ScanMode::Quick);
    let fs: Vec<_> = summary.volumes.iter().map(|v| (v.filesystem.as_str(), v.label.as_deref())).collect();
    assert_eq!(fs, vec![("FAT32", Some("PFAT")), ("NTFS", Some("LNTFS")), ("exFAT", Some("LEXFAT"))]);
    assert!(summary.volumes.iter().all(|v| v.error.is_none() && v.corrupt == 0));
    // Each volume contributes the same user files; deleted ones are flagged and erased (TRIM).
    for v in &summary.volumes {
        let report = items
            .iter()
            .find(|i| i.volume_offset == v.offset && i.path == "/docs/report.bin")
            .unwrap_or_else(|| panic!("{} report.bin", v.filesystem));
        assert_eq!((report.size, report.deleted, report.data_state), (102_400, false, DataState::Present));
        assert!(
            report.extents.iter().all(|e| e.offset.is_some_and(|o| o >= v.offset && o < v.offset + v.size)),
            "absolute extents inside the volume"
        );
        let gone = items
            .iter()
            .find(|i| i.volume_offset == v.offset && i.deleted && i.name == "to-delete-1.bin")
            .unwrap_or_else(|| panic!("{} deleted file", v.filesystem));
        assert_eq!(gone.data_state, DataState::Erased, "{}", v.filesystem);
    }
    assert!(items.iter().all(|i| i.origin == Origin::Metadata));
    assert_eq!(summary.erased, items.iter().filter(|i| i.data_state == DataState::Erased).count() as u64);
}

#[test]
#[ignore = "needs Windows-built fixtures"]
fn deep_scan_does_not_duplicate_files_known_from_metadata() {
    // The FS fixtures contain only random-data files, so a deep scan must add nothing that
    // duplicates a metadata item.
    let img = fixture("gpt-ntfs.img");
    let (quick, qitems, _) = run(&img, ScanMode::Quick);
    let (deep, ditems, progress) = run(&img, ScanMode::Deep);
    assert_eq!(quick.items, qitems.len() as u64);
    let meta_starts: std::collections::HashSet<u64> =
        qitems.iter().filter_map(|i| i.extents.first().and_then(|e| e.offset)).collect();
    for c in ditems.iter().filter(|i| i.origin == Origin::Carved) {
        assert!(!meta_starts.contains(&c.extents[0].offset.unwrap()), "duplicate carve {c:?}");
    }
    assert!(deep.carve.is_some());
    assert!(progress.iter().any(|p| p.phase == Phase::Carving));
    assert!(
        progress
            .windows(2)
            .filter(|w| w[0].phase == Phase::Carving && w[1].phase == Phase::Carving)
            .all(|w| w[0].bytes_done <= w[1].bytes_done)
    );
}

#[test]
#[ignore = "needs fixtures/out/carving.img"]
fn deep_scan_of_raw_media_carves_real_files() {
    let img = fixture("carving.img");
    let (summary, items, _) = run(&img, ScanMode::Deep);
    assert_eq!(summary.volumes[0].filesystem, "Unknown");
    let complete =
        items.iter().filter(|i| i.carve_status == Some(khodz_carve::CarveStatus::Complete)).count();
    // jpg, png, gif, zip, docx — plus the real GIF used as the gap inside the fragmented PNG,
    // which is itself a complete file at its own offset.
    assert_eq!(complete, 6);
    let docx = items.iter().find(|i| i.file_type.as_deref() == Some("docx")).unwrap();
    assert!(docx.path.starts_with("/$Carved/docx/"));
    assert_eq!(docx.quality, "exact");
}
