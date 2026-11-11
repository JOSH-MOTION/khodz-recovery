//! Attach Windows volumes to the physical disks they live on.

use crate::win_parse::Extent;
use crate::{PhysicalDisk, Volume};

pub struct RawDisk {
    pub index: u32,
    pub disk: PhysicalDisk,
}

pub struct RawVolume {
    pub volume: Volume,
    pub extents: Vec<Extent>,
}

/// `system_root` is the OS volume's root path, e.g. `C:\`.
pub fn assemble(
    mut disks: Vec<RawDisk>,
    volumes: Vec<RawVolume>,
    system_root: Option<&str>,
) -> Vec<PhysicalDisk> {
    disks.sort_by_key(|d| d.index);
    for raw in volumes {
        let is_system = system_root
            .is_some_and(|root| raw.volume.mount_points.iter().any(|m| m.eq_ignore_ascii_case(root)));
        let mut disk_indices: Vec<u32> = raw.extents.iter().map(|e| e.disk_index).collect();
        disk_indices.sort_unstable();
        disk_indices.dedup();
        let spans = disk_indices.len() > 1;

        for extent in &raw.extents {
            let Some(target) = disks.iter_mut().find(|d| d.index == extent.disk_index) else { continue };
            // Only the first extent on a given disk is listed.
            if target.disk.volumes.iter().any(|v| v.id == raw.volume.id) {
                continue;
            }
            let mut v = raw.volume.clone();
            v.is_system = is_system;
            v.spans_multiple_disks = spans;
            v.offset_bytes = Some(extent.offset);
            if v.size_bytes.is_none() && !spans {
                v.size_bytes = Some(extent.length);
            }
            target.disk.is_system |= is_system;
            target.disk.volumes.push(v);
        }
    }
    let mut out: Vec<PhysicalDisk> = disks.into_iter().map(|d| d.disk).collect();
    for d in &mut out {
        d.volumes.sort_by_key(|v| v.offset_bytes.unwrap_or(u64::MAX));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BusType;

    fn disk(index: u32) -> RawDisk {
        RawDisk {
            index,
            disk: PhysicalDisk {
                id: format!("PhysicalDrive{index}"),
                display_name: format!("Disk {index}"),
                device_path: format!(r"\\.\PhysicalDrive{index}"),
                model: None,
                vendor: None,
                serial: None,
                size_bytes: 1 << 30,
                logical_sector_size: 512,
                physical_sector_size: None,
                bus: BusType::Sata,
                removable: false,
                is_system: false,
                is_virtual: false,
                solid_state: None,
                partition_scheme: None,
                media_present: true,
                volumes: vec![],
            },
        }
    }

    fn volume(id: &str, mounts: &[&str], extents: &[(u32, u64, u64)]) -> RawVolume {
        RawVolume {
            volume: Volume {
                id: id.into(),
                device_path: id.into(),
                mount_points: mounts.iter().map(|s| s.to_string()).collect(),
                label: None,
                filesystem: Some("NTFS".into()),
                size_bytes: None,
                free_bytes: None,
                offset_bytes: None,
                encrypted: None,
                is_system: false,
                spans_multiple_disks: false,
            },
            extents: extents
                .iter()
                .map(|&(d, o, l)| Extent { disk_index: d, offset: o, length: l })
                .collect(),
        }
    }

    #[test]
    fn attaches_volumes_sorted_and_marks_system_disk() {
        let out = assemble(
            vec![disk(1), disk(0)],
            vec![
                volume("C", &["c:\\"], &[(0, 500, 100)]),
                volume("recovery", &[], &[(0, 100, 50)]),
                volume("E", &["E:\\"], &[(1, 1024, 4096)]),
            ],
            Some("C:\\"),
        );
        assert_eq!(out[0].id, "PhysicalDrive0", "disks sorted by index");
        assert!(out[0].is_system);
        assert!(!out[1].is_system);
        let ids: Vec<_> = out[0].volumes.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["recovery", "C"], "volumes sorted by offset");
        assert!(out[0].volumes[1].is_system);
        assert_eq!(out[0].volumes[1].size_bytes, Some(100), "size falls back to extent length");
        assert_eq!(out[1].volumes[0].offset_bytes, Some(1024));
    }

    #[test]
    fn spanned_volume_listed_under_each_disk_once() {
        let out = assemble(
            vec![disk(0), disk(1)],
            vec![volume("span", &["S:\\"], &[(0, 0, 10), (1, 0, 10), (1, 20, 10)])],
            None,
        );
        assert_eq!(out[0].volumes.len(), 1);
        assert_eq!(out[1].volumes.len(), 1);
        assert!(out[1].volumes[0].spans_multiple_disks);
        assert_eq!(out[1].volumes[0].size_bytes, None, "extent length is not the volume size");
    }

    #[test]
    fn volumes_on_unknown_disks_or_without_extents_are_dropped() {
        let out = assemble(vec![disk(0)], vec![volume("x", &[], &[(9, 0, 1)]), volume("y", &[], &[])], None);
        assert!(out[0].volumes.is_empty());
    }
}
