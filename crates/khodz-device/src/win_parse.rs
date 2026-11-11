//! Pure parsers for Windows storage IOCTL output. Kept platform-independent so they are unit
//! tested on every OS. Layouts follow the documented `winioctl.h` structures (little-endian).

use crate::BusType;

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const FILE_DEVICE_DISK: u32 = 0x07;
const IOCTL_STORAGE_BASE: u32 = 0x2d;
const IOCTL_VOLUME_BASE: u32 = 0x56;
const METHOD_BUFFERED: u32 = 0;
const FILE_ANY_ACCESS: u32 = 0;

/// All IOCTLs used for enumeration are `FILE_ANY_ACCESS`: they work on a handle with no read
/// or write rights.
pub const IOCTL_STORAGE_QUERY_PROPERTY: u32 =
    ctl_code(IOCTL_STORAGE_BASE, 0x500, METHOD_BUFFERED, FILE_ANY_ACCESS);
pub const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 =
    ctl_code(FILE_DEVICE_DISK, 0x28, METHOD_BUFFERED, FILE_ANY_ACCESS);
pub const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 =
    ctl_code(IOCTL_VOLUME_BASE, 0, METHOD_BUFFERED, FILE_ANY_ACCESS);

pub const STORAGE_DEVICE_PROPERTY: u32 = 0;
pub const STORAGE_ACCESS_ALIGNMENT_PROPERTY: u32 = 6;
pub const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;

/// `STORAGE_PROPERTY_QUERY { PropertyId, QueryType = PropertyStandardQuery, AdditionalParameters[1] }`
pub fn property_query(property_id: u32) -> [u8; 12] {
    let mut q = [0u8; 12];
    q[0..4].copy_from_slice(&property_id.to_le_bytes());
    q
}

