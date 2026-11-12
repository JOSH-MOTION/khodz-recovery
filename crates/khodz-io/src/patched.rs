use khodz_core::Result;

use crate::BlockSource;

/// A read-only view of another source in which the first `patch.len()` bytes are replaced by
/// an in-memory copy. Used to open a volume whose primary boot sector is damaged by
/// substituting its backup copy. The underlying medium is never modified.
#[derive(Debug)]
pub struct PatchedSource<S> {
    inner: S,
    patch: Vec<u8>,
}

impl<S: BlockSource> PatchedSource<S> {
    /// The patch is truncated to the source length.
    pub fn new(inner: S, mut patch: Vec<u8>) -> Self {
        patch.truncate(usize::try_from(inner.len()).unwrap_or(usize::MAX));
        Self { inner, patch }
    }
}

impl<S: BlockSource> BlockSource for PatchedSource<S> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let plen = self.patch.len() as u64;
        if offset < plen && !buf.is_empty() {
            // offset < plen ≤ usize::MAX, so these conversions are exact.
            let start = usize::try_from(offset).unwrap_or(0);
            let n = self.patch.len().saturating_sub(start).min(buf.len());
            buf[..n].copy_from_slice(&self.patch[start..start.saturating_add(n)]);
            return Ok(n);
        }
        self.inner.read_at(offset, buf)
    }

    fn describe(&self) -> String {
        format!("{} (boot sector from backup)", self.inner.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemSource;

    #[test]
    fn patch_replaces_only_the_start() {
        let src = MemSource::new((0u8..64).collect::<Vec<_>>(), 16);
        let p = PatchedSource::new(&src, vec![0xAA; 8]);
        let mut buf = [0u8; 16];
        assert_eq!(p.read_exact_at(4, &mut buf).unwrap(), 16);
        assert_eq!(&buf[..4], &[0xAA; 4]);
        assert_eq!(&buf[4..], &(8u8..20).collect::<Vec<_>>()[..]);
        let mut tail = [0u8; 4];
        p.read_exact_at(60, &mut tail).unwrap();
        assert_eq!(tail, [60, 61, 62, 63]);
        assert_eq!(p.len(), 64);
        // The original is untouched.
        let mut orig = [0u8; 4];
        src.read_exact_at(0, &mut orig).unwrap();
        assert_eq!(orig, [0, 1, 2, 3]);
    }
}
