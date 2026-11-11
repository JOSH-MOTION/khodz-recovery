use khodz_core::{Error, Result};

use crate::{BlockSource, clamp_len};

/// A window `[start, start + len)` into another source — e.g. one partition of a disk.
/// Reads can never escape the window.
#[derive(Debug)]
pub struct SliceSource<S> {
    inner: S,
    start: u64,
    len: u64,
    label: String,
}

impl<S: BlockSource> SliceSource<S> {
    pub fn new(inner: S, start: u64, len: u64, label: impl Into<String>) -> Result<Self> {
        let end = start.checked_add(len).ok_or_else(|| Error::InvalidData("slice end overflows".into()))?;
        if end > inner.len() {
            return Err(Error::InvalidData(format!(
                "slice {start}..{end} exceeds source length {}",
                inner.len()
            )));
        }
        Ok(Self { inner, start, len, label: label.into() })
    }

    pub fn start(&self) -> u64 {
        self.start
    }
}

impl<S: BlockSource> BlockSource for SliceSource<S> {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = clamp_len(self.len, offset, buf.len());
        if n == 0 {
            return Ok(0);
        }
        // offset < len and start + len <= inner.len(), so this cannot overflow.
        let abs = self.start.saturating_add(offset);
        self.inner.read_at(abs, &mut buf[..n]).map_err(|e| match e {
            // Report offsets relative to the slice the caller is using.
            Error::MediaError { offset: _, detail } => Error::MediaError { offset, detail },
            other => other,
        })
    }

    fn describe(&self) -> String {
        format!("{} of {}", self.label, self.inner.describe())
    }
}