fn read_u32(buf: &[u8], off: usize) -> Option<u32> {
    let bytes = buf.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_i64(buf: &[u8], off: usize) -> Option<i64> {
    let bytes = buf.get(off..off.checked_add(8)?)?;
    Some(i64::from_le_bytes(bytes.try_into().ok()?))
}

/// NUL-terminated ASCII string at `offset` (0 means "absent"). Trimmed; empty → None.
fn c_string_at(buf: &[u8], offset: u32) -> Option<String> {
    if offset == 0 {
        return None;
    }
    let tail = buf.get(usize::try_from(offset).ok()?..)?;
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    let s = String::from_utf8_lossy(&tail[..end]).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

#[derive(Debug, PartialEq)]
pub struct DeviceDescriptor {
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub removable: bool,
    pub bus: BusType,
}

/// Parse `STORAGE_DEVICE_DESCRIPTOR`.
pub fn parse_device_descriptor(buf: &[u8]) -> Option<DeviceDescriptor> {
    let removable = *buf.get(10)? != 0;
    Some(DeviceDescriptor {
        vendor: c_string_at(buf, read_u32(buf, 12)?),
        product: c_string_at(buf, read_u32(buf, 16)?),
        serial: c_string_at(buf, read_u32(buf, 24)?),
        removable,
        bus: bus_from_storage_bus_type(read_u32(buf, 28)?),
    })
}

/// `STORAGE_BUS_TYPE` → [`BusType`].
pub fn bus_from_storage_bus_type(v: u32) -> BusType {
    match v {
        0x01 => BusType::Scsi,
        0x02 => BusType::Atapi,
        0x03 => BusType::Ata,
        0x04 => BusType::Firewire,
        0x06 => BusType::FibreChannel,
        0x07 => BusType::Usb,
        0x08 => BusType::Raid,
        0x09 => BusType::Iscsi,
        0x0A => BusType::Sas,
        0x0B => BusType::Sata,
        0x0C => BusType::SdCard,
        0x0D => BusType::Mmc,
        0x0E => BusType::Virtual,
        0x0F => BusType::DiskImage,
        0x10 => BusType::StorageSpaces,
        0x11 => BusType::Nvme,
        0x13 => BusType::Ufs,
        _ => BusType::Unknown,
    }
}

/// `DISK_GEOMETRY_EX` → (bytes per sector, disk size).
pub fn parse_geometry_ex(buf: &[u8]) -> Option<(u32, u64)> {
    let bytes_per_sector = read_u32(buf, 20)?;
    let size = u64::try_from(read_i64(buf, 24)?).ok()?;
    Some((bytes_per_sector, size))
}

/// `STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR` → (logical, physical) sector size.
pub fn parse_access_alignment(buf: &[u8]) -> Option<(u32, u32)> {
    Some((read_u32(buf, 16)?, read_u32(buf, 20)?))
}

/// `DEVICE_SEEK_PENALTY_DESCRIPTOR` → incurs seek penalty (i.e. rotational).
pub fn parse_seek_penalty(buf: &[u8]) -> Option<bool> {
    Some(*buf.get(8)? != 0)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Extent {
    pub disk_index: u32,
    pub offset: u64,
    pub length: u64,
}

/// `VOLUME_DISK_EXTENTS` (count at 0, `DISK_EXTENT[24 bytes]` array from offset 8).
pub fn parse_disk_extents(buf: &[u8]) -> Vec<Extent> {
    const MAX_EXTENTS: u32 = 64;
    let Some(count) = read_u32(buf, 0) else { return Vec::new() };
    (0..count.min(MAX_EXTENTS))
        .map_while(|i| {
            let base = 8usize.checked_add(usize::try_from(i).ok()?.checked_mul(24)?)?;
            Some(Extent {
                disk_index: read_u32(buf, base)?,
                offset: u64::try_from(read_i64(buf, base.checked_add(8)?)?).ok()?,
                length: u64::try_from(read_i64(buf, base.checked_add(16)?)?).ok()?,
            })
        })
        .collect()
}

/// Split a `REG_MULTI_SZ`-style UTF-16 buffer (as returned by
/// `GetVolumePathNamesForVolumeNameW`) into strings.
pub fn split_multi_sz(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_codes_match_winioctl_h() {
        assert_eq!(IOCTL_STORAGE_QUERY_PROPERTY, 0x002D_1400);
        assert_eq!(IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, 0x0007_00A0);
        assert_eq!(IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, 0x0056_0000);
    }

    fn descriptor(vendor: &str, product: &str, serial: &str, removable: bool, bus: u32) -> Vec<u8> {
        let mut buf = vec![0u8; 64];
        buf[10] = removable as u8;
        buf[28..32].copy_from_slice(&bus.to_le_bytes());
        let mut put = |field: usize, s: &str| {
            if s.is_empty() {
                return;
            }
            let off = buf.len() as u32;
            buf[field..field + 4].copy_from_slice(&off.to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
        };
        put(12, vendor);
        put(16, product);
        put(24, serial);
        buf
    }

    #[test]
    fn parses_device_descriptor() {
        let buf = descriptor("  SanDisk ", "Ultra USB 3.0   ", "4C530001", true, 0x07);
        let d = parse_device_descriptor(&buf).unwrap();
        assert_eq!(d.vendor.as_deref(), Some("SanDisk"));
        assert_eq!(d.product.as_deref(), Some("Ultra USB 3.0"));
        assert_eq!(d.serial.as_deref(), Some("4C530001"));
        assert!(d.removable);
        assert_eq!(d.bus, BusType::Usb);
    }

    #[test]
    fn missing_strings_are_none_and_bad_offsets_do_not_panic() {
        let mut buf = descriptor("", "", "", false, 0x11);
        let d = parse_device_descriptor(&buf).unwrap();
        assert_eq!((d.vendor, d.product, d.serial, d.bus), (None, None, None, BusType::Nvme));
        // Offset pointing past the end of the buffer.
        buf[16..20].copy_from_slice(&9999u32.to_le_bytes());
        assert_eq!(parse_device_descriptor(&buf).unwrap().product, None);
        assert_eq!(parse_device_descriptor(&buf[..20]), None, "truncated buffer");
    }

    #[test]
    fn parses_geometry_and_alignment() {
        let mut g = vec![0u8; 32];
        g[20..24].copy_from_slice(&512u32.to_le_bytes());
        g[24..32].copy_from_slice(&256_060_514_304i64.to_le_bytes());
        assert_eq!(parse_geometry_ex(&g), Some((512, 256_060_514_304)));
        g[24..32].copy_from_slice(&(-1i64).to_le_bytes());
        assert_eq!(parse_geometry_ex(&g), None, "negative size rejected");

        let mut a = vec![0u8; 28];
        a[16..20].copy_from_slice(&512u32.to_le_bytes());
        a[20..24].copy_from_slice(&4096u32.to_le_bytes());
        assert_eq!(parse_access_alignment(&a), Some((512, 4096)));
    }

    #[test]
    fn parses_extents_and_caps_count() {
        let mut buf = vec![0u8; 8 + 2 * 24];
        buf[0..4].copy_from_slice(&2u32.to_le_bytes());
        for (i, (disk, off, len)) in [(0u32, 1_048_576i64, 4096i64), (3, 0, 8192)].into_iter().enumerate() {
            let b = 8 + i * 24;
            buf[b..b + 4].copy_from_slice(&disk.to_le_bytes());
            buf[b + 8..b + 16].copy_from_slice(&off.to_le_bytes());
            buf[b + 16..b + 24].copy_from_slice(&len.to_le_bytes());
        }
        let e = parse_disk_extents(&buf);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0], Extent { disk_index: 0, offset: 1_048_576, length: 4096 });
        assert_eq!(e[1].disk_index, 3);

        // Count claims more extents than the buffer holds: stop at the buffer end.
        buf[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse_disk_extents(&buf).len(), 2);
        assert!(parse_disk_extents(&[]).is_empty());
    }

    #[test]
    fn splits_multi_sz() {
        let w: Vec<u16> = "C:\\\0D:\\mnt\\\0\0garbage".encode_utf16().collect();
        assert_eq!(split_multi_sz(&w), vec!["C:\\", "D:\\mnt\\"]);
        assert!(split_multi_sz(&[0, 0]).is_empty());
    }
}
