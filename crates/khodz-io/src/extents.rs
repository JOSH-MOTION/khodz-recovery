use khodz_core::Result;

use crate::BlockSource;

/// A file assembled from runs of another source: `(Some(offset), len)` reads `len` bytes at
/// `offset`, `(None, len)` is a sparse run that reads as zeros. Lets format validators check a
/// deleted file exactly as recovery would write it. Read-only like every source.
#[derive(Debug)]
pub struct ExtentSource<S> {
    inner: S,
    /// (logical start, physical start or sparse, length)
    runs: Vec<(u64, Option<u64>, u64)>,
    len: u64,
}

impl<S: BlockSource> ExtentSource<S> {
    /// `size` caps the logical length (a file is shorter than its last cluster).
    pub fn new(inner: S, extents: &[(Option<u64>, u64)], size: u64) -> Self {
        let mut runs = Vec::with_capacity(extents.len());
        let mut logical = 0u64;
        for &(phys, len) in extents {
            if logical >= size {
                break;
            }
            let len = len.min(size.saturating_sub(logical));
            runs.push((logical, phys, len));
            logical = logical.saturating_add(len);
        }
        Self { inner, runs, len: logical }
    }
}

impl<S: BlockSource> BlockSource for ExtentSource<S> {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let i = self.runs.partition_point(|&(start, _, _)| start <= offset).saturating_sub(1);
        let Some(&(start, phys, len)) = self.runs.get(i) else { return Ok(0) };
        let within = offset.saturating_sub(start);
        let n = usize::try_from(len.saturating_sub(within)).unwrap_or(usize::MAX).min(buf.len());
        match phys {
            Some(p) => self.inner.read_at(p.saturating_add(within), &mut buf[..n]),
            None => {
                buf[..n].fill(0);
                Ok(n)
            }
        }
    }

    fn describe(&self) -> String {
        format!("file of {} runs on {}", self.runs.len(), self.inner.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemSource;

    #[test]
    fn stitches_runs_and_sparse_gaps() {
        let disk = MemSource::new((0u8..=255).cycle().take(4096).collect(), 512);
        let f = ExtentSource::new(&disk, &[(Some(1000), 4), (None, 3), (Some(10), 5)], 10);
        assert_eq!(f.len(), 10);
        let mut b = [9u8; 16];
        assert_eq!(f.read_exact_at(0, &mut b).unwrap(), 10);
        let first = (1000 % 256) as u8;
        assert_eq!(&b[..10], &[first, first + 1, first + 2, first + 3, 0, 0, 0, 10, 11, 12]);
        let mut tail = [0u8; 4];
        assert_eq!(f.read_exact_at(8, &mut tail).unwrap(), 2);
        assert_eq!(&tail[..2], &[11, 12]);
    }
}
