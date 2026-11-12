//! Read-only diagnostic: scan a device or image with the engine and dump every item found as
//! JSON lines, plus a summary, so recovery quality can be analysed offline.
//!
//! cargo run --release -p khodz-engine --example diagnose -- <source> <quick|deep> <out-dir>
//! (A raw device such as \\.\PhysicalDrive1 needs an elevated prompt on Windows.)
#![allow(clippy::arithmetic_side_effects)]

use std::collections::BTreeMap;
use std::io::Write;

use khodz_core::CancelToken;
use khodz_engine::{ScanMode, scan};
use khodz_io::BlockSource;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (source, mode, out) = (&args[1], &args[2], std::path::Path::new(&args[3]));
    std::fs::create_dir_all(out).unwrap();
    let mut log = std::fs::File::create(out.join("diagnose.log")).unwrap();
    let src: Box<dyn BlockSource> = if source.starts_with(r"\\.\") {
        match khodz_io::device::open_device(source) {
            Ok(d) => Box::new(d),
            Err(e) => {
                writeln!(log, "open failed: {e}").unwrap();
                std::process::exit(2);
            }
        }
    } else {
        Box::new(khodz_io::ImageFile::open(source).unwrap())
    };
    writeln!(log, "source {source}: {} bytes, sector {}", src.len(), src.sector_size()).unwrap();
    let layout = khodz_engine::partitions::inspect_layout(&*src).unwrap();
    writeln!(log, "layout: {}", serde_json::to_string_pretty(&layout).unwrap()).unwrap();

    let mode = if mode == "deep" { ScanMode::Deep } else { ScanMode::Quick };
    let mut items = std::io::BufWriter::new(std::fs::File::create(out.join("items.jsonl")).unwrap());
    let mut by_kind: BTreeMap<String, u64> = BTreeMap::new();
    let mut videos = std::fs::File::create(out.join("videos.txt")).unwrap();
    let mut last = std::time::Instant::now();
    let started = std::time::Instant::now();
    let summary = scan(
        &*src,
        mode,
        &CancelToken::new(),
        &mut |p| {
            if last.elapsed().as_secs() >= 5 {
                last = std::time::Instant::now();
                let _ =
                    writeln!(log, "{:?} {}/{} items {}", p.phase, p.bytes_done, p.bytes_total, p.items_found);
            }
        },
        &mut |mut i| {
            i.resident = None;
            let video = matches!(i.file_type.as_deref(), Some("mov" | "mp4" | "m4v" | "mkv"));
            if i.deleted && video && i.size > 65536 && !i.name.starts_with("._") {
                let _ = writeln!(
                    videos,
                    "VIDEO {:>8.1} MB  {:<18} overwritten={:<5} content={:<9} {}  | {}",
                    i.size as f64 / 1e6,
                    i.quality,
                    i.overwritten_pct.map_or("-".into(), |p| format!("{p}%")),
                    i.content_check.map_or("-".into(), |c| format!("{c:?}")),
                    i.path,
                    i.note.clone().unwrap_or_default()
                );
            }
            let key = format!(
                "{:?} deleted={} quality={} state={:?} carve={:?} type={}",
                i.origin,
                i.deleted,
                i.quality,
                i.data_state,
                i.carve_status,
                i.file_type.clone().unwrap_or_default()
            );
            *by_kind.entry(key).or_default() += 1;
            serde_json::to_writer(&mut items, &i).unwrap();
            items.write_all(b"\n").unwrap();
            Ok(())
        },
    );
    items.flush().unwrap();
    match summary {
        Ok(s) => writeln!(log, "summary: {}", serde_json::to_string_pretty(&s).unwrap()).unwrap(),
        Err(e) => writeln!(log, "scan error: {e}").unwrap(),
    }
    writeln!(log, "elapsed {:?}", started.elapsed()).unwrap();
    for (k, n) in by_kind {
        writeln!(log, "{n:>7}  {k}").unwrap();
    }
    writeln!(log, "done").unwrap();
}
