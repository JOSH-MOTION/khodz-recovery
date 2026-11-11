//! Windows device enumeration.
//!
//! Every handle is opened with **zero access rights** (`dwDesiredAccess = 0`). That is enough for
//! the `FILE_ANY_ACCESS` metadata IOCTLs used here and makes reading or writing data through
//! these handles impossible. No elevation is required.
#![allow(unsafe_code)] // FFI only; each block below has a SAFETY note.

use std::io;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    GetDiskFreeSpaceExW, GetVolumeInformationW, GetVolumePathNamesForVolumeNameW, OPEN_EXISTING,
};
use windows_sys::Win32::System::Diagnostics::Debug::SetThreadErrorMode;
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::assemble::{RawDisk, RawVolume, assemble};
use crate::win_parse::*;
use crate::{PhysicalDisk, Volume};

/// Desired access for every handle this module opens: none.
const DESIRED_ACCESS: u32 = 0;
/// Windows assigns `PhysicalDriveN` numbers densely but may leave gaps after removal; probe a
/// fixed range rather than stopping at the first missing index.
const MAX_DISK_INDEX: u32 = 64;
const ERROR_MORE_DATA: u32 = 234;
const SEM_FAILCRITICALERRORS: u32 = 0x0001;
const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0 is a valid handle returned by CreateFileW and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn open_metadata_only(path: &str) -> io::Result<Handle> {
    let w = wide(path);
    // SAFETY: `w` is NUL-terminated and outlives the call; optional pointers are null.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            DESIRED_ACCESS,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Handle(h)) }
}

fn ioctl(h: &Handle, code: u32, input: &[u8], out: &mut [u8]) -> io::Result<usize> {
    let mut returned = 0u32;
    // SAFETY: buffers are valid for the lengths passed; synchronous call (no OVERLAPPED).
    let ok = unsafe {
        DeviceIoControl(
            h.0,
            code,
            if input.is_empty() { null() } else { input.as_ptr().cast() },
            u32::try_from(input.len()).unwrap_or(0),
            out.as_mut_ptr().cast(),
            u32::try_from(out.len()).unwrap_or(0),
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(usize::try_from(returned).unwrap_or(0).min(out.len()))
}

fn query_property(h: &Handle, property: u32) -> io::Result<Vec<u8>> {
    let mut out = vec![0u8; 4096];
    let n = ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, &property_query(property), &mut out)?;
    out.truncate(n);
    Ok(out)
}

/// Suppresses "There is no disk in the drive" dialogs while probing empty removable drives.
struct QuietErrors(u32);

impl QuietErrors {
    fn new() -> Self {
        let mut old = 0u32;
        // SAFETY: valid out-pointer; affects only the calling thread.
        unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX, &mut old) };
        Self(old)
    }
}

impl Drop for QuietErrors {
    fn drop(&mut self) {
        // SAFETY: restores the mode captured in `new`.
        unsafe { SetThreadErrorMode(self.0, null_mut()) };
    }
}

fn probe_disk(index: u32) -> Option<RawDisk> {
    let path = format!(r"\\.\PhysicalDrive{index}");
    let h = open_metadata_only(&path).ok()?;

    let desc = query_property(&h, STORAGE_DEVICE_PROPERTY).ok().and_then(|b| parse_device_descriptor(&b));
    let mut geo = [0u8; 256];
    let geometry = ioctl(&h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut geo)
        .ok()
        .and_then(|n| parse_geometry_ex(&geo[..n]));
    let alignment =
        query_property(&h, STORAGE_ACCESS_ALIGNMENT_PROPERTY).ok().and_then(|b| parse_access_alignment(&b));
    let seek_penalty =
        query_property(&h, STORAGE_DEVICE_SEEK_PENALTY_PROPERTY).ok().and_then(|b| parse_seek_penalty(&b));

    let (vendor, model, serial, removable, bus) = match desc {
        Some(d) => (d.vendor, d.product, d.serial, d.removable, d.bus),
        None => (None, None, None, false, crate::BusType::Unknown),
    };
    let is_virtual = matches!(bus, crate::BusType::Virtual | crate::BusType::DiskImage);
    Some(RawDisk {
        index,
        disk: PhysicalDisk {
            display_name: crate::display_name(vendor.as_deref(), model.as_deref(), &format!("Disk {index}")),
            id: format!("PhysicalDrive{index}"),
            device_path: path,
            model,
            vendor,
            serial,
            size_bytes: geometry.map_or(0, |g| g.1),
            logical_sector_size: alignment.map(|a| a.0).or(geometry.map(|g| g.0)).unwrap_or(512),
            physical_sector_size: alignment.map(|a| a.1),
            bus,
            removable,
            is_system: false,
            is_virtual,
            solid_state: seek_penalty.map(|p| !p),
            partition_scheme: None,
            // Card readers / removable drives with no media fail the geometry query.
            media_present: geometry.is_some_and(|g| g.1 > 0),
            volumes: Vec::new(),
        },
    })
}

