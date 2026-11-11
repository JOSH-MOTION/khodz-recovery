//! List every entry (live and deleted) on each volume of a raw disk image. Read-only.
//! `cargo run -p khodz-fs --example walk -- <image> [--deleted-only]`

use khodz_core::{ByteSize, CancelToken};
use khodz_io::{BlockSource, ImageFile, SliceSource};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: walk <image> [--deleted-only]");
        std::process::exit(2);
    };
    let deleted_only = args.any(|a| a == "--deleted-only");
    let img = ImageFile::open(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let table = khodz_part::read_partition_table(&img).expect("partition table");
    println!("{:?} table, {} partitions, warnings: {:?}", table.kind, table.partitions.len(), table.warnings);

    let volumes: Vec<(u64, u64, String)> = if table.partitions.is_empty() {
        vec![(0, img.len(), "whole image".into())]
    } else {
        table
            .partitions
            .iter()
            .filter(|p| !p.is_container())
            .map(|p| (p.offset, p.size, format!("partition {} ({})", p.number, p.type_name)))
            .collect()
    };
    for (off, size, label) in volumes {
        let slice = SliceSource::new(&img, off, size, label.clone()).expect("slice");
        let info = khodz_fs::detect(&slice).expect("detect");
        println!("\n== {label} @ {off}: {} label={:?}", info.kind.display_name(), info.label);
        let Ok(fs) = khodz_fs::open(&slice) else { continue };
        let stats = fs
            .walk(&CancelToken::new(), &mut |e| {
                if !deleted_only || e.deleted {
                    println!(
                        "{} {:<50} {:>10} {:?} extents={} id={}",
                        if e.deleted { "DEL" } else { "   " },
                        e.path,
                        ByteSize(e.size).to_string(),
                        e.quality,
                        e.extents.len(),
                        e.id
                    );
                }
                Ok(())
            })
            .expect("walk");
        println!("stats: {stats:?}");
    }
}
