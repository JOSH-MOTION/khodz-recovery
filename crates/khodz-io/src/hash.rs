use sha2::{Digest, Sha256};

use khodz_core::{CancelToken, Progress, Result};

use crate::{BlockSource, for_each_chunk};

/// SHA-256 of an entire source, used to fingerprint disk images and to prove a source was not
/// modified. If sectors were unreadable they are hashed as zeros and reported, so a fingerprint
/// is never silently wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub sha256: String,
    pub bytes: u64,
    pub unreadable_bytes: u64,
    pub unreadable_ranges: Vec<(u64, u64)>,
}

pub fn sha256_source<S: BlockSource + ?Sized>(
    src: &S,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<SourceFingerprint> {
    let mut hasher = Sha256::new();
    let report = for_each_chunk(src, 0..src.len(), 1 << 20, cancel, on_progress, |_, data| {
        hasher.update(data);
        Ok(())
    })?;
    let sha256 = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok(SourceFingerprint {
        sha256,
        bytes: report.bytes_read,
        unreadable_bytes: report.bad.total_bytes(),
        unreadable_ranges: report.bad.ranges().iter().map(|r| (r.start, r.end)).collect(),
    })
}
