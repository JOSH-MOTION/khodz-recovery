//! Build the disk model from `diskutil list -plist` and `diskutil info -plist <id>` output.
//!
//! Platform-independent so it is unit tested everywhere. diskutil keys vary between macOS
//! releases, so every key is optional and several aliases are tried.
//!
//! APFS note: `diskutil` presents each APFS container as a *synthesized* whole disk (e.g.
//! `disk3`) whose `APFSPhysicalStores` point at the real partition (e.g. `disk0s2`). We do not
//! list synthesized disks as drives; their volumes are attached to the physical disk that holds
//! the store, which is what a recovery scan must read.

use plist::{Dictionary, Value};

use crate::{BusType, PhysicalDisk, Volume};

fn get_str<'a>(d: &'a Dictionary, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| d.get(k).and_then(Value::as_string)).map(str::trim).filter(|s| !s.is_empty())
}

fn get_u64(d: &Dictionary, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| d.get(k).and_then(Value::as_unsigned_integer))
}

fn get_bool(d: &Dictionary, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|k| d.get(k).and_then(Value::as_boolean))
}

fn ids_of(d: &Dictionary, key: &str) -> Vec<String> {
    d.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_dictionary)
                .filter_map(|e| get_str(e, &["DeviceIdentifier"]).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// `disk0s2` → `disk0`; `disk12` → `disk12`.
pub fn whole_disk_of(id: &str) -> Option<String> {
    let rest = id.strip_prefix("disk")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() { None } else { Some(format!("disk{digits}")) }
}

pub fn bus_from_protocol(protocol: &str) -> BusType {
    match protocol.to_ascii_lowercase().as_str() {
        "usb" => BusType::Usb,
        "sata" => BusType::Sata,
        "nvme" | "nvm express" => BusType::Nvme,
        "pci-express" | "pci express" | "pci" => BusType::PciExpress,
        "apple fabric" => BusType::AppleFabric,
        "thunderbolt" => BusType::Thunderbolt,
        "sas" => BusType::Sas,
        "scsi" => BusType::Scsi,
        "ata" => BusType::Ata,
        "atapi" => BusType::Atapi,
        "secure digital" | "sd" | "sdxc" => BusType::SdCard,
        "firewire" => BusType::Firewire,
        "fibre channel interface" | "fibre channel" => BusType::FibreChannel,
        "disk image" => BusType::DiskImage,
        "virtual interface" => BusType::Virtual,
        _ => BusType::Unknown,
    }
}

/// Partition types that are APFS container stores, not mountable volumes. Their volumes come
/// from the synthesized container disk.
fn is_apfs_store(content: &str) -> bool {
    content.starts_with("Apple_APFS") || content.eq_ignore_ascii_case("41504653-0000-11AA-AA11-00306543ECAC")
}

fn volume_from(entry: &Dictionary, info: Option<&Dictionary>) -> Option<Volume> {
    let id = get_str(entry, &["DeviceIdentifier"])?.to_string();
    let empty = Dictionary::new();
    let info = info.unwrap_or(&empty);
    let mount = get_str(entry, &["MountPoint"]).or_else(|| get_str(info, &["MountPoint"]));
    let encrypted = match (get_bool(info, &["FileVault"]), get_bool(info, &["Encryption", "Encrypted"])) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(false) || b.unwrap_or(false)),
    };
    Some(Volume {
        device_path: format!("/dev/r{id}"),
        mount_points: mount.map(|m| vec![m.to_string()]).unwrap_or_default(),
        label: get_str(entry, &["VolumeName"]).or_else(|| get_str(info, &["VolumeName"])).map(String::from),
        filesystem: get_str(info, &["FilesystemName", "FilesystemUserVisibleName", "FilesystemType"])
            .or_else(|| get_str(entry, &["Content"]))
            .map(String::from),
        size_bytes: get_u64(entry, &["Size"]).or_else(|| get_u64(info, &["Size", "TotalSize"])),
        free_bytes: get_u64(info, &["FreeSpace", "APFSContainerFree", "ContainerFree"]),
        offset_bytes: get_u64(info, &["PartitionMapPartitionOffset"]),
        encrypted,
        is_system: matches!(mount, Some("/") | Some("/System/Volumes/Data")),
        spans_multiple_disks: false,
        id,
    })
}