fn mount_points(volume_name: &[u16]) -> Vec<String> {
    let mut buf = vec![0u16; 1024];
    loop {
        let mut needed = 0u32;
        // SAFETY: `volume_name` is NUL-terminated; buf length passed matches allocation.
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                volume_name.as_ptr(),
                buf.as_mut_ptr(),
                u32::try_from(buf.len()).unwrap_or(0),
                &mut needed,
            )
        };
        if ok != 0 {
            return split_multi_sz(&buf);
        }
        // SAFETY: trivially safe.
        let err = unsafe { GetLastError() };
        let needed = usize::try_from(needed).unwrap_or(0);
        if err == ERROR_MORE_DATA && needed > buf.len() && needed <= 1 << 16 {
            buf = vec![0u16; needed];
        } else {
            return Vec::new();
        }
    }
}

fn describe_volume(guid_path: &str) -> RawVolume {
    let root = wide(guid_path); // with trailing backslash
    let mounts = mount_points(&root);

    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    // SAFETY: buffers sized as passed; unused out-params are null.
    let info_ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            label.as_mut_ptr(),
            261,
            null_mut(),
            null_mut(),
            null_mut(),
            fs.as_mut_ptr(),
            261,
        )
    } != 0;

    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: valid out-pointers.
    let space_ok = unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0;

    let no_slash = guid_path.trim_end_matches('\\');
    let extents = open_metadata_only(no_slash)
        .ok()
        .and_then(|h| {
            let mut out = vec![0u8; 8 + 24 * 64];
            ioctl(&h, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[], &mut out).ok().map(|n| {
                out.truncate(n);
                parse_disk_extents(&out)
            })
        })
        .unwrap_or_default();

    let letter_path = mounts
        .iter()
        .find(|m| m.len() == 3 && m.ends_with(":\\"))
        .map(|m| format!(r"\\.\{}", m.trim_end_matches('\\')));
    let non_empty = |s: String| if s.is_empty() { None } else { Some(s) };

    RawVolume {
        volume: Volume {
            id: guid_path.to_string(),
            device_path: letter_path.unwrap_or_else(|| no_slash.to_string()),
            mount_points: mounts,
            label: if info_ok { non_empty(from_wide(&label)) } else { None },
            filesystem: if info_ok { non_empty(from_wide(&fs)) } else { None },
            size_bytes: space_ok.then_some(total),
            free_bytes: space_ok.then_some(free),
            offset_bytes: None,
            // BitLocker status needs WMI or elevation; reported in a later stage.
            encrypted: None,
            is_system: false,
            spans_multiple_disks: false,
        },
        extents,
    }
}

fn enumerate_volumes() -> Vec<RawVolume> {
    let mut name = [0u16; 512];
    // SAFETY: buffer length matches allocation.
    let find = unsafe { FindFirstVolumeW(name.as_mut_ptr(), 512) };
    if find == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut out = Vec::new();
    loop {
        out.push(describe_volume(&from_wide(&name)));
        // SAFETY: `find` is a valid search handle; buffer length matches allocation.
        if unsafe { FindNextVolumeW(find, name.as_mut_ptr(), 512) } == 0 {
            break;
        }
    }
    // SAFETY: closes the handle from FindFirstVolumeW exactly once.
    unsafe { FindVolumeClose(find) };
    out
}

pub fn list_disks() -> khodz_core::Result<Vec<PhysicalDisk>> {
    let _quiet = QuietErrors::new();
    let disks: Vec<RawDisk> = (0..MAX_DISK_INDEX).filter_map(probe_disk).collect();
    let volumes = enumerate_volumes();
    let system_root = std::env::var("SystemDrive").ok().map(|d| format!("{}\\", d.trim_end_matches('\\')));
    Ok(assemble(disks, volumes, system_root.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_opened_without_read_or_write_access() {
        assert_eq!(DESIRED_ACCESS, 0);
    }

    #[test]
    fn wide_string_round_trip() {
        let w = wide("C:\\");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w), "C:\\");
    }
}
