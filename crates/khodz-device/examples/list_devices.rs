//! Print detected disks and volumes. Read-only; no elevation needed.
//! `cargo run -p khodz-device --example list_devices`

use khodz_core::ByteSize;

fn main() {
    let disks = match khodz_device::list_disks() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("device discovery failed: {e}");
            std::process::exit(1);
        }
    };
    for d in &disks {
        let mut flags = Vec::new();
        if d.is_system {
            flags.push("system");
        }
        if d.removable {
            flags.push("removable");
        }
        if d.is_virtual {
            flags.push("virtual");
        }
        if d.solid_state == Some(true) {
            flags.push("ssd");
        }
        if !d.media_present {
            flags.push("no-media");
        }
        println!(
            "{}  {}  {:?}  {}  sector {}/{}  [{}]",
            d.id,
            d.display_name,
            d.bus,
            ByteSize(d.size_bytes),
            d.logical_sector_size,
            d.physical_sector_size.map_or("?".into(), |p| p.to_string()),
            flags.join(", "),
        );
        println!("    path {}", d.device_path);
        for v in &d.volumes {
            println!(
                "    - {:<10} {:<14} {:<8} size {:>10}  free {:>10}  @ {}{}",
                v.mount_points.first().map(String::as_str).unwrap_or("(none)"),
                v.label.as_deref().unwrap_or(""),
                v.filesystem.as_deref().unwrap_or("?"),
                v.size_bytes.map_or("?".into(), |s| ByteSize(s).to_string()),
                v.free_bytes.map_or("?".into(), |s| ByteSize(s).to_string()),
                v.offset_bytes.map_or("?".into(), |o| o.to_string()),
                if v.is_system { "  (system)" } else { "" },
            );
        }
    }
}
