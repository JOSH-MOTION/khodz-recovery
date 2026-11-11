use khodz_core::{Error, Result, align_down, align_up};

use crate::BlockSource;

/// Adapter for sources that only accept sector-aligned offsets and lengths (raw disks on Windows
/// and `/dev/rdisk*` on macOS). Unaligned requests go through a bounce buffer whose size is
/// capped at `max_io` plus two sectors, so memory stays bounded for any request size.
#[derive(Debug)]
pub struct AlignedSource<S> {
    inner: S,
    max_io: usize,
}

impl<S: BlockSource> AlignedSource<S> {
    pub const DEFAULT_MAX_IO: usize = 4 << 20;

    pub fn new(inner: S) -> Self {
        Self { inner, max_io: Self::DEFAULT_MAX_IO }
    }

    pub fn inner(&self) -> &S {
        &self.inner
    }
}

impl<S: BlockSource> BlockSource for AlignedSource<S> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let len = self.inner.len();
        let want = crate::clamp_len(len, offset, buf.len().min(self.max_io));
        if want == 0 {
            return Ok(0);
        }
        let ss = u64::from(self.inner.sector_size().max(1));
        let end = offset.saturating_add(want as u64);
        let a_start = align_down(offset, ss).ok_or_else(|| Error::InvalidData("zero sector size".into()))?;
        let a_end = align_up(end, ss).unwrap_or(end).min(align_up(len, ss).unwrap_or(len));

        if a_start == offset && a_end == end {
            return self.inner.read_exact_at(offset, &mut buf[..want]);
        }

        let span = usize::try_from(a_end.saturating_sub(a_start))
            .map_err(|_| Error::InvalidData("aligned span too large".into()))?;
        let mut bounce = vec![0u8; span];
        let got = self.inner.read_exact_at(a_start, &mut bounce)?;
        let skip = usize::try_from(offset.saturating_sub(a_start)).unwrap_or(usize::MAX);
        if got <= skip {
            return Ok(0);
        }
        let n = want.min(got.saturating_sub(skip));
        buf[..n].copy_from_slice(&bounce[skip..skip.saturating_add(n)]);
        Ok(n)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}
