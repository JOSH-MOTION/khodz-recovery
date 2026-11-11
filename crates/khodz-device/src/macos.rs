//! macOS device enumeration via `diskutil -plist` (metadata only; no device is opened).
//!
//! Decision: `diskutil` is Apple's supported interface, gives APFS container/volume
//! relationships directly, needs no privileges and no `unsafe` FFI. DiskArbitration/IOKit can
//! replace it later if hot-plug events are needed.

use std::process::Command;

use khodz_core::{Error, Result};

use crate::PhysicalDisk;
use crate::macos_parse::build_disks;

const DISKUTIL: &str = "/usr/sbin/diskutil";

fn diskutil(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new(DISKUTIL).args(args).output()?;
    if !out.status.success() {
        return Err(Error::InvalidData(format!(
            "diskutil {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

pub fn list_disks() -> Result<Vec<PhysicalDisk>> {
    let list = diskutil(&["list", "-plist"])?;
    build_disks(&list, |id| {
        if id.is_empty() || !id.starts_with("disk") || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let bytes = diskutil(&["info", "-plist", id]).ok()?;
        plist::Value::from_reader(std::io::Cursor::new(bytes)).ok()?.into_dictionary()
    })
    .map_err(Error::InvalidData)
}
