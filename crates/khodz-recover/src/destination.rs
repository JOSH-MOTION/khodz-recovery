use std::path::Path;

use serde::Serialize;

use crate::platform::{disks_for_device, disks_for_path, free_space};

/// Room kept free on the destination beyond the data itself (filesystem metadata, manifest).
const MARGIN: u64 = 64 << 20;

#[derive(Debug, Clone, Copy)]
pub enum SourceRef<'a> {
    Image(&'a Path),
    Device(&'a str),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DestinationCheck {
    pub destination: String,
    pub dest_disks: Vec<String>,
    pub source_disks: Vec<String>,
    /// The destination shares a physical disk with a *live device* source: writing there can
    /// overwrite the deleted data being recovered.
    pub same_disk: bool,
    pub free_bytes: Option<u64>,
    pub required_bytes: u64,
    /// Blocking problems.
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
}

impl DestinationCheck {
    /// Recovery may proceed. Same-disk recovery additionally needs an explicit override.
    pub fn ok(&self, allow_same_disk: bool) -> bool {
        self.problems.is_empty() && (!self.same_disk || allow_same_disk)
    }
}

fn human(n: u64) -> String {
    khodz_core::ByteSize(n).to_string()
}

/// Validate a destination folder before anything is written.
pub fn check_destination(dest: &Path, source: SourceRef<'_>, required_bytes: u64) -> DestinationCheck {
    let mut c = DestinationCheck {
        destination: dest.display().to_string(),
        dest_disks: Vec::new(),
        source_disks: Vec::new(),
        same_disk: false,
        free_bytes: None,
        required_bytes,
        problems: Vec::new(),
        warnings: Vec::new(),
    };

    if !dest.is_dir() {
        c.problems.push("The destination folder does not exist.".into());
        return c;
    }
    // Writability: create and remove a probe file (never touches existing files).
    let probe = dest.join(format!(".khodz-write-test-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) => c.problems.push(format!("Cannot write to the destination: {e}.")),
    }

    match free_space(dest) {
        Ok(free) => {
            c.free_bytes = Some(free);
            if free < required_bytes.saturating_add(MARGIN) {
                c.problems.push(format!(
                    "Not enough free space: {} needed (plus {} margin), {} available.",
                    human(required_bytes),
                    human(MARGIN),
                    human(free)
                ));
            }
        }
        Err(e) => c.warnings.push(format!("Free space could not be checked: {e}.")),
    }

    let dest_disks = disks_for_path(dest);
    let source_disks = match source {
        SourceRef::Device(d) => disks_for_device(d),
        SourceRef::Image(p) => disks_for_path(p),
    };
    match (&dest_disks, &source_disks, source) {
        (Ok(dd), Ok(sd), _) => {
            c.dest_disks = dd.clone();
            c.source_disks = sd.clone();
            let shared = dd.iter().any(|d| sd.contains(d));
            // Only a live device is at risk: recovered files from an image land in free space
            // that the (read-only) image file does not occupy.
            c.same_disk = shared && matches!(source, SourceRef::Device(_));
        }
        (Err(e), _, SourceRef::Device(_)) | (_, Err(e), SourceRef::Device(_)) => c.problems.push(format!(
            "Cannot confirm the destination is on a different physical disk than the source ({e}). Choose a folder on another local drive."
        )),
        (Err(e), _, _) | (_, Err(e), _) => c.warnings.push(format!("Disk layout could not be determined: {e}.")),
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_destination_is_a_problem() {
        let c =
            check_destination(Path::new("Z:/definitely/not/here"), SourceRef::Image(Path::new("x.img")), 1);
        assert!(!c.ok(true));
        assert!(c.problems[0].contains("does not exist"));
    }

    #[test]
    fn insufficient_space_is_a_problem() {
        let dir = tempfile::tempdir().unwrap();
        let c = check_destination(dir.path(), SourceRef::Image(Path::new("x.img")), u64::MAX / 2);
        assert!(c.problems.iter().any(|p| p.contains("Not enough free space")), "{c:?}");
    }

    #[cfg(windows)]
    #[test]
    fn same_physical_disk_is_detected_for_devices_only() {
        let dir = tempfile::tempdir().unwrap();
        let disk = disks_for_path(dir.path()).unwrap()[0].clone();
        let device = format!(r"\\.\{disk}");

        let c = check_destination(dir.path(), SourceRef::Device(&device), 1024);
        assert!(c.same_disk, "{c:?}");
        assert!(!c.ok(false), "blocked without override");
        assert!(c.ok(true), "allowed only with explicit override");

        // A different (possibly non-existent) disk number is not the same disk.
        let c = check_destination(dir.path(), SourceRef::Device(r"\\.\PhysicalDrive63"), 1024);
        assert!(!c.same_disk && c.ok(false), "{c:?}");

        // Images on the same disk are fine: the image file is never written.
        let img = dir.path().join("x.img");
        std::fs::write(&img, [0u8; 512]).unwrap();
        let c = check_destination(dir.path(), SourceRef::Image(&img), 1024);
        assert!(!c.same_disk && c.ok(false), "{c:?}");
        // The probe file was removed.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
