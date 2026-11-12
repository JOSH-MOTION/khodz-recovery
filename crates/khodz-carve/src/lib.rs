//! Signature carving: find files in raw data by their headers, then *validate their structure*
//! to decide how much of each file is really there.
//!
//! Honesty rules (see TESTING.md, "fragmentation honesty"):
//! * [`CarveStatus::Complete`] is only reported when the format's structure was verified end to
//!   end: PNG (every chunk CRC), ZIP/DOCX/XLSX/PPTX (central directory ↔ local headers ↔ end
//!   record), GIF (block chain), JPEG (marker chain, clean entropy data, no zeroed runs), PDF
//!   (`startxref` points at a real cross-reference).
//! * Formats whose payload cannot be checked are [`CarveStatus::Unverified`]: BMP, WAV, AVI,
//!   WebP, and MP4/MOV/HEIC (box chain verified, media data opaque).
//! * Anything that breaks part-way is [`CarveStatus::Partial`], with the length that validated.
//!
//! Carving assumes a file is stored contiguously from its header, so a fragmented file breaks
//! validation and is reported partial. Known limit: a JPEG fragment gap filled with *another
//! JPEG's* image data is structurally indistinguishable without decoding.

#![cfg_attr(test, allow(clippy::arithmetic_side_effects))]

mod formats;
pub mod media;
mod stream;

use serde::Serialize;

use khodz_core::{CancelToken, Progress, Result};
use khodz_io::{BadRanges, BlockSource, read_tolerant};

pub use formats::validate_at;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Pdf,
    Zip,
    Docx,
    Xlsx,
    Pptx,
    Mp4,
    Mov,
    Heic,
    Wav,
    Avi,
    Webp,
}

impl FileKind {
    pub fn extension(self) -> &'static str {
        match self {
            FileKind::Jpeg => "jpg",
            FileKind::Png => "png",
            FileKind::Gif => "gif",
            FileKind::Bmp => "bmp",
            FileKind::Pdf => "pdf",
            FileKind::Zip => "zip",
            FileKind::Docx => "docx",
            FileKind::Xlsx => "xlsx",
            FileKind::Pptx => "pptx",
            FileKind::Mp4 => "mp4",
            FileKind::Mov => "mov",
            FileKind::Heic => "heic",
            FileKind::Wav => "wav",
            FileKind::Avi => "avi",
            FileKind::Webp => "webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CarveStatus {
    /// Structure verified end to end.
    Complete,
    /// Header and declared length are plausible; the payload cannot be verified.
    Unverified,
    /// Structure broke part-way (truncated, overwritten or fragmented).
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Carved {
    pub offset: u64,
    pub len: u64,
    pub kind: FileKind,
    pub status: CarveStatus,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CarveStats {
    pub bytes_scanned: u64,
    pub headers_seen: u64,
    pub found: u64,
    pub rejected: u64,
    pub unreadable_bytes: u64,
}

const CHUNK: usize = 1 << 20;

/// Scan `range` of `src`, testing every `step`-aligned offset (normally the sector size) for a
/// known header. `skip` lets the caller suppress offsets already explained by filesystem
/// metadata. Complete files are skipped over so their internals are not re-carved.
pub fn carve<S: BlockSource + ?Sized>(
    src: &S,
    range: std::ops::Range<u64>,
    step: u64,
    cancel: &CancelToken,
    skip: &dyn Fn(u64) -> bool,
    mut on_progress: impl FnMut(Progress),
    mut on_found: impl FnMut(Carved) -> Result<()>,
) -> Result<CarveStats> {
    let step = step.max(512);
    let end = range.end.min(src.len());
    let start = range.start.min(end);
    let total = end.saturating_sub(start);
    let mut stats = CarveStats::default();
    let mut bad = BadRanges::default();
    let mut buf = vec![0u8; CHUNK];
    let mut pos = start;

    while pos < end {
        cancel.check()?;
        let want = usize::try_from(end.saturating_sub(pos)).unwrap_or(CHUNK).min(CHUNK);
        let got = read_tolerant(src, pos, &mut buf[..want], &mut bad)?;
        if got == 0 {
            break;
        }
        let mut next = pos.saturating_add(got as u64);
        let mut i = 0usize;
        while i < got {
            let at = pos.saturating_add(i as u64);
            let head = &buf[i..got.min(i.saturating_add(32))];
            if formats::looks_like_header(head) && !skip(at) {
                stats.headers_seen = stats.headers_seen.saturating_add(1);
                match validate_at(src, at)? {
                    Some(c) => {
                        let jump_over = c.status == CarveStatus::Complete;
                        let file_end = at.saturating_add(c.len);
                        stats.found = stats.found.saturating_add(1);
                        on_found(c)?;
                        if jump_over && file_end > at {
                            let aligned = file_end.div_ceil(step).saturating_mul(step);
                            if aligned >= pos.saturating_add(got as u64) {
                                next = aligned;
                                break;
                            }
                            i = usize::try_from(aligned.saturating_sub(pos)).unwrap_or(got);
                            continue;
                        }
                    }
                    None => stats.rejected = stats.rejected.saturating_add(1),
                }
            }
            i = i.saturating_add(usize::try_from(step).unwrap_or(512));
        }
        stats.bytes_scanned = stats.bytes_scanned.saturating_add(got as u64);
        pos = next;
        on_progress(Progress {
            bytes_done: pos.min(end).saturating_sub(start),
            bytes_total: total,
            items_found: stats.found,
        });
    }
    stats.unreadable_bytes = bad.total_bytes();
    Ok(stats)
}
