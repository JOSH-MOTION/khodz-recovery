//! Runs device discovery against the machine's real hardware and checks invariants that hold
//! on any working Windows or macOS system. No device data is read.
#![cfg(any(windows, target_os = "macos"))]

use khodz_device::list_disks;

#[test]
fn finds_exactly_the_system_disk_and_its_os_volume() {
    let disks = list_disks().expect("enumeration succeeds without elevation");
    assert!(!disks.is_empty(), "every running system has at least one disk");

    let system: Vec<_> = disks.iter().filter(|d| d.is_system).collect();
    assert_eq!(system.len(), 1, "exactly one disk holds the OS: {disks:#?}");
    let os_volume =
        system[0].volumes.iter().find(|v| v.is_system).expect("OS volume attached to system disk");

    #[cfg(windows)]
    {
        let drive = std::env::var("SystemDrive").unwrap();
        assert!(os_volume.mount_points.iter().any(|m| m.eq_ignore_ascii_case(&format!("{drive}\\"))));
        assert_eq!(os_volume.filesystem.as_deref(), Some("NTFS"));
    }
    #[cfg(target_os = "macos")]
    assert!(os_volume.mount_points.iter().any(|m| m == "/" || m == "/System/Volumes/Data"));
}

#[test]
fn disk_and_volume_geometry_is_consistent() {
    for d in list_disks().unwrap() {
        if !d.media_present {
            assert!(d.volumes.is_empty(), "{}: no media but volumes listed", d.id);
            continue;
        }
        assert!(d.size_bytes > 0, "{}", d.id);
        assert!(matches!(d.logical_sector_size, 512 | 2048 | 4096), "{}: {}", d.id, d.logical_sector_size);
        #[cfg(windows)]
        assert!(d.device_path.starts_with(r"\\.\PhysicalDrive"));
        #[cfg(target_os = "macos")]
        assert!(d.device_path.starts_with("/dev/rdisk"));

        for v in &d.volumes {
            if let (Some(off), Some(size)) = (v.offset_bytes, v.size_bytes) {
                assert!(off.saturating_add(size) <= d.size_bytes, "{} {} exceeds disk", d.id, v.id);
                assert_eq!(off % u64::from(d.logical_sector_size), 0, "{} unaligned", v.id);
            }
            if let (Some(free), Some(size)) = (v.free_bytes, v.size_bytes) {
                assert!(free <= size, "{} free > size", v.id);
            }
        }
    }
}

#[test]
fn enumeration_is_deterministic() {
    let a = list_disks().unwrap();
    let b = list_disks().unwrap();
    let ids = |v: &[khodz_device::PhysicalDisk]| v.iter().map(|d| d.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&a), ids(&b));
}
