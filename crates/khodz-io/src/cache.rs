use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use khodz_core::{Error, Result};

use crate::BlockSource;

/// Bounded LRU cache of fixed-size chunks in front of a slow source. Filesystem parsers re-read
/// the same metadata (MFT records, FAT sectors, B-tree nodes) many times; this keeps those hits in
/// memory while the total cached bytes never exceed `chunk_size * max_chunks`.
pub struct CachedSource<S> {
    inner: S,
    chunk_size: usize,
    max_chunks: usize,
    state: Mutex<State>,
    hits: AtomicU64,
    misses: AtomicU64,
}

#[derive(Default)]
struct State {
    chunks: HashMap<u64, (Arc<Vec<u8>>, u64)>,
    tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub cached_chunks: usize,
}

impl<S: BlockSource> CachedSource<S> {
    /// `chunk_size` must be a non-zero multiple of the sector size; `max_chunks >= 1`.
    pub fn new(inner: S, chunk_size: usize, max_chunks: usize) -> Result<Self> {
        let ss = inner.sector_size() as usize;
        if chunk_size == 0 || chunk_size.checked_rem(ss) != Some(0) || max_chunks == 0 {
            return Err(Error::InvalidData("invalid cache geometry".into()));
        }
        Ok(Self {
            inner,
            chunk_size,
            max_chunks,
            state: Mutex::new(State::default()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    /// Default: 256 KiB chunks, 64 MiB maximum.
    pub fn with_defaults(inner: S) -> Result<Self> {
        Self::new(inner, 256 << 10, 256)
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            cached_chunks: self.state.lock().map(|s| s.chunks.len()).unwrap_or(0),
        }
    }

    fn chunk(&self, index: u64) -> Result<Arc<Vec<u8>>> {
        {
            let mut st = self.state.lock().map_err(|_| Error::InvalidData("cache lock poisoned".into()))?;
            st.tick = st.tick.wrapping_add(1);
            let tick = st.tick;
            if let Some(entry) = st.chunks.get_mut(&index) {
                entry.1 = tick;
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(Arc::clone(&entry.0));
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        // Read outside the lock so other threads are not blocked on slow media.
        let start = index.saturating_mul(self.chunk_size as u64);
        let mut data = vec![0u8; self.chunk_size];
        let n = self.inner.read_exact_at(start, &mut data)?;
        data.truncate(n);
        let data = Arc::new(data);

        let mut st = self.state.lock().map_err(|_| Error::InvalidData("cache lock poisoned".into()))?;
        let tick = st.tick;
        st.chunks.insert(index, (Arc::clone(&data), tick));
        while st.chunks.len() > self.max_chunks {
            let oldest = st.chunks.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| *k);
            match oldest {
                Some(k) => st.chunks.remove(&k),
                None => break,
            };
        }
        Ok(data)
    }
}

impl<S: BlockSource> BlockSource for CachedSource<S> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let want = crate::clamp_len(self.len(), offset, buf.len());
        let cs = self.chunk_size as u64;
        let mut done = 0usize;
        while done < want {
            let pos = offset.saturating_add(done as u64);
            // cs > 0 is guaranteed by `new`.
            let chunk = self.chunk(pos.checked_div(cs).unwrap_or(0))?;
            let within = usize::try_from(pos.checked_rem(cs).unwrap_or(0)).unwrap_or(0);
            if within >= chunk.len() {
                break;
            }
            let n = want.saturating_sub(done).min(chunk.len().saturating_sub(within));
            let dst_end = done.saturating_add(n);
            buf[done..dst_end].copy_from_slice(&chunk[within..within.saturating_add(n)]);
            done = dst_end;
        }
        Ok(done)
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}
