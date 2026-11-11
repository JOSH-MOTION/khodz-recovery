//! Carving files produced by real encoders (fixtures/windows/make-carving-fixture.ps1).
//! Run: `cargo test -p khodz-carve --test encoder_fixture -- --ignored --nocapture`
#![allow(clippy::arithmetic_side_effects)]

use khodz_carve::{CarveStatus, Carved, carve};
use khodz_core::CancelToken;
use khodz_io::{BlockSource, ImageFile};
use sha2::{Digest, Sha256};

#[test]
#[ignore = "needs fixtures/out/carving.img: run fixtures/windows/make-carving-fixture.ps1"]
fn real_encoder_output_is_carved_honestly() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/out");
    let text =
        std::fs::read_to_string(dir.join("carving-manifest.json")).expect("run make-carving-fixture.ps1");
    let m: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    let path = dir.join(m["image"].as_str().unwrap());
    let before: Vec<u8> = std::fs::read(&path).unwrap();
    let img = ImageFile::open(&path).unwrap();

    let mut found: Vec<Carved> = Vec::new();
    let stats = carve(
        &img,
        0..img.len(),
        512,
        &CancelToken::new(),
        &|_| false,
        |_| {},
        |c| {
            found.push(c);
            Ok(())
        },
    )
    .unwrap();
    println!("{stats:?}");

    for f in m["files"].as_array().unwrap() {
        let name = f["name"].as_str().unwrap();
        let off = f["offset"].as_u64().unwrap();
        let len = f["length"].as_u64().unwrap();
        let c =
            found.iter().find(|c| c.offset == off).unwrap_or_else(|| panic!("{name}: not found at {off}"));
        println!(
            "{name:<28} {:?} {:?} len {} / {len} {}",
            c.kind,
            c.status,
            c.len,
            c.note.as_deref().unwrap_or("")
        );
        match f["expect"].as_str().unwrap() {
            "complete" => {
                assert_eq!(c.status, CarveStatus::Complete, "{name}");
                assert_eq!(c.len, len, "{name}: carved length");
                let mut data = vec![0u8; len as usize];
                img.read_exact_at(off, &mut data).unwrap();
                let hex: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(hex, f["sha256"].as_str().unwrap(), "{name}: content");
                assert_eq!(
                    format!("{:?}", c.kind).to_lowercase(),
                    f["kind"].as_str().unwrap(),
                    "{name}: type"
                );
            }
            "unverified" => {
                assert_eq!(c.status, CarveStatus::Unverified, "{name}");
                assert_eq!(c.len, len, "{name}");
            }
            "not_complete" => assert_ne!(c.status, CarveStatus::Complete, "{name} falsely complete"),
            other => panic!("bad expectation {other}"),
        }
    }
    drop(img);
    assert_eq!(std::fs::read(&path).unwrap(), before, "image modified");
}
