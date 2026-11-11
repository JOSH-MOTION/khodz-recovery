//! A forward-reading, buffered cursor over a block source with a hard length limit, so
//! validators can walk structures of any size in bounded memory.

use khodz_core::Result;
use khodz_io::BlockSource;

pub struct Stream<'a, S: ?Sized> {
    src: &'a S,
    base: u64,
    /// Bytes consumed from `base`.
    pub pos: u64,
    limit: u64,
    buf: Vec<u8>,
    buf_start: u64,
}

const BUF: usize = 64 << 10;

impl<'a, S: BlockSource + ?Sized> Stream<'a, S> {
    pub fn new(src: &'a S, base: u64, limit: u64) -> Self {
        let limit = limit.min(src.len().saturating_sub(base));
        Self { src, base, pos: 0, limit, buf: Vec::new(), buf_start: 0 }
    }

    /// Bytes available from `base` (capped by the format's maximum size).
    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.pos)
    }

    fn fill(&mut self) -> Result<bool> {
        if self.pos >= self.limit {
            return Ok(false);
        }
        let in_buf =
            self.pos >= self.buf_start && self.pos < self.buf_start.saturating_add(self.buf.len() as u64);
        if in_buf {
            return Ok(true);
        }
        let n = usize::try_from(self.remaining()).unwrap_or(BUF).min(BUF);
        self.buf.resize(n, 0);
        let got = self.src.read_exact_at(self.base.saturating_add(self.pos), &mut self.buf)?;
        self.buf.truncate(got);
        self.buf_start = self.pos;
        Ok(got > 0)
    }

    /// Next byte, or None at the limit / end of source.
    pub fn byte(&mut self) -> Result<Option<u8>> {
        if !self.fill()? {
            return Ok(None);
        }
        let idx = usize::try_from(self.pos.saturating_sub(self.buf_start)).unwrap_or(usize::MAX);
        let b = self.buf.get(idx).copied();
        if b.is_some() {
            self.pos = self.pos.saturating_add(1);
        }
        Ok(b)
    }

    /// Exactly `n` bytes (small reads only), or None if they are not all available.
    pub fn take(&mut self, n: usize) -> Result<Option<Vec<u8>>> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            match self.byte()? {
                Some(b) => out.push(b),
                None => return Ok(None),
            }
        }
        Ok(Some(out))
    }

    pub fn u16_be(&mut self) -> Result<Option<u16>> {
        Ok(self.take(2)?.map(|b| u16::from_be_bytes([b[0], b[1]])))
    }

    pub fn u32_be(&mut self) -> Result<Option<u32>> {
        Ok(self.take(4)?.map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
    }

    /// Skip `n` bytes; false if that would pass the limit.
    pub fn skip(&mut self, n: u64) -> bool {
        match self.pos.checked_add(n) {
            Some(p) if p <= self.limit => {
                self.pos = p;
                true
            }
            _ => false,
        }
    }

    /// Feed the next `n` bytes to `f` in buffered pieces (used for CRCs). False if short.
    pub fn consume(&mut self, mut n: u64, mut f: impl FnMut(&[u8])) -> Result<bool> {
        while n > 0 {
            if !self.fill()? {
                return Ok(false);
            }
            let idx = usize::try_from(self.pos.saturating_sub(self.buf_start)).unwrap_or(usize::MAX);
            let avail = self.buf.len().saturating_sub(idx);
            let take = usize::try_from(n).unwrap_or(usize::MAX).min(avail);
            if take == 0 {
                return Ok(false);
            }
            f(&self.buf[idx..idx.saturating_add(take)]);
            self.pos = self.pos.saturating_add(take as u64);
            n = n.saturating_sub(take as u64);
        }
        Ok(true)
    }
}
