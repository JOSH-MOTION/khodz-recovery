use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use khodz_core::{Error, Result};

use crate::{BlockSource, clamp_len, classify_io_error};

/// A raw disk image file (`.img`, `.dd`, `.bin`, …) opened strictly read-only.
#[derive(Debug)]
pub struct ImageFile {
    file: File,
    path: PathBuf,
    len: u64,
    sector_size: u32,
}

impl ImageFile {
    /// Open an image for reading. Only `read(true)` is requested — the OS will refuse any write
    /// through this handle, and read-only files (or files on read-only media) open fine.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = OpenOptions::new().read(true).open(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::InvalidData(format!("{} does not exist", path.display())),
            _ => classify_io_error(e, 0, &path.display().to_string()),
        })?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(Error::InvalidData(format!("{} is not a regular file", path.display())));
        }
        Ok(Self { file, path: path.to_path_buf(), len: meta.len(), sector_size: 512 })
    }

    /// Images carry no sector size of their own; 512 is assumed until a partition table or
    /// filesystem says otherwise (Stage 4).
    pub fn with_sector_size(mut self, sector_size: u32) -> Result<Self> {
        if !matches!(sector_size, 512 | 1024 | 2048 | 4096) {
            return Err(Error::InvalidData(format!("unsupported sector size {sector_size}")));
        }
        self.sector_size = sector_size;
        Ok(self)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(windows)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

#[cfg(unix)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

impl BlockSource for ImageFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let want = clamp_len(self.len, offset, buf.len());
        if want == 0 {
            return Ok(0);
        }
        pread(&self.file, &mut buf[..want], offset)
            .map_err(|e| classify_io_error(e, offset, &self.describe()))
    }

    fn describe(&self) -> String {
        format!("image {}", self.path.display())
    }
}
