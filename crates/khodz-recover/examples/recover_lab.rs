//! Read-only lab tool: scan a source, then recover every file whose path contains a filter,
//! applying the same rules as the app (nothing written for erased / fully overwritten files;
//! known damage reported as incomplete). Prints one line per file.
//!
//! cargo run --release -p khodz-recover --example recover_lab -- <source> <path-filter> <out-dir>
#![allow(clippy::arithmetic_side_effects)]

use std::io::Write;
use std::time::SystemTime;

use khodz_core::CancelToken;
use khodz_engine::{ContentCheck, DataState, ScanMode, scan};
use khodz_io::BlockSource;
use khodz_recover::{RecoverItem, create_session_dir, output_relative_path, recover};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (source, filter, out) = (&args[1], &args[2], std::path::Path::new(&args[3]));
    std::fs::create_dir_all(out).unwrap();
    let mut log = std::fs::File::create(out.join("recover_lab.log")).unwrap();
    let src: Box<dyn BlockSource> = if source.starts_with(r"\\.\") {
        Box::new(khodz_io::device::open_device(source).unwrap())
    } else {
        Box::new(khodz_io::ImageFile::open(source).unwrap())
    };
    let mut items = Vec::new();
    scan(&*src, ScanMode::Quick, &CancelToken::new(), &mut |_| {}, &mut |i| {
        if !i.is_dir && i.path.contains(filter.as_str()) && !i.name.starts_with("._") {
            items.push(i);
        }
        Ok(())
    })
    .unwrap();
    let wanted: Vec<RecoverItem> = items
        .iter()
        .enumerate()
        .map(|(n, i)| {
            let gone = i.data_state == DataState::Erased
                || (i.overwritten_pct == Some(100) && i.content_check != Some(ContentCheck::Verified));
            RecoverItem {
                id: n as i64,
                rel_path: output_relative_path("recovered", &i.path),
                size: i.size,
                extents: i.extents.clone(),
                resident: i.resident.clone(),
                location_partial: i.quality == "partial",
                skip_reason: gone.then(|| "nothing left (erased or overwritten)".into()),
                damage: (i.content_check == Some(ContentCheck::Damaged))
                    .then(|| i.note.clone().unwrap_or_default()),
                modified_ms: i.modified_ms,
            }
        })
        .collect();
    let session = create_session_dir(out, SystemTime::now()).unwrap();
    let mut results = Vec::new();
    let summary = recover(&*src, &wanted, &session, &CancelToken::new(), &mut |_| {}, &mut |r| {
        results.push(r.clone());
        Ok(())
    })
    .unwrap();
    for (i, r) in items.iter().zip(&results) {
        writeln!(
            log,
            "{:<14} {:>8.1} MB  {}  -> {}  {}",
            r.outcome.as_str(),
            i.size as f64 / 1e6,
            i.path,
            r.output.as_ref().map_or("-".into(), |p| p.display().to_string()),
            r.note.clone().unwrap_or_default()
        )
        .unwrap();
    }
    writeln!(log, "summary: {summary:?}\nsession: {}", session.display()).unwrap();
}
