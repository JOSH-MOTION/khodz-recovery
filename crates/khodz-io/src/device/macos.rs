//! Raw read-only access to `/dev/rdiskN[sM]` on macOS.
#![allow(unsafe_code)] // ioctl FFI only; SAFETY notes below.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;

use khodz_core::{Error, Result};

use crate::{BlockSource, clamp_len, classify_io_error};

/// `_IOR('d', 24, uint32_t)`
const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418;
/// `_IOR('d', 25, uint64_t)`
const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419;

/// A raw disk opened `O_RDONLY`. Character devices (`rdisk`) need sector-aligned I/O; use it
/// through [`crate::AlignedSource`] (see [`super::open_device`]).
#[derive(Debug)]
pub struct RawDevice {
    file: File,
    path: String,
    len: u64,
    sector_size: u32,
}

impl RawDevice {
    pub fn open(path: &str) -> Result<Self> {
        if !super::is_device_path(path) {
            return Err(Error::InvalidData(format!("{path} is not a disk device path")));
        }
        let file =
            OpenOptions::new().read(true).open(path).map_err(|e| match classify_io_error(e, 0, path) {
                Error::AccessDenied(_) => Error::AccessDenied(format!(
                    "reading {path} requires administrator rights and Full Disk Access"
                )),
                other => other,
            })?;
        let fd = file.as_raw_fd();
        let mut block_size: u32 = 0;
        let mut block_count: u64 = 0;
        // SAFETY: fd is valid for the lifetime of `file`; out-pointers have the sizes these
        // ioctls write (uint32_t / uint64_t).
        let ok = unsafe {
            libc::ioctl(fd, DKIOCGETBLOCKSIZE, &mut block_size as *mut u32) == 0
                && libc::ioctl(fd, DKIOCGETBLOCKCOUNT, &mut block_count as *mut u64) == 0
        };
        if !ok {
            return Err(classify_io_error(std::io::Error::last_os_error(), 0, path));
        }
        if !block_size.is_power_of_two() || block_size < 512 {
            return Err(Error::InvalidData(format!("{path}: implausible block size {block_size}")));
        }
        let len = block_count
            .checked_mul(u64::from(block_size))
            .ok_or_else(|| Error::InvalidData(format!("{path}: size overflow")))?;
        Ok(Self { file, path: path.to_string(), len, sector_size: block_size })
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
        self.file.read_at(&mut buf[..n], offset).map_err(|e| classify_io_error(e, offset, &self.path))
    }

    fn describe(&self) -> String {
        self.path.clone()
    }
}
