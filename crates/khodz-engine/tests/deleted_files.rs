//! What is really left of deleted files, tested on the Windows-built FAT32 volume of
//! `mbr-multi.img` with the on-disk changes real deletions make, applied in memory:
//!
//! * a folder deleted the way macOS does it (folder entry marked deleted, the files inside keep
//!   live-looking entries, their cluster chains freed) — found on a real pen drive;
//! * a deleted file whose clusters were since taken by newer files (overwritten);
//! * a deleted file that had been written *around* another file (fragmented), recovered by
//!   checking its content.
//!
//! Run: `cargo test -p khodz-engine --test deleted_files -- --ignored`
#![allow(clippy::arithmetic_side_effects)]

use std::path::PathBuf;

use khodz_core::CancelToken;
use khodz_engine::*;
use khodz_io::MemSource;
use sha2::{Digest, Sha256};

const VOL: usize = 65_536;

fn image() -> Vec<u8> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    std::fs::read(dir.join("mbr-multi.img")).expect("generate or unpack fixtures first (see TESTING.md)")
}

fn manifest_sha(path: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let t = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
    let m: serde_json::Value = serde_json::from_str(t.trim_start_matches('\u{feff}')).unwrap();
    let img = m["images"].as_array().unwrap().iter().find(|i| i["file"] == "mbr-multi.img").unwrap();
    img["volumes"][0]["files"].as_array().unwrap().iter().find(|f| f["path"] == path).unwrap()["sha256"]
        .as_str()
        .unwrap()
        .to_string()
}

fn hex(d: &[u8]) -> String {
    Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect()
}

/// FAT32 geometry of the PFAT volume, read from its boot sector.
struct Fat {
    bps: usize,
    cs: usize,
    fat_offsets: Vec<usize>,
    data: usize,
    root: u32,
}

