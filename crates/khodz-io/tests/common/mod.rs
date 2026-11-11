//! Test-only block sources that simulate hardware behaviour.
#![allow(dead_code, clippy::arithmetic_side_effects)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use khodz_core::{Error, Result};
use khodz_io::{BlockSource, MemSource};

/// Deterministic pseudo-random bytes (no RNG dependency).
pub fn pattern(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Simple LCG for reproducible "random" offsets.
pub struct Lcg(pub u64);
impl Lcg {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.0 >> 11
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

/// Rejects any read whose offset or length is not sector-aligned, like a raw disk handle.
pub struct StrictAligned(pub MemSource);
impl BlockSource for StrictAligned {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn sector_size(&self) -> u32 {
        self.0.sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let ss = u64::from(self.sector_size());
        assert_eq!(offset % ss, 0, "unaligned offset {offset}");
        assert_eq!(buf.len() as u64 % ss, 0, "unaligned length {}", buf.len());
        self.0.read_at(offset, buf)
    }
    fn describe(&self) -> String {
        "strict".into()
    }
}

/// Counts reads that reach the underlying source.
pub struct Counting(pub MemSource, pub AtomicU64);
impl Counting {
    pub fn new(m: MemSource) -> Self {
        Self(m, AtomicU64::new(0))
    }
    pub fn reads(&self) -> u64 {
        self.1.load(Ordering::Relaxed)
    }
}
impl BlockSource for Counting {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn sector_size(&self) -> u32 {
        self.0.sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.1.fetch_add(1, Ordering::Relaxed);
        self.0.read_at(offset, buf)
    }
    fn describe(&self) -> String {
        "counting".into()
    }
}

/// Fails reads touching listed bad sectors (media error) or reaching `lost_at` (device gone).
pub struct Faulty {
    pub mem: MemSource,
    pub bad_sectors: HashSet<u64>,
    pub lost_at: Option<u64>,
}
impl BlockSource for Faulty {
    fn len(&self) -> u64 {
        self.mem.len()
    }
    fn sector_size(&self) -> u32 {
        self.mem.sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        let ss = u64::from(self.sector_size());
        let end = offset + buf.len() as u64;
        if let Some(lost) = self.lost_at
            && end > lost
        {
            return Err(Error::DeviceLost("test device unplugged".into()));
        }
        let first = offset / ss;
        let last = end.saturating_sub(1) / ss;
        if let Some(bad) = (first..=last).find(|s| self.bad_sectors.contains(s)) {
            return Err(Error::MediaError { offset: bad * ss, detail: "CRC error".into() });
        }
        self.mem.read_at(offset, buf)
    }
    fn describe(&self) -> String {
        "faulty".into()
    }
}
