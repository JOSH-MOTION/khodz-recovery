//! Which physical disk(s) hold a path, and how much free space it has.
//! Disk ids: `PhysicalDriveN` (Windows) or `diskN` (macOS).

use std::path::Path;

use khodz_core::{Error, Result};

/// Nearest existing ancestor (the destination may be about to be created).
fn existing(path: &Path) -> &Path {
    let mut p = path;
    while !p.exists() {
        match p.parent() {
            Some(parent) => p = parent,
            None => break,
        }
    }
    p
}

#[cfg(windows)]
mod imp {
    #![allow(unsafe_code)] // Win32 FFI; SAFETY notes below.

    use std::path::Path;
    use std::ptr::{null, null_mut};

    use khodz_core::{Error, Result};
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW,
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(b: &[u16]) -> String {
        String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())])
    }

    fn last_err(what: &str) -> Error {
        Error::Unsupported(format!("{what}: {}", std::io::Error::last_os_error()))
    }

    pub fn disks(path: &Path) -> Result<Vec<String>> {
        let w = wide(&path.to_string_lossy());
        let mut mount = [0u16; 1024];
        // SAFETY: NUL-terminated input; output buffer length matches.
        if unsafe { GetVolumePathNameW(w.as_ptr(), mount.as_mut_ptr(), 1024) } == 0 {
            return Err(last_err("cannot resolve the volume of the destination"));
        }
        let mut vol = [0u16; 128];
        // SAFETY: `mount` is NUL-terminated (written by the API); output length matches.
        if unsafe { GetVolumeNameForVolumeMountPointW(mount.as_ptr(), vol.as_mut_ptr(), 128) } == 0 {
            return Err(last_err("network or virtual locations cannot be mapped to a disk"));
        }
        let vol = from_wide(&vol);
        let dev = wide(vol.trim_end_matches('\\'));
        // SAFETY: zero desired access — metadata only; optional pointers null.
        let h = unsafe {
            CreateFileW(
                dev.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(last_err("cannot open the destination volume"));
        }
        let mut out = vec![0u8; 8 + 24 * 32];
        let mut returned = 0u32;
        // SAFETY: valid handle and output buffer; synchronous.
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                null(),
                0,
                out.as_mut_ptr().cast(),
                u32::try_from(out.len()).unwrap_or(0),
                &mut returned,
                null_mut(),
            )
        };
        // SAFETY: closes the handle opened above exactly once.
        unsafe { CloseHandle(h) };
        if ok == 0 {
            return Err(last_err("cannot read the destination volume's disk layout"));
        }
        let count = khodz_core::bytes::le_u32(&out, 0).unwrap_or(0).min(32) as usize;
        let mut disks: Vec<String> = (0..count)
            .filter_map(|i| khodz_core::bytes::le_u32(&out, 8usize.saturating_add(i.saturating_mul(24))))
            .map(|n| format!("PhysicalDrive{n}"))
            .collect();
        disks.sort();
        disks.dedup();
        Ok(disks)
    }

    pub fn free(path: &Path) -> Result<u64> {
        let w = wide(&path.to_string_lossy());
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: valid out-pointers, NUL-terminated input.
        if unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
            return Err(last_err("cannot read free space"));
        }
        Ok(avail)
    }

    /// `\\.\PhysicalDriveN` → `PhysicalDriveN`; volume paths are mapped through their extents.
    pub fn device_disks(device: &str) -> Result<Vec<String>> {
        let lower = device.to_ascii_lowercase();
        if let Some(n) = lower.strip_prefix(r"\\.\physicaldrive") {
            return Ok(vec![format!("PhysicalDrive{n}")]);
        }
        if let Some(rest) = device.strip_prefix(r"\\.\") {
            return disks(Path::new(&format!("{rest}\\")));
        }
        disks(Path::new(&format!("{}\\", device.trim_end_matches('\\'))))
    }
}

#[cfg(target_os = "macos")]
mod imp {
    #![allow(unsafe_code)] // statvfs FFI; SAFETY note below.

    use std::path::Path;
    use std::process::Command;