impl Fat {
    fn read(d: &[u8]) -> Fat {
        let b = &d[VOL..VOL + 512];
        let bps = u16::from_le_bytes([b[11], b[12]]) as usize;
        let cs = bps * b[13] as usize;
        let reserved = u16::from_le_bytes([b[14], b[15]]) as usize;
        let nfats = b[16] as usize;
        let fatsz = u32::from_le_bytes([b[36], b[37], b[38], b[39]]) as usize;
        let fat_offsets = (0..nfats).map(|i| VOL + (reserved + i * fatsz) * bps).collect();
        let data = VOL + (reserved + nfats * fatsz) * bps;
        Fat { bps, cs, fat_offsets, data, root: u32::from_le_bytes([b[44], b[45], b[46], b[47]]) }
    }
    fn cluster_at(&self, c: u32) -> usize {
        self.data + (c as usize - 2) * self.cs
    }
    fn cluster_of(&self, abs: u64) -> u32 {
        ((abs as usize - self.data) / self.cs) as u32 + 2
    }
    fn set(&self, d: &mut [u8], c: u32, v: u32) {
        for &f in &self.fat_offsets {
            d[f + c as usize * 4..f + c as usize * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    /// Byte offset of the 32-byte directory entry with this 8.3 name in the directory at `c`.
    fn entry(&self, d: &[u8], dir_cluster: u32, name83: &[u8; 11]) -> usize {
        let at = self.cluster_at(dir_cluster);
        (0..self.cs / 32).map(|i| at + i * 32).find(|&o| &d[o..o + 11] == name83).expect("entry not found")
    }
}

fn scan_all(d: Vec<u8>) -> Vec<ScanItem> {
    let src = MemSource::new(d, 512);
    let mut items = Vec::new();
    scan(&src, ScanMode::Quick, &CancelToken::new(), &mut |_| {}, &mut |i| {
        items.push(i);
        Ok(())
    })
    .unwrap();
    items
}

fn find<'a>(items: &'a [ScanItem], path: &str) -> &'a ScanItem {
    items
        .iter()
        .find(|i| {
            i.volume_offset == VOL as u64
                && (i.path == path
                    || (path.matches('/').count() > 1
                        && i.path.ends_with(&path[path[1..].find('/').map_or(0, |p| p + 1)..])))
        })
        .unwrap_or_else(|| panic!("{path}"))
}

fn read(d: &[u8], i: &ScanItem) -> Vec<u8> {
    read_item_range(&MemSource::new(d.to_vec(), 512), &i.extents, None, i.size, 0, usize::MAX).unwrap()
}

/// Clusters of a live file, from the intact scan.
fn clusters(fat: &Fat, i: &ScanItem) -> Vec<u32> {
    let mut out = Vec::new();
    for e in &i.extents {
        let first = fat.cluster_of(e.offset.unwrap());
        out.extend(first..first + e.len.div_ceil(fat.cs as u64) as u32);
    }
    out
}

/// Delete the `docs` folder like macOS: the folder's own entry is marked deleted, the entries
/// inside are left alone, and every cluster chain under it is freed.
fn delete_docs_like_macos(d: &mut [u8], fat: &Fat, intact: &[ScanItem]) {
    let e = fat.entry(d, fat.root, b"DOCS       ");
    d[e] = 0xE5;
    // Only this FAT32 volume's files: the other volumes have their own /docs folders.
    for i in intact
        .iter()
        .filter(|i| i.volume_offset == VOL as u64 && i.path.starts_with("/docs/") && !i.deleted && !i.is_dir)
    {
        for c in clusters(fat, i) {
            fat.set(d, c, 0);
        }
    }
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn files_in_a_folder_deleted_like_macos_are_located_and_intact() {
    let mut d = image();
    let fat = Fat::read(&d);
    assert_eq!(fat.bps, 512);
    let intact = scan_all(d.clone());
    delete_docs_like_macos(&mut d, &fat, &intact);
    let items = scan_all(d.clone());
    for path in ["docs/report.bin", "docs/nested/deep/image.dat"] {
        let i = find(&items, &format!("/{path}"));
        assert!(i.deleted, "{path} is inside a deleted folder");
        assert_eq!(i.overwritten_pct, Some(0), "{path}: nothing else uses its clusters");
        assert_ne!(i.quality, "partial", "{path}: location known from its entry, not just one cluster");
        let orig = find(&intact, &format!("/{path}"));
        assert_eq!(
            hex(&read(&image(), orig)),
            manifest_sha(path),
            "{path}: intact image matches the manifest"
        );
        let clean = image();
        let at = orig.extents[0].offset.unwrap() as usize;
        let diffs: Vec<usize> = (at..at + orig.size as usize).filter(|&k| clean[k] != d[k]).collect();
        assert!(
            diffs.is_empty(),
            "test edits touched file data at {} bytes, first at {:?} (data area starts {}, FATs at {:?})",
            diffs.len(),
            diffs.first(),
            fat.data,
            fat.fat_offsets
        );
        assert_eq!(
            hex(&read(&d, i)),
            manifest_sha(path),
            "{path} recovered byte for byte: found {} {:?} q={}, originally {:?}",
            i.path,
            i.extents,
            i.quality,
            orig.extents
        );
    }
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn a_deleted_file_whose_space_was_reused_is_reported_overwritten() {
    let mut d = image();
    let fat = Fat::read(&d);
    let intact = scan_all(d.clone());
    delete_docs_like_macos(&mut d, &fat, &intact);
    // Newer files take report.bin's clusters.
    let report = find(&intact, "/docs/report.bin");
    for c in clusters(&fat, report) {
        fat.set(&mut d, c, 0x0FFF_FFFF);
    }
    let items = scan_all(d.clone());
    let i = find(&items, "/docs/report.bin");
    assert_eq!(i.overwritten_pct, Some(100));
    assert!(i.note.as_deref().unwrap().contains("overwritten"), "{:?}", i.note);
    // The neighbour that was not touched is still whole.
    assert_eq!(find(&items, "/docs/nested/deep/image.dat").overwritten_pct, Some(0));
}

/// A valid PNG of about `len` bytes (every chunk CRC correct; the validator checks them all).
fn png(len: usize) -> Vec<u8> {
    fn chunk(t: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c = (body.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(t);
        c.extend_from_slice(body);
        let crc = crc32fast::hash(&[&t[..], body].concat());
        c.extend(crc.to_be_bytes());
        c
    }
    let mut ihdr = Vec::new();
    ihdr.extend(64u32.to_be_bytes());
    ihdr.extend(64u32.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    let idat: Vec<u8> = (0..len - 57).map(|i| (i * 7 % 251) as u8).collect();
    [b"\x89PNG\r\n\x1a\n".to_vec(), chunk(b"IHDR", &ihdr), chunk(b"IDAT", &idat), chunk(b"IEND", &[])]
        .concat()
}

#[test]
#[ignore = "needs fixtures/out/mbr-multi.img"]
fn a_fragmented_deleted_file_is_rebuilt_around_the_file_it_was_written_around() {
    let mut d = image();
    let fat = Fat::read(&d);
    let intact = scan_all(d.clone());
    let report = find(&intact, "/docs/report.bin");
    let cs = clusters(&fat, report);
    assert!(cs.windows(2).all(|w| w[1] == w[0] + 1), "fixture file is contiguous");

    // Turn report.bin into a deleted PNG that was written around one cluster another file
    // still owns: first half, [foreign cluster], second half.
    let data = png(cs.len() / 2 * fat.cs);
    let gap = cs[cs.len() / 4];
    let mut pos = 0;
    for &c in &cs {
        let at = fat.cluster_at(c);
        if c == gap {
            d[at..at + fat.cs].fill(0xAB);
            continue;
        }
        let n = (data.len() - pos).min(fat.cs);
        d[at..at + n].copy_from_slice(&data[pos..pos + n]);
        pos += n;
        if pos == data.len() {
            break;
        }
    }
    let docs_cluster = {
        let e = fat.entry(&d, fat.root, b"DOCS       ");
        u32::from(u16::from_le_bytes([d[e + 26], d[e + 27]]))
            | (u32::from(u16::from_le_bytes([d[e + 20], d[e + 21]])) << 16)
    };
    let e = fat.entry(&d, docs_cluster, b"REPORT  BIN");
    d[e + 8..e + 11].copy_from_slice(b"PNG");
    d[e + 28..e + 32].copy_from_slice(&(data.len() as u32).to_le_bytes());
    d[e] = 0xE5;
    for &c in &cs {
        fat.set(&mut d, c, if c == gap { 0x0FFF_FFFF } else { 0 });
    }

    let items = scan_all(d.clone());
    let i = items
        .iter()
        .find(|i| i.deleted && i.file_type.as_deref() == Some("png") && i.size == data.len() as u64)
        .unwrap();
    assert_eq!(i.content_check, Some(ContentCheck::Verified), "{:?}", i.note);
    assert!(i.note.as_deref().unwrap().contains("rebuilt around other files"), "{:?}", i.note);
    assert_eq!(i.overwritten_pct, Some(0));
    assert!(i.extents.len() >= 2, "the foreign cluster is skipped");
    assert_eq!(read(&d, i), data, "recovered byte for byte");
}