/// `list_xml`: output of `diskutil list -plist`. `info`: looks up `diskutil info -plist <id>`.
pub fn build_disks(
    list_xml: &[u8],
    info: impl Fn(&str) -> Option<Dictionary>,
) -> Result<Vec<PhysicalDisk>, String> {
    let root =
        Value::from_reader(std::io::Cursor::new(list_xml)).map_err(|e| format!("invalid plist: {e}"))?;
    let entries = root
        .as_dictionary()
        .and_then(|d| d.get("AllDisksAndPartitions"))
        .and_then(Value::as_array)
        .ok_or("diskutil output has no AllDisksAndPartitions")?;

    let mut disks: Vec<PhysicalDisk> = Vec::new();
    let mut containers: Vec<(Vec<String>, Vec<Dictionary>)> = Vec::new();

    for entry in entries.iter().filter_map(Value::as_dictionary) {
        let Some(id) = get_str(entry, &["DeviceIdentifier"]) else { continue };
        let apfs_volumes: Vec<Dictionary> = entry
            .get("APFSVolumes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_dictionary).cloned().collect())
            .unwrap_or_default();

        let stores = ids_of(entry, "APFSPhysicalStores");
        if !stores.is_empty() {
            containers.push((stores, apfs_volumes));
            continue;
        }

        let d_info = info(id).unwrap_or_default();
        let bus = get_str(&d_info, &["BusProtocol"]).map_or(BusType::Unknown, bus_from_protocol);
        let internal = get_bool(&d_info, &["Internal"]);
        let removable = get_bool(&d_info, &["Removable", "RemovableMedia"]).unwrap_or(false)
            || (internal == Some(false) && get_bool(&d_info, &["Ejectable"]).unwrap_or(false));
        let size =
            get_u64(entry, &["Size"]).or_else(|| get_u64(&d_info, &["TotalSize", "Size"])).unwrap_or(0);

        let mut volumes = Vec::new();
        let partitions = entry.get("Partitions").and_then(Value::as_array).cloned().unwrap_or_default();
        for p in partitions.iter().filter_map(Value::as_dictionary) {
            if get_str(p, &["Content"]).is_some_and(is_apfs_store) {
                continue;
            }
            let pid = get_str(p, &["DeviceIdentifier"]).unwrap_or_default();
            if let Some(v) = volume_from(p, info(pid).as_ref()) {
                volumes.push(v);
            }
        }
        // An unpartitioned APFS/HFS disk image can carry APFSVolumes directly.
        for v in &apfs_volumes {
            let vid = get_str(v, &["DeviceIdentifier"]).unwrap_or_default();
            volumes.extend(volume_from(v, info(vid).as_ref()));
        }

        let model = get_str(&d_info, &["MediaName", "IORegistryEntryName"]);
        disks.push(PhysicalDisk {
            id: id.to_string(),
            display_name: crate::display_name(None, model, id),
            device_path: format!("/dev/r{id}"),
            model: model.map(String::from),
            vendor: None,
            serial: None,
            size_bytes: size,
            logical_sector_size: get_u64(&d_info, &["DeviceBlockSize"])
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(512),
            physical_sector_size: None,
            bus,
            removable,
            is_system: false,
            is_virtual: bus == BusType::DiskImage
                || get_str(&d_info, &["VirtualOrPhysical"]) == Some("Virtual"),
            solid_state: get_bool(&d_info, &["SolidState"]),
            partition_scheme: get_str(entry, &["Content"]).map(String::from),
            media_present: size > 0,
            volumes,
        });
    }

    for (stores, vols) in containers {
        for store in stores {
            let Some(parent) = whole_disk_of(&store) else { continue };
            let Some(disk) = disks.iter_mut().find(|d| d.id == parent) else { continue };
            for v in &vols {
                let vid = get_str(v, &["DeviceIdentifier"]).unwrap_or_default();
                if disk.volumes.iter().any(|existing| existing.id == vid) {
                    continue;
                }
                disk.volumes.extend(volume_from(v, info(vid).as_ref()));
            }
        }
    }

    for d in &mut disks {
        d.is_system = d.volumes.iter().any(|v| v.is_system);
    }
    disks.sort_by_key(|d| d.id.strip_prefix("disk").and_then(|n| n.parse::<u32>().ok()).unwrap_or(u32::MAX));
    Ok(disks)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Hand-written to mirror `diskutil list -plist` on an Apple Silicon Mac with a USB stick.
    /// Must be re-validated against real output on a Mac (see TESTING.md).
    const LIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
 <key>AllDisksAndPartitions</key><array>
  <dict>
   <key>Content</key><string>GUID_partition_scheme</string>
   <key>DeviceIdentifier</key><string>disk0</string>
   <key>OSInternal</key><false/>
   <key>Size</key><integer>500277790720</integer>
   <key>Partitions</key><array>
    <dict><key>Content</key><string>Apple_APFS_ISC</string><key>DeviceIdentifier</key><string>disk0s1</string><key>Size</key><integer>524288000</integer></dict>
    <dict><key>Content</key><string>Apple_APFS</string><key>DeviceIdentifier</key><string>disk0s2</string><key>Size</key><integer>494384795648</integer></dict>
   </array>
  </dict>
  <dict>
   <key>APFSPhysicalStores</key><array><dict><key>DeviceIdentifier</key><string>disk0s2</string></dict></array>
   <key>APFSVolumes</key><array>
    <dict><key>DeviceIdentifier</key><string>disk3s1</string><key>MountPoint</key><string>/System/Volumes/Data</string><key>Size</key><integer>494384795648</integer><key>VolumeName</key><string>Data</string></dict>
    <dict><key>DeviceIdentifier</key><string>disk3s3</string><key>MountPoint</key><string>/</string><key>Size</key><integer>494384795648</integer><key>VolumeName</key><string>Macintosh HD</string></dict>
   </array>
   <key>Content</key><string>EF57347C-0000-11AA-AA11-00306543ECAC</string>
   <key>DeviceIdentifier</key><string>disk3</string>
   <key>Size</key><integer>494384795648</integer>
  </dict>
  <dict>
   <key>Content</key><string>FDisk_partition_scheme</string>
   <key>DeviceIdentifier</key><string>disk4</string>
   <key>Size</key><integer>30765219840</integer>
   <key>Partitions</key><array>
    <dict><key>Content</key><string>DOS_FAT_32</string><key>DeviceIdentifier</key><string>disk4s1</string><key>MountPoint</key><string>/Volumes/KINGSTON</string><key>Size</key><integer>30764171264</integer><key>VolumeName</key><string>KINGSTON</string></dict>
   </array>
  </dict>
 </array>
</dict></plist>"#;

    fn dict(pairs: &[(&str, Value)]) -> Dictionary {
        let mut d = Dictionary::new();
        for (k, v) in pairs {
            d.insert((*k).into(), v.clone());
        }
        d
    }

    fn infos() -> HashMap<&'static str, Dictionary> {
        HashMap::from([
            (
                "disk0",
                dict(&[
                    ("MediaName", "APPLE SSD AP0512Z".into()),
                    ("BusProtocol", "Apple Fabric".into()),
                    ("Internal", true.into()),
                    ("Removable", false.into()),
                    ("SolidState", true.into()),
                    ("DeviceBlockSize", 4096u64.into()),
                    ("VirtualOrPhysical", "Physical".into()),
                ]),
            ),
            (
                "disk3s1",
                dict(&[
                    ("FilesystemName", "APFS".into()),
                    ("FileVault", true.into()),
                    ("APFSContainerFree", 120_000_000_000u64.into()),
                ]),
            ),
            ("disk3s3", dict(&[("FilesystemName", "APFS".into()), ("FileVault", true.into())])),
            (
                "disk4",
                dict(&[
                    ("MediaName", "DataTraveler 3.0".into()),
                    ("BusProtocol", "USB".into()),
                    ("Internal", false.into()),
                    ("Removable", false.into()),
                    ("Ejectable", true.into()),
                    ("DeviceBlockSize", 512u64.into()),
                ]),
            ),
            (
                "disk4s1",
                dict(&[
                    ("FilesystemName", "MS-DOS (FAT32)".into()),
                    ("FreeSpace", 2_000_000_000u64.into()),
                    ("PartitionMapPartitionOffset", 1_048_576u64.into()),
                ]),
            ),
        ])
    }

    fn build() -> Vec<PhysicalDisk> {
        let infos = infos();
        build_disks(LIST.as_bytes(), |id| infos.get(id).cloned()).unwrap()
    }

    #[test]
    fn synthesized_apfs_containers_are_not_drives() {
        let ids: Vec<_> = build().iter().map(|d| d.id.clone()).collect();
        assert_eq!(ids, ["disk0", "disk4"]);
    }

    #[test]
    fn apfs_volumes_attach_to_the_physical_store_disk() {
        let disks = build();
        let internal = &disks[0];
        assert!(internal.is_system);
        assert_eq!(internal.bus, BusType::AppleFabric);
        assert_eq!(internal.logical_sector_size, 4096);
        assert_eq!(internal.solid_state, Some(true));
        assert_eq!(internal.device_path, "/dev/rdisk0");
        let names: Vec<_> = internal.volumes.iter().filter_map(|v| v.label.as_deref()).collect();
        assert_eq!(names, ["Data", "Macintosh HD"], "store/ISC partitions are not volumes");
        let data = &internal.volumes[0];
        assert_eq!(data.encrypted, Some(true));
        assert!(data.is_system);
        assert_eq!(data.free_bytes, Some(120_000_000_000));
    }

    #[test]
    fn external_usb_stick() {
        let usb = &build()[1];
        assert_eq!(usb.bus, BusType::Usb);
        assert!(usb.removable, "external + ejectable counts as removable");
        assert!(!usb.is_system);
        assert_eq!(usb.model.as_deref(), Some("DataTraveler 3.0"));
        assert_eq!(usb.partition_scheme.as_deref(), Some("FDisk_partition_scheme"));
        let v = &usb.volumes[0];
        assert_eq!(v.filesystem.as_deref(), Some("MS-DOS (FAT32)"));
        assert_eq!(v.mount_points, ["/Volumes/KINGSTON"]);
        assert_eq!(v.offset_bytes, Some(1_048_576));
        assert_eq!(v.encrypted, None);
        assert_eq!(v.device_path, "/dev/rdisk4s1");
    }

    #[test]
    fn tolerates_missing_info() {
        let disks = build_disks(LIST.as_bytes(), |_| None).unwrap();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[1].bus, BusType::Unknown);
        assert_eq!(disks[1].volumes[0].filesystem.as_deref(), Some("DOS_FAT_32"), "falls back to Content");
    }

    #[test]
    fn rejects_garbage() {
        assert!(build_disks(b"not a plist", |_| None).is_err());
        assert!(build_disks(br#"<plist version="1.0"><dict/></plist>"#, |_| None).is_err());
    }

    #[test]
    fn whole_disk_ids() {
        assert_eq!(whole_disk_of("disk0s2").as_deref(), Some("disk0"));
        assert_eq!(whole_disk_of("disk12").as_deref(), Some("disk12"));
        assert_eq!(whole_disk_of("diskX"), None);
        assert_eq!(whole_disk_of("sda1"), None);
    }
}
