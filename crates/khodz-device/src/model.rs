use serde::Serialize;

/// Storage interconnect as reported by the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BusType {
    Usb,
    Sata,
    Nvme,
    PciExpress,
    Sas,
    Scsi,
    Ata,
    Atapi,
    SdCard,
    Mmc,
    Thunderbolt,
    AppleFabric,
    Firewire,
    FibreChannel,
    Raid,
    Iscsi,
    StorageSpaces,
    Virtual,
    DiskImage,
    Ufs,
    Unknown,
}

/// A whole physical (or virtual) disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhysicalDisk {
    /// Stable short id: `PhysicalDrive0` (Windows) or `disk0` (macOS).
    pub id: String,
    /// Human-readable name, e.g. "SanDisk 3.2Gen1" (vendor + model, generic vendors dropped).
    pub display_name: String,
    /// Raw device path used for read-only access in later stages:
    /// `\\.\PhysicalDrive0` or `/dev/rdisk0`.
    pub device_path: String,
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub serial: Option<String>,
    pub size_bytes: u64,
    pub logical_sector_size: u32,
    pub physical_sector_size: Option<u32>,
    pub bus: BusType,
    pub removable: bool,
    /// Holds the running operating system. Recovering *to* this disk is discouraged and scanning
    /// it while in use cannot give a consistent snapshot.
    pub is_system: bool,
    pub is_virtual: bool,
    pub solid_state: Option<bool>,
    /// e.g. `GUID_partition_scheme` (macOS). Windows fills this in Stage 4 from our own parser.
    pub partition_scheme: Option<String>,
    /// False for empty card readers / removable drives without media.
    pub media_present: bool,
    pub volumes: Vec<Volume>,
}

/// Combine vendor and model into a display name. Placeholder vendors that some USB bridges and
/// ATA drivers report ("USB", "ATA", "Generic") are dropped, as is a vendor the model already
/// starts with.
pub fn display_name(vendor: Option<&str>, model: Option<&str>, fallback: &str) -> String {
    const GENERIC: [&str; 5] = ["usb", "ata", "generic", "scsi", "nvme"];
    let vendor = vendor
        .map(str::trim)
        .filter(|v| !v.is_empty() && !GENERIC.contains(&v.to_ascii_lowercase().as_str()));
    let model = model.map(str::trim).filter(|m| !m.is_empty());
    match (vendor, model) {
        (Some(v), Some(m)) if m.to_ascii_lowercase().starts_with(&v.to_ascii_lowercase()) => m.to_string(),
        (Some(v), Some(m)) => format!("{v} {m}"),
        (None, Some(m)) => m.to_string(),
        (Some(v), None) => v.to_string(),
        (None, None) => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::display_name;

    #[test]
    fn display_names() {
        assert_eq!(display_name(Some("Esmart"), Some("Tech"), "x"), "Esmart Tech");
        assert_eq!(display_name(Some("USB"), Some("SanDisk 3.2Gen1"), "x"), "SanDisk 3.2Gen1");
        assert_eq!(display_name(Some("SanDisk"), Some("SanDisk Ultra"), "x"), "SanDisk Ultra");
        assert_eq!(display_name(None, Some("APPLE SSD AP0512Z"), "x"), "APPLE SSD AP0512Z");
        assert_eq!(display_name(Some(" "), None, "Disk 3"), "Disk 3");
    }
}

/// A volume (formatted partition) located on a disk.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Volume {
    /// `\\?\Volume{GUID}\` (Windows) or `disk3s1` (macOS).
    pub id: String,
    /// `\\.\C:` / `\\?\Volume{GUID}` (Windows) or `/dev/rdisk3s1` (macOS).
    pub device_path: String,
    pub mount_points: Vec<String>,
    pub label: Option<String>,
    /// Filesystem name as the OS reports it (e.g. `NTFS`, `FAT32`, `MS-DOS (FAT32)`, `APFS`).
    /// `None` when the OS cannot read it (unformatted, locked, damaged).
    pub filesystem: Option<String>,
    pub size_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    /// Byte offset of the volume's start on its disk, when known.
    pub offset_bytes: Option<u64>,
    /// `Some(true)` when the OS reports encryption (FileVault, …); `None` = unknown.
    pub encrypted: Option<bool>,
    pub is_system: bool,
    /// Spanned/striped volumes are listed under each disk they touch.
    pub spans_multiple_disks: bool,
}
