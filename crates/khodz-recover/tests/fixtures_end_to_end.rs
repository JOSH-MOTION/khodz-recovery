//! Scan → recover → compare with the originals, on Windows-built and real-encoder fixtures.
//! Run: `cargo test -p khodz-recover --test fixtures_end_to_end -- --ignored --nocapture`
#![allow(clippy::arithmetic_side_effects)]

use std::path::PathBuf;
use std::time::SystemTime;

use khodz_core::CancelToken;
use khodz_engine::{DataState, ScanItem, ScanMode, scan};
use khodz_io::ImageFile;
use khodz_recover::*;
use sha2::{Digest, Sha256};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out")
}

fn json(name: &str) -> serde_json::Value {
    let t = std::fs::read_to_string(dir().join(name)).expect("generate fixtures first (see TESTING.md)");
    serde_json::from_str(t.trim_start_matches('\u{feff}')).unwrap()
}

fn hex(d: &[u8]) -> String {
    Sha256::digest(d).iter().map(|b| format!("{b:02x}")).collect()
}

fn to_recover(i: &ScanItem, id: i64, group: &str) -> RecoverItem {
    RecoverItem {
        id,
        rel_path: output_relative_path(group, &i.path),
        size: i.size,
        extents: i.extents.clone(),
        resident: i.resident.clone(),
        location_partial: i.quality == "partial",
        erased: i.data_state == DataState::Erased,
        modified_ms: i.modified_ms,
    }
}

fn scan_all(img: &ImageFile, mode: ScanMode) -> Vec<ScanItem> {
    let mut items = Vec::new();
    scan(img, mode, &CancelToken::new(), &mut |_| {}, &mut |i| {
        items.push(i);
        Ok(())
    })
    .unwrap();
    items
}

#[test]
#[ignore = "needs fixtures/out (both generators)"]
fn recovered_files_match_the_originals_byte_for_byte() {
    let fs_manifest = json("manifest.json");
    let out = tempfile::tempdir().unwrap();
    let mut verified = 0;

    // ---- every live file on every volume of the multi-partition Windows disk ----
    let path = dir().join("mbr-multi.img");
    let before = hex(&std::fs::read(&path).unwrap());
    let img = ImageFile::open(&path).unwrap();
    let items = scan_all(&img, ScanMode::Quick);
    let image =
        fs_manifest["images"].as_array().unwrap().iter().find(|i| i["file"] == "mbr-multi.img").unwrap();
    let session = create_session_dir(out.path(), SystemTime::now()).unwrap();
    for vol in image["volumes"].as_array().unwrap() {
        let offset = vol["offset"].as_u64().unwrap();
        let label = vol["label"].as_str().unwrap();
        let wanted: Vec<RecoverItem> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.volume_offset == offset && !i.is_dir && !i.system)
            .map(|(n, i)| to_recover(i, n as i64, label))
            .collect();
        let mut results = Vec::new();
        recover(&img, &wanted, &session, &CancelToken::new(), &mut |_| {}, &mut |r| {
            results.push(r.clone());
            Ok(())
        })
        .unwrap();
        for f in vol["files"].as_array().unwrap() {
            let rel = f["path"].as_str().unwrap();
            let on_disk = session.join(label).join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if f["deleted"].as_bool().unwrap() {
                // TRIM erased these: recovery must refuse rather than write zeros.
                if let Some(r) =
                    results.iter().find(|r| r.note.as_deref().is_some_and(|n| n.contains("erased")))
                {
                    assert_eq!(r.outcome, Outcome::Failed);
                }
                assert!(!on_disk.exists(), "{label}/{rel}: erased data must not produce a file");
                continue;
            }
            let data = std::fs::read(&on_disk).unwrap_or_else(|e| panic!("{label}/{rel}: {e}"));
            assert_eq!(hex(&data), f["sha256"].as_str().unwrap(), "{label}/{rel}");
            verified += 1;
        }
        let ok = results.iter().filter(|r| r.outcome == Outcome::Verified).count();
        println!(
            "{label}: {ok} verified, {} not recovered (erased)",
            results.iter().filter(|r| r.outcome == Outcome::Failed).count()
        );
    }
    drop(img);
    assert_eq!(hex(&std::fs::read(&path).unwrap()), before, "source image modified");

    // ---- signature-carved files from the real-encoder image ----
    let carving = json("carving-manifest.json");
    let path = dir().join("carving.img");
    let img = ImageFile::open(&path).unwrap();
    let items = scan_all(&img, ScanMode::Deep);
    let session2 = create_session_dir(out.path(), SystemTime::now()).unwrap();
    let wanted: Vec<RecoverItem> =
        items.iter().enumerate().map(|(n, i)| to_recover(i, n as i64, "Found by signature")).collect();
    let mut results = Vec::new();
    recover(&img, &wanted, &session2, &CancelToken::new(), &mut |_| {}, &mut |r| {
        results.push(r.clone());
        Ok(())
    })
    .unwrap();
    for f in carving["files"].as_array().unwrap() {
        let off = f["offset"].as_u64().unwrap();
        let idx = items.iter().position(|i| i.extents[0].offset == Some(off)).unwrap();
        let r = results.iter().find(|r| r.id == idx as i64).unwrap();
        match f["expect"].as_str().unwrap() {
            "complete" | "unverified" => {
                assert_eq!(r.outcome, Outcome::Verified, "{}", f["name"]);
                let data = std::fs::read(session2.join(r.output.as_ref().unwrap())).unwrap();
                assert_eq!(hex(&data), f["sha256"].as_str().unwrap(), "{}", f["name"]);
                verified += 1;
            }
            // Fragmented / truncated: written, but never claimed verified-complete.
            _ => assert_eq!(r.outcome, Outcome::PartialSource, "{} {:?}", f["name"], r),
        }
    }
    println!("verified {verified} recovered files against the originals");
    // 8 live files on each of the 3 volumes + 5 complete and 1 unverified (BMP) carved files.
    assert_eq!(verified, 3 * 8 + 6);
}
