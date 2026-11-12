//! Fault-tolerant sequential reading with cancellation and bounded memory.

use std::ops::Range;

use khodz_core::{CancelToken, Error, Progress, Result};

use crate::BlockSource;

/// Unreadable byte ranges encountered during a read. Adjacent ranges are merged; at most
/// [`BadRanges::MAX_RECORDED`] distinct ranges are kept (the total is always exact).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BadRanges {
    ranges: Vec<Range<u64>>,
    total_bytes: u64,
    truncated: bool,
}

impl BadRanges {
    pub const MAX_RECORDED: usize = 10_000;

    pub fn add(&mut self, r: Range<u64>) {
        if r.is_empty() {
            return;
        }
        self.total_bytes = self.total_bytes.saturating_add(r.end.saturating_sub(r.start));
        if let Some(last) = self.ranges.last_mut().filter(|last| last.end == r.start) {
            last.end = r.end;
            return;
        }
        if self.ranges.len() < Self::MAX_RECORDED {
            self.ranges.push(r);
        } else {
            self.truncated = true;
        }
    }

    pub fn ranges(&self) -> &[Range<u64>] {
        &self.ranges
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn is_empty(&self) -> bool {
        self.total_bytes == 0
    }
}

/// After this many consecutive unreadable sectors we stop treating failures as bad sectors and
/// report the device as lost: a yanked USB drive can surface as I/O errors, and hammering it
/// sector by sector would never finish (and stresses failing hardware).
pub const MAX_CONSECUTIVE_BAD_SECTORS: u32 = 1024;

/// Read `buf.len()` bytes at `offset`, replacing unreadable sectors with zeros and recording them
/// in `bad`. Device loss, access errors and other failures are returned as errors.
/// Returns the number of bytes covered (short only at end of source).
pub fn read_tolerant<S: BlockSource + ?Sized>(
    src: &S,
    offset: u64,
    buf: &mut [u8],
    bad: &mut BadRanges,
) -> Result<usize> {
    match src.read_exact_at(offset, buf) {
        Err(Error::MediaError { .. }) => {}
        other => return other,
    }
    // Slow path: retry sector by sector, once each.
    let want = crate::clamp_len(src.len(), offset, buf.len());
    let ss = u64::from(src.sector_size().max(1));
    let mut done = 0usize;
    let mut consecutive = 0u32;
    while done < want {
        let pos = offset.saturating_add(done as u64);
        let to_boundary = ss.saturating_sub(pos.checked_rem(ss).unwrap_or(0));
        let piece = usize::try_from(to_boundary).unwrap_or(usize::MAX).min(want.saturating_sub(done));
        let slot = &mut buf[done..done.saturating_add(piece)];
        match src.read_exact_at(pos, slot) {
            Ok(n) if n == piece => consecutive = 0,
            Ok(n) => {
                // Short read: end of source.
                done = done.saturating_add(n);
                return Ok(done);
            }
            Err(Error::MediaError { .. }) => {
                slot.fill(0);
                bad.add(pos..pos.saturating_add(piece as u64));
                consecutive = consecutive.saturating_add(1);
                if consecutive >= MAX_CONSECUTIVE_BAD_SECTORS {
                    return Err(Error::DeviceLost(format!(
                        "{}: {consecutive} consecutive unreadable sectors ending at byte {pos}",
                        src.describe()
                    )));
                }
            }
            Err(e) => return Err(e),
        }
        done = done.saturating_add(piece);
    }
    Ok(done)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkReport {
    pub bytes_read: u64,
    pub bad: BadRanges,
}

/// Read `range` of `src` in chunks of `chunk_size` (one reused buffer), calling `f` with each
/// chunk's absolute offset and data. Checks `cancel` between chunks and reports progress after
/// each one. Unreadable sectors are zero-filled and listed in the report.
pub fn for_each_chunk<S: BlockSource + ?Sized>(
    src: &S,
    range: Range<u64>,
    chunk_size: usize,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
    mut f: impl FnMut(u64, &[u8]) -> Result<()>,
) -> Result<ChunkReport> {
    let end = range.end.min(src.len());
    let start = range.start.min(end);
    let total = end.saturating_sub(start);
    let chunk_size = chunk_size.max(src.sector_size() as usize).max(1);
    let mut buf = vec![0u8; chunk_size];
    let mut report = ChunkReport::default();
    let mut pos = start;

    while pos < end {
        cancel.check()?;
        let n = usize::try_from(end.saturating_sub(pos)).unwrap_or(usize::MAX).min(chunk_size);
        let got = read_tolerant(src, pos, &mut buf[..n], &mut report.bad)?;
        if got == 0 {
            break;
        }
        f(pos, &buf[..got])?;
        pos = pos.saturating_add(got as u64);
        report.bytes_read = report.bytes_read.saturating_add(got as u64);
        on_progress(Progress { bytes_done: pos.saturating_sub(start), bytes_total: total, items_found: 0 });
    }
    Ok(report)
}
