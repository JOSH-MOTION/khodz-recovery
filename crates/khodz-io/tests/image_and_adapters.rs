#![allow(clippy::arithmetic_side_effects)]

mod common;

use std::io::Write;
use std::sync::Arc;

use common::*;
use khodz_core::{CancelToken, Error};
use khodz_io::*;
use sha2::{Digest, Sha256};

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn write_image(len: usize, seed: u64) -> (tempfile::NamedTempFile, Vec<u8>) {
    let data = pattern(len, seed);
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(&data).unwrap();
    f.flush().unwrap();
    (f, data)
}

fn file_fingerprint(path: &std::path::Path) -> (String, std::time::SystemTime, u64) {
    let bytes = std::fs::read(path).unwrap();
    let meta = std::fs::metadata(path).unwrap();
    (sha256_hex(&bytes), meta.modified().unwrap(), meta.len())
}

#[test]
fn image_reads_are_exact_and_source_is_unchanged() {
    let (file, data) = write_image(3 * 1024 * 1024 + 123, 1);
    let before = file_fingerprint(file.path());

    let img = ImageFile::open(file.path()).unwrap();
    assert_eq!(img.len(), data.len() as u64);
    assert_eq!(img.sector_size(), 512);

    let mut rng = Lcg(42);
    let mut buf = vec![0u8; 70_000];
    for _ in 0..500 {
        let off = rng.below(data.len() as u64 + 1000);
        let len = rng.below(buf.len() as u64) as usize;
        let n = img.read_exact_at(off, &mut buf[..len]).unwrap();
        let expect = data.get(off as usize..).map_or(&[][..], |s| &s[..s.len().min(len)]);
        assert_eq!(&buf[..n], expect, "offset {off} len {len}");
    }
    assert_eq!(img.read_at(data.len() as u64, &mut buf).unwrap(), 0);
    assert_eq!(img.read_at(u64::MAX, &mut buf).unwrap(), 0);

    let fp = sha256_source(&img, &CancelToken::new(), |_| {}).unwrap();
    assert_eq!(fp.sha256, sha256_hex(&data));
    assert_eq!(fp.bytes, data.len() as u64);
    assert_eq!(fp.unreadable_bytes, 0);

    drop(img);
    assert_eq!(file_fingerprint(file.path()), before, "image content, mtime and size unchanged");
}

#[test]
fn read_only_image_files_open() {
    let (file, data) = write_image(64 * 1024, 2);
    let mut perms = std::fs::metadata(file.path()).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(file.path(), perms.clone()).unwrap();

    // Proves we never request write access: opening a read-only file would fail otherwise.
    let img = ImageFile::open(file.path()).unwrap();
    let mut buf = vec![0u8; data.len()];
    assert_eq!(img.read_exact_at(0, &mut buf).unwrap(), data.len());
    assert_eq!(buf, data);
    drop(img);

    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(file.path(), perms).unwrap();
}

#[test]
fn image_open_errors() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(ImageFile::open(dir.path().join("missing.img")), Err(Error::InvalidData(_))));
    assert!(ImageFile::open(dir.path()).is_err(), "directories are rejected");
    let (file, _) = write_image(512, 3);
    assert!(ImageFile::open(file.path()).unwrap().with_sector_size(1000).is_err());
    assert_eq!(ImageFile::open(file.path()).unwrap().with_sector_size(4096).unwrap().sector_size(), 4096);
}

