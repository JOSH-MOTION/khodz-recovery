use khodz_core::Result;

use crate::{BlockSource, clamp_len};

/// An in-memory source. Used for synthetic test media and for small buffers already read.
#[derive(Debug, Clone)]
pub struct MemSource {
    data: Vec<u8>,
    sector_size: u32,
}

impl MemSource {
    pub fn new(data: Vec<u8>, sector_size: u32) -> Self {
        Self { data, sector_size }
    }
}

impl BlockSource for MemSource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let n = clamp_len(self.len(), offset, buf.len());
        if n == 0 {
            return Ok(0);
        }
        // offset < len <= usize::MAX here.
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        buf[..n].copy_from_slice(&self.data[start..start.saturating_add(n)]);
        Ok(n)
    }

    fn describe(&self) -> String {
        format!("memory ({} bytes)", self.data.len())
    }
}
