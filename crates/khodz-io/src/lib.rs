//! Read-only block access for Khodz Recovery.
//!
//! **Data-safety invariant:** nothing in this crate can write to a source. [`BlockSource`] has
//! no write method, image files are opened with `read(true)` only, and raw devices are opened
//! read-only at the OS level. Tests hash every source before and after use.
//!
//! Memory is bounded: adapters use fixed-size bounce buffers and caches with hard caps, and
//! [`for_each_chunk`] reuses a single buffer for an entire pass over a source.

#![cfg_attr(test, allow(clippy::arithmetic_side_effects))]

mod aligned;
mod cache;
mod chunked;
mod classify;
pub mod device;
mod extents;
mod hash;
mod image;
mod mem;
mod patched;
mod slice;

use khodz_core::{Error, Result};

pub use aligned::AlignedSource;
pub use cache::{CacheStats, CachedSource};
pub use chunked::{BadRanges, ChunkReport, MAX_CONSECUTIVE_BAD_SECTORS, for_each_chunk, read_tolerant};
pub use classify::classify_io_error;
pub use extents::ExtentSource;
pub use hash::{SourceFingerprint, sha256_source};
pub use image::ImageFile;
pub use mem::MemSource;
pub use patched::PatchedSource;
pub use slice::SliceSource;

/// A read-only, randomly addressable byte source (disk, volume, partition or image).
///
/// Implementations must be safe to share across threads and must never modify the underlying
/// medium.
pub trait BlockSource: Send + Sync {
    /// Total size in bytes.
    fn len(&self) -> u64;

    /// Logical sector size in bytes (512 or 4096 for most media).
    fn sector_size(&self) -> u32;

    /// Read up to `buf.len()` bytes at `offset`. Returns the number of bytes read; `0` means
    /// `offset` is at or past the end. Arbitrary (unaligned) offsets and lengths are allowed.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize>;

    /// Human-readable description for logs and error messages.
    fn describe(&self) -> String;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read until `buf` is full or the end of the source is reached. Returns bytes read.
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset
                .checked_add(done as u64)
                .ok_or_else(|| Error::InvalidData("read offset overflow".into()))?;
            let n = self.read_at(pos, &mut buf[done..])?;
            if n == 0 {
                break;
            }
            done = done.saturating_add(n);
        }
        Ok(done)
    }
}

macro_rules! delegate_block_source {
    ($($ptr:ty),*) => {$(
        impl<T: BlockSource + ?Sized> BlockSource for $ptr {
            fn len(&self) -> u64 { (**self).len() }
            fn sector_size(&self) -> u32 { (**self).sector_size() }
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> { (**self).read_at(offset, buf) }
            fn describe(&self) -> String { (**self).describe() }
        }
    )*};
}

delegate_block_source!(std::sync::Arc<T>, Box<T>, &T);

/// Clamp a requested read to the bytes remaining in a source of length `len`.
pub(crate) fn clamp_len(len: u64, offset: u64, want: usize) -> usize {
    if offset >= len {
        return 0;
    }
    let remaining = len.saturating_sub(offset);
    usize::try_from(remaining).map_or(want, |r| r.min(want))
}