#[test]
fn concurrent_reads_from_shared_image() {
    let (file, data) = write_image(2 * 1024 * 1024, 4);
    let img = Arc::new(ImageFile::open(file.path()).unwrap());
    let data = Arc::new(data);
    let threads: Vec<_> = (0..8u64)
        .map(|t| {
            let (img, data) = (Arc::clone(&img), Arc::clone(&data));
            std::thread::spawn(move || {
                let mut rng = Lcg(t + 100);
                let mut buf = vec![0u8; 8192];
                for _ in 0..300 {
                    let off = rng.below(data.len() as u64 - 8192);
                    img.read_exact_at(off, &mut buf).unwrap();
                    assert_eq!(&buf[..], &data[off as usize..off as usize + 8192]);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

#[test]
fn aligned_adapter_serves_any_range_with_only_aligned_reads() {
    for ss in [512u32, 4096] {
        let data = pattern(ss as usize * 37, ss as u64);
        let src = AlignedSource::new(StrictAligned(MemSource::new(data.clone(), ss)));
        let mut rng = Lcg(7);
        let mut buf = vec![0u8; ss as usize * 5];
        for _ in 0..2000 {
            let off = rng.below(data.len() as u64 + 10);
            let len = rng.below(buf.len() as u64) as usize;
            let n = src.read_exact_at(off, &mut buf[..len]).unwrap();
            let expect = data.get(off as usize..).map_or(&[][..], |s| &s[..s.len().min(len)]);
            assert_eq!(&buf[..n], expect, "ss {ss} off {off} len {len}");
        }
    }
}

#[test]
fn slices_cannot_escape_their_window() {
    let data = pattern(10_000, 5);
    let mem = MemSource::new(data.clone(), 512);
    assert!(SliceSource::new(&mem, 9_000, 2_000, "p").is_err());
    assert!(SliceSource::new(&mem, u64::MAX, 2, "p").is_err());

    let s = SliceSource::new(&mem, 1_000, 3_000, "partition 1").unwrap();
    assert_eq!(s.len(), 3_000);
    let mut buf = vec![0u8; 5_000];
    let n = s.read_exact_at(2_500, &mut buf).unwrap();
    assert_eq!(n, 500, "clamped at slice end");
    assert_eq!(&buf[..n], &data[3_500..4_000]);
    assert_eq!(s.read_at(3_000, &mut buf).unwrap(), 0);
    assert!(s.describe().contains("partition 1"));

    // Nested slices compose.
    let inner = SliceSource::new(&s, 100, 50, "inner").unwrap();
    let mut b = [0u8; 50];
    inner.read_exact_at(0, &mut b).unwrap();
    assert_eq!(&b[..], &data[1_100..1_150]);
}

#[test]
fn slice_rebases_media_error_offsets() {
    let f = Faulty { mem: MemSource::new(vec![0; 512 * 20], 512), bad_sectors: [12].into(), lost_at: None };
    let s = SliceSource::new(&f, 512 * 10, 512 * 10, "p").unwrap();
    let mut b = [0u8; 512];
    match s.read_at(512 * 2, &mut b) {
        Err(Error::MediaError { offset, .. }) => assert_eq!(offset, 1024),
        other => panic!("{other:?}"),
    }
}

#[test]
fn cache_is_correct_hits_repeat_reads_and_respects_its_bound() {
    let data = pattern(64 * 4096, 6);
    let counting = Counting::new(MemSource::new(data.clone(), 512));
    let cache = CachedSource::new(&counting, 4096, 4).unwrap();

    let mut buf = vec![0u8; 6000];
    cache.read_exact_at(1000, &mut buf).unwrap();
    assert_eq!(&buf[..], &data[1000..7000]);
    let reads = counting.reads();
    cache.read_exact_at(1500, &mut buf[..3000]).unwrap();
    assert_eq!(counting.reads(), reads, "second read served from cache");
    assert!(cache.stats().hits > 0);

    let mut rng = Lcg(9);
    for _ in 0..500 {
        let off = rng.below(data.len() as u64);
        let len = rng.below(9000) as usize;
        let n = cache.read_exact_at(off, &mut buf[..len.min(6000)]).unwrap();
        assert_eq!(&buf[..n], &data[off as usize..off as usize + n]);
        assert!(cache.stats().cached_chunks <= 4, "cache bound exceeded");
    }
    assert!(CachedSource::new(&counting, 1000, 4).is_err(), "chunk must be sector multiple");
    assert!(CachedSource::new(&counting, 4096, 0).is_err());
}

#[test]
fn bad_sectors_are_zero_filled_reported_and_skipped() {
    let data = pattern(512 * 300, 8);
    let src = Faulty {
        mem: MemSource::new(data.clone(), 512),
        bad_sectors: [5, 6, 100, 299].into(),
        lost_at: None,
    };
    let mut out = Vec::new();
    let report = for_each_chunk(
        &src,
        0..src.len(),
        64 * 512,
        &CancelToken::new(),
        |_| {},
        |_, d| {
            out.extend_from_slice(d);
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(report.bytes_read, data.len() as u64);
    let ranges: Vec<_> = report.bad.ranges().to_vec();
    // Sectors 5–6 merge into one range; 100 and 299 stand alone (299 * 512 = 153088).
    assert_eq!(ranges, vec![5 * 512..7 * 512, 100 * 512..101 * 512, 299 * 512..300 * 512]);
    assert_eq!(report.bad.total_bytes(), 4 * 512);
    for s in 0..300u64 {
        let r = (s * 512) as usize..((s + 1) * 512) as usize;
        if [5, 6, 100, 299].contains(&s) {
            assert!(out[r].iter().all(|&b| b == 0), "bad sector {s} zero-filled");
        } else {
            assert_eq!(out[r.clone()], data[r], "good sector {s} intact");
        }
    }

    let fp = sha256_source(&src, &CancelToken::new(), |_| {}).unwrap();
    assert_eq!(fp.unreadable_bytes, 2048, "fingerprint never hides unreadable data");
}

#[test]
fn device_loss_aborts_the_pass() {
    let src = Faulty {
        mem: MemSource::new(pattern(512 * 1000, 9), 512),
        bad_sectors: Default::default(),
        lost_at: Some(512 * 600),
    };
    let mut last = 0;
    let res = for_each_chunk(
        &src,
        0..src.len(),
        100 * 512,
        &CancelToken::new(),
        |p| last = p.bytes_done,
        |_, _| Ok(()),
    );
    assert!(matches!(res, Err(Error::DeviceLost(_))), "{res:?}");
    assert_eq!(last, 512 * 600, "progress reached the point of loss");
}

#[test]
fn a_run_of_failures_is_treated_as_device_loss() {
    let sectors = MAX_CONSECUTIVE_BAD_SECTORS as u64 + 50;
    let src = Faulty {
        mem: MemSource::new(vec![0; (512 * (sectors + 10)) as usize], 512),
        bad_sectors: (10..10 + sectors).collect(),
        lost_at: None,
    };
    let res = for_each_chunk(&src, 0..src.len(), 1 << 20, &CancelToken::new(), |_| {}, |_, _| Ok(()));
    assert!(matches!(res, Err(Error::DeviceLost(_))), "{res:?}");
}

#[test]
fn cancellation_stops_between_chunks() {
    let src = MemSource::new(pattern(1 << 20, 10), 512);
    let cancel = CancelToken::new();
    let mut calls = 0;
    let res = for_each_chunk(
        &src,
        0..src.len(),
        64 * 1024,
        &cancel,
        |_| cancel.cancel(),
        |_, _| {
            calls += 1;
            Ok(())
        },
    );
    assert!(matches!(res, Err(Error::Cancelled)));
    assert_eq!(calls, 1);
}

#[test]
fn progress_is_monotonic_and_complete() {
    let src = MemSource::new(pattern(1_000_000, 11), 512);
    let mut seen = Vec::new();
    for_each_chunk(&src, 1000..900_000, 65_536, &CancelToken::new(), |p| seen.push(p), |_, _| Ok(()))
        .unwrap();
    assert!(seen.windows(2).all(|w| w[0].bytes_done < w[1].bytes_done));
    let last = seen.last().unwrap();
    assert_eq!((last.bytes_done, last.bytes_total), (899_000, 899_000));
}

#[test]
fn read_tolerant_passes_through_non_media_errors() {
    let src =
        Faulty { mem: MemSource::new(vec![0; 4096], 512), bad_sectors: Default::default(), lost_at: Some(0) };
    let mut bad = BadRanges::default();
    assert!(matches!(read_tolerant(&src, 0, &mut [0u8; 512], &mut bad), Err(Error::DeviceLost(_))));
    assert!(bad.is_empty());
}
