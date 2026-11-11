//! Raw read-only access to `\\.\PhysicalDriveN`, `\\.\X:` and `\\?\Volume{…}` on Windows.
#![allow(unsafe_code)] // DeviceIoControl FFI only; SAFETY notes below.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::FileExt;
use std::os::windows::io::AsRawHandle;
use std::ptr::null_mut;

use khodz_core::{Error, Result};
use windows_sys::Win32::System::IO::DeviceIoControl;

use crate::{BlockSource, clamp_len, classify_io_error};

/// `CTL_CODE(FILE_DEVICE_DISK, 0x17, METHOD_BUFFERED, FILE_READ_ACCESS)`
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
/// `CTL_CODE(FILE_DEVICE_DISK, 0x28, METHOD_BUFFERED, FILE_ANY_ACCESS)`
const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;

/// A raw disk or volume handle opened with `GENERIC_READ` only. Reads must be sector-aligned;
/// use it through [`crate::AlignedSource`] (see [`super::open_device`]).
#[derive(Debug)]
pub struct RawDevice {
    file: File,
    path: String,
    len: u64,
    sector_size: u32,
}

fn ioctl_out(file: &File, code: u32, out: &mut [u8]) -> std::io::Result<usize> {
    let mut returned = 0u32;
    // SAFETY: the handle is valid for the lifetime of `file`; `out` is valid for its length;
    // no input buffer; synchronous call.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            std::ptr::null(),
            0,
            out.as_mut_ptr().cast(),
            u32::try_from(out.len()).unwrap_or(0),
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(returned as usize) }
}

impl RawDevice {
    pub fn open(path: &str) -> Result<Self> {
        if !super::is_device_path(path) {
            return Err(Error::InvalidData(format!("{path} is not a disk or volume device path")));
        }
        // read(true) only => GENERIC_READ. std uses FILE_SHARE_READ|WRITE|DELETE, so the OS
        // and other programs keep using the drive normally; we never lock or dismount it.
        let file =
            OpenOptions::new().read(true).open(path).map_err(|e| match classify_io_error(e, 0, path) {
                Error::AccessDenied(_) => {
                    Error::AccessDenied(format!("administrator rights are required to read {path} directly"))
                }
                other => other,
            })?;

        let mut len_buf = [0u8; 8];
        let n = ioctl_out(&file, IOCTL_DISK_GET_LENGTH_INFO, &mut len_buf)
            .map_err(|e| classify_io_error(e, 0, path))?;
        if n < 8 {
            return Err(Error::InvalidData(format!("{path}: short length reply")));
        }
        let len = u64::try_from(i64::from_le_bytes(len_buf))
            .map_err(|_| Error::InvalidData(format!("{path}: negative length")))?;

        let mut geo = [0u8; 256];
        let sector_size = ioctl_out(&file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &mut geo)
            .ok()
            .filter(|&n| n >= 24)
            .map(|_| u32::from_le_bytes([geo[20], geo[21], geo[22], geo[23]]))
            .filter(|s| s.is_power_of_two() && (512..=65536).contains(s))
            .unwrap_or(512);

        Ok(Self { file, path: path.to_string(), len, sector_size })
    }
}

impl BlockSource for RawDevice {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = clamp_len(self.len, offset, buf.len());
        if n == 0 {
            return Ok(0);
        }
        self.file.seek_read(&mut buf[..n], offset).map_err(|e| classify_io_error(e, offset, &self.path))
    }

    fn describe(&self) -> String {
        self.path.clone()
    }
}