    use khodz_core::{Error, Result};

    fn whole(id: &str) -> Option<String> {
        let rest = id.trim_start_matches("/dev/").trim_start_matches('r').strip_prefix("disk")?;
        let n: String = rest.chars().take_while(char::is_ascii_digit).collect();
        (!n.is_empty()).then(|| format!("disk{n}"))
    }

    pub fn disks(path: &Path) -> Result<Vec<String>> {
        let out = Command::new("/usr/sbin/diskutil").args(["info", "-plist"]).arg(path).output()?;
        if !out.status.success() {
            return Err(Error::Unsupported("diskutil could not identify the destination".into()));
        }
        let v = plist::Value::from_reader(std::io::Cursor::new(out.stdout))
            .map_err(|e| Error::InvalidData(format!("diskutil output: {e}")))?;
        let d = v.as_dictionary().ok_or_else(|| Error::InvalidData("diskutil output".into()))?;
        // APFS volumes live in a synthesized container; the physical store is what matters.
        if let Some(stores) = d.get("APFSPhysicalStores").and_then(plist::Value::as_array) {
            let mut v: Vec<String> = stores
                .iter()
                .filter_map(|s| s.as_dictionary()?.get("APFSPhysicalStore")?.as_string().and_then(whole))
                .collect();
            v.sort();
            v.dedup();
            if !v.is_empty() {
                return Ok(v);
            }
        }
        d.get("ParentWholeDisk")
            .and_then(plist::Value::as_string)
            .and_then(whole)
            .map(|w| vec![w])
            .ok_or_else(|| Error::Unsupported("destination is not on a local disk".into()))
    }

    pub fn free(path: &Path) -> Result<u64> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| Error::InvalidData("path".into()))?;
        // SAFETY: zeroed POD struct filled by statvfs; `c` is a valid C string.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(u64::from(st.f_bavail).saturating_mul(st.f_frsize as u64))
    }

    pub fn device_disks(device: &str) -> Result<Vec<String>> {
        whole(device)
            .map(|w| vec![w])
            .ok_or_else(|| Error::InvalidData(format!("{device} is not a disk path")))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use std::path::Path;

    use khodz_core::{Error, Result};

    pub fn disks(_: &Path) -> Result<Vec<String>> {
        Err(Error::Unsupported("disk mapping on this OS".into()))
    }
    pub fn free(_: &Path) -> Result<u64> {
        Err(Error::Unsupported("free space on this OS".into()))
    }
    pub fn device_disks(_: &str) -> Result<Vec<String>> {
        Err(Error::Unsupported("disk mapping on this OS".into()))
    }
}

/// Physical disks holding `path` (or its nearest existing ancestor).
pub fn disks_for_path(path: &Path) -> Result<Vec<String>> {
    imp::disks(existing(path))
}

/// Bytes available to the current user at `path`.
pub fn free_space(path: &Path) -> Result<u64> {
    imp::free(existing(path))
}

pub(crate) fn disks_for_device(device: &str) -> Result<Vec<String>> {
    imp::device_disks(device)
        .and_then(|d| if d.is_empty() { Err(Error::Unsupported("no disk".into())) } else { Ok(d) })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn maps_real_paths_to_physical_disks() {
        let tmp = std::env::temp_dir();
        let disks = disks_for_path(&tmp).unwrap();
        assert!(!disks.is_empty() && disks.iter().all(|d| d.starts_with("PhysicalDrive")), "{disks:?}");
        // A not-yet-existing subfolder resolves through its existing ancestor.
        assert_eq!(disks_for_path(&tmp.join("does/not/exist/yet")).unwrap(), disks);
        assert!(free_space(&tmp).unwrap() > 0);
        assert_eq!(disks_for_device(r"\\.\PhysicalDrive7").unwrap(), vec!["PhysicalDrive7"]);
        // The system drive letter maps to the same disk as the temp folder (both on C:).
        let sys = std::env::var("SystemDrive").unwrap();
        assert_eq!(disks_for_device(&format!(r"\\.\{sys}")).unwrap(), disks);
    }
}
