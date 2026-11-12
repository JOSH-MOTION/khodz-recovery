//! Deep verification of MP4 / MOV (ISO base media) files: read the movie index (`moov`) and
//! check that the media data it points to is really there.
//!
//! A valid box chain says nothing about the payload: a deleted video whose clusters now hold
//! another file's bytes (or zeros) still has a perfect `ftyp`/`mdat`/`moov` chain if the index
//! survived. So for H.264 / H.265 video tracks, samples spread across the whole file are
//! parsed as length-prefixed NAL units, which must exactly fill each sample's recorded size
//! with plausible NAL headers. Random or foreign data fails this within a few bytes.
//!
//! Every value read from the file is untrusted: sizes and counts are capped and all
//! arithmetic is checked.

use khodz_core::Result;
use khodz_core::bytes::{be_u32, be_u64};
use khodz_io::BlockSource;

/// Largest movie index read into memory.
const MAX_MOOV: u64 = 64 << 20;
/// Video samples checked per track, spread evenly across the file.
const SAMPLES_CHECKED: usize = 256;
/// NAL units followed within one sample before giving up on it (treated as valid).
const MAX_NALS_PER_SAMPLE: usize = 512;
/// Upper bounds on table sizes (entries).
const MAX_ENTRIES: u32 = 8_000_000;

#[derive(Debug, Clone, PartialEq)]
pub enum MediaVerdict {
    /// The index is consistent and every checked video sample parses correctly.
    Verified { samples_checked: u32 },
    /// Some media data is not what the index says. `from` is how far into the presentation
    /// (0.0–1.0, by chunk order) the first bad sample sits — roughly where playback breaks.
    Damaged { from: f64, bad: u32, checked: u32, reason: String },
    /// Nothing could be concluded (no video track, unsupported codec, no index).
    Unknown(String),
}

#[derive(Debug, Default)]
struct Track {
    video: bool,
    /// NAL length prefix size (1, 2 or 4) and codec, for H.264 / H.265.
    nal: Option<(u8, Codec)>,
    chunk_offsets: Vec<u64>,
    sample_sizes: Vec<u32>,
    /// (first_chunk (1-based), samples_per_chunk)
    stsc: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Codec {
    Avc,
    Hevc,
}

/// Iterate the child boxes of `buf` (header size, type, body range).
fn boxes(buf: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        let size32 = be_u32(buf, pos)?;
        let t = buf.get(pos.checked_add(4)?..pos.checked_add(8)?)?;
        let typ = [t[0], t[1], t[2], t[3]];
        let (size, header) = match size32 {
            0 => (buf.len().checked_sub(pos)?, 8usize),
            1 => (usize::try_from(be_u64(buf, pos.checked_add(8)?)?).ok()?, 16usize),
            n => (usize::try_from(n).ok()?, 8usize),
        };
        if size < header {
            return None;
        }
        let end = pos.checked_add(size)?;
        let body = buf.get(pos.checked_add(header)?..end)?;
        pos = end;
        Some((typ, body))
    })
}

fn find<'a>(buf: &'a [u8], typ: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(buf).find(|(t, _)| t == typ).map(|(_, b)| b)
}

fn parse_track(trak: &[u8]) -> Option<Track> {
    let mdia = find(trak, b"mdia")?;
    let hdlr = find(mdia, b"hdlr")?;
    let mut t = Track { video: hdlr.get(8..12) == Some(b"vide"), ..Track::default() };
    let stbl = find(find(mdia, b"minf")?, b"stbl")?;

    // Sample description: first entry's codec and NAL length size.
    if let Some(stsd) = find(stbl, b"stsd") {
        if let Some((typ, entry)) = stsd.get(8..).and_then(|b| boxes(b).next()) {
            let config = match &typ {
                b"avc1" | b"avc3" => Some((b"avcC", Codec::Avc, 4usize)),
                b"hvc1" | b"hev1" => Some((b"hvcC", Codec::Hevc, 21usize)),
                _ => None,
            };
            // VisualSampleEntry fields take 78 bytes before the child boxes.
            if let (Some((cfg_type, codec, at)), Some(children)) = (config, entry.get(78..)) {
                if let Some(cfg) = find(children, cfg_type) {
                    let len = cfg.get(at).map(|b| (b & 0x03).saturating_add(1));
                    if let Some(l @ (1 | 2 | 4)) = len {
                        t.nal = Some((l, codec));
                    }
                }
            }
        }
    }

    if let Some(stco) = find(stbl, b"stco") {
        let n = be_u32(stco, 4)?.min(MAX_ENTRIES);
        t.chunk_offsets = (0..n)
            .map_while(|i| be_u32(stco, 8usize.checked_add((i as usize).checked_mul(4)?)?))
            .map(u64::from)
            .collect();
    } else if let Some(co64) = find(stbl, b"co64") {
        let n = be_u32(co64, 4)?.min(MAX_ENTRIES);
        t.chunk_offsets =
            (0..n).map_while(|i| be_u64(co64, 8usize.checked_add((i as usize).checked_mul(8)?)?)).collect();
    }
    if let Some(stsz) = find(stbl, b"stsz") {
        let fixed = be_u32(stsz, 4)?;
        let n = be_u32(stsz, 8)?.min(MAX_ENTRIES);
        t.sample_sizes = if fixed != 0 {
            vec![fixed; n as usize]
        } else {
            (0..n).map_while(|i| be_u32(stsz, 12usize.checked_add((i as usize).checked_mul(4)?)?)).collect()
        };
    }
    if let Some(stsc) = find(stbl, b"stsc") {
        let n = be_u32(stsc, 4)?.min(MAX_ENTRIES);
        t.stsc = (0..n)
            .map_while(|i| {
                let o = 8usize.checked_add((i as usize).checked_mul(12)?)?;
                Some((be_u32(stsc, o)?, be_u32(stsc, o.checked_add(4)?)?))
            })
            .collect();
    }
    Some(t)
}

/// For every chunk: (file offset, first sample index, sample count).
fn chunk_layout(t: &Track) -> Vec<(u64, usize, u32)> {
    let mut out = Vec::with_capacity(t.chunk_offsets.len());
    let mut sample = 0usize;
    for (i, &off) in t.chunk_offsets.iter().enumerate() {
        let chunk_no = u32::try_from(i).unwrap_or(u32::MAX).saturating_add(1);
        let per = t.stsc.iter().rev().find(|(first, _)| *first <= chunk_no).map_or(1, |(_, n)| *n);
        out.push((off, sample, per));
        sample = sample.saturating_add(per as usize);
    }
    out
}

fn nal_header_ok(codec: Codec, b0: u8, b1: u8) -> bool {
    if b0 & 0x80 != 0 {
        return false;
    }
    match codec {
        // 1..=23 are defined NAL unit types; 0 and 24..31 never appear in MP4 samples.
        Codec::Avc => (1..=23).contains(&(b0 & 0x1F)),
        // 0..=40 defined; temporal id (low 3 bits of the second byte) must be at least 1.
        Codec::Hevc => ((b0 >> 1) & 0x3F) <= 40 && (b1 & 0x07) != 0,
    }
}

/// Whether one sample is a sequence of length-prefixed NAL units filling it exactly.
fn sample_ok<S: BlockSource + ?Sized>(
    src: &S,
    at: u64,
    size: u32,
    nal_len: u8,
    codec: Codec,
) -> Result<bool> {
    let size = u64::from(size);
    let l = u64::from(nal_len);
    let mut pos = 0u64;
    let mut hdr = [0u8; 6];
    for _ in 0..MAX_NALS_PER_SAMPLE {
        if pos == size {
            return Ok(true);
        }
        let need = l.saturating_add(2);
        if pos.saturating_add(need) > size {
            return Ok(false);
        }
        let n = usize::try_from(need).unwrap_or(6).min(6);
        let Some(abs) = at.checked_add(pos) else { return Ok(false) };
        if src.read_exact_at(abs, &mut hdr[..n])? < n {
            return Ok(false);
        }
        let len = hdr[..usize::from(nal_len)].iter().fold(0u64, |a, &b| (a << 8) | u64::from(b));
        let k = usize::from(nal_len);
        if len < 2 || !nal_header_ok(codec, hdr[k], hdr[k.saturating_add(1)]) {
            return Ok(false);
        }
        pos = pos.saturating_add(l).saturating_add(len);
        if pos > size {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Verify an MP4/MOV file stored at `at..at+len` of `src`.
pub fn verify_isobmff<S: BlockSource + ?Sized>(src: &S, at: u64, len: u64) -> Result<MediaVerdict> {
    // Top-level boxes: locate the index and the media data.
    let mut pos = 0u64;
    let mut moov: Option<(u64, u64)> = None;
    let mut mdats: Vec<(u64, u64)> = Vec::new();
    let mut hdr = [0u8; 16];
    while pos.saturating_add(8) <= len {
        let Some(abs) = at.checked_add(pos) else { break };
        if src.read_exact_at(abs, &mut hdr)? < 8 {
            break;
        }
        let size32 = u64::from(u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]));
        let (size, header) = match size32 {
            0 => (len.saturating_sub(pos), 8),
            1 => (
                u64::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11], hdr[12], hdr[13], hdr[14], hdr[15]]),
                16,
            ),
            n => (n, 8),
        };
        if size < header {
            break;
        }
        match &hdr[4..8] {
            b"moov" => moov = Some((pos.saturating_add(header), size.saturating_sub(header))),
            b"mdat" => mdats.push((pos, pos.saturating_add(size))),
            _ => {}
        }
        pos = pos.saturating_add(size);
    }
    let Some((moov_at, moov_len)) = moov else {
        return Ok(MediaVerdict::Unknown("no movie index (moov) found".into()));
    };
    if moov_len > MAX_MOOV || moov_at.saturating_add(moov_len) > len {
        return Ok(MediaVerdict::Damaged {
            from: 1.0,
            bad: 1,
            checked: 1,
            reason: "the movie index is truncated".into(),
        });
    }
    let mut index = vec![0u8; usize::try_from(moov_len).unwrap_or(0)];
    let got = src.read_exact_at(at.saturating_add(moov_at), &mut index)?;
    index.truncate(got);

    let tracks: Vec<Track> =
        boxes(&index).filter(|(t, _)| t == b"trak").filter_map(|(_, b)| parse_track(b)).collect();
    if tracks.is_empty() {
        // Real movies always have tracks; finding none means this index layout was not
        // understood, which is not evidence of damage.
        return Ok(MediaVerdict::Unknown("no tracks found in the movie index".into()));
    }
    let in_mdat = |off: u64, end: u64| mdats.iter().any(|&(a, b)| off >= a && end <= b);

    let mut checked = 0u32;
    let mut bad = 0u32;
    let mut first_bad: Option<f64> = None;
    let mut reason = String::new();
    let mut any_video = false;
    for t in &tracks {
        let layout = chunk_layout(t);
        let total = layout.len().max(1);
        // Every chunk must lie inside the file's media data.
        for (i, &(off, first, count)) in layout.iter().enumerate() {
            let bytes: u64 = t
                .sample_sizes
                .get(first..first.saturating_add(count as usize))
                .map_or(0, |s| s.iter().map(|&x| u64::from(x)).sum());
            if !in_mdat(off, off.saturating_add(bytes)) {
                bad = bad.saturating_add(1);
                let f = i as f64 / total as f64;
                first_bad = Some(first_bad.map_or(f, |b: f64| b.min(f)));
                if reason.is_empty() {
                    reason = "the index points outside the media data".into();
                }
                break;
            }
        }
        let Some((nal_len, codec)) = t.nal.filter(|_| t.video) else { continue };
        any_video = true;
        let step = (layout.len() / SAMPLES_CHECKED).max(1);
        for (i, &(off, first, _)) in layout.iter().enumerate().step_by(step) {
            let Some(&size) = t.sample_sizes.get(first) else { break };
            checked = checked.saturating_add(1);
            if !sample_ok(src, at.saturating_add(off), size, nal_len, codec)? {
                bad = bad.saturating_add(1);
                let f = i as f64 / total as f64;
                first_bad = Some(first_bad.map_or(f, |b: f64| b.min(f)));
                if reason.is_empty() {
                    reason = "video frames contain data that is not video (overwritten or misplaced)".into();
                }
            }
        }
    }
    if bad > 0 {
        return Ok(MediaVerdict::Damaged {
            from: first_bad.unwrap_or(0.0),
            bad,
            checked: checked.max(1),
            reason,
        });
    }
    if !any_video {
        return Ok(MediaVerdict::Unknown("no H.264/H.265 video track to check".into()));
    }
    Ok(MediaVerdict::Verified { samples_checked: checked })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use khodz_io::MemSource;

    fn bx(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(typ);
        v.extend_from_slice(body);
        v
    }

    fn full(typ: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 4];
        b.extend_from_slice(body);
        bx(typ, &b)
    }

    /// A well-formed H.264 sample of `size` bytes: one NAL unit (type 1) with 4-byte length.
    fn sample(size: usize, seed: u8) -> Vec<u8> {
        let mut s = ((size - 4) as u32).to_be_bytes().to_vec();
        s.push(0x41); // nal_ref_idc 2, type 1
        s.extend((0..size - 5).map(|i| (i as u8).wrapping_mul(seed) | 1));
        s
    }

    /// Build a QuickTime-style file: ftyp, mdat (n samples, one per chunk), moov at the end.
    pub(crate) fn movie(n: usize, size: usize) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"qt  \0\0\0\0qt  ");
        let mut mdat_body = Vec::new();
        let mdat_start = ftyp.len() + 8;
        let mut offsets = Vec::new();
        for i in 0..n {
            offsets.push((mdat_start + mdat_body.len()) as u32);
            mdat_body.extend(sample(size, (i as u8).wrapping_add(3)));
        }
        let mdat = bx(b"mdat", &mdat_body);
        let mut avc1 = vec![0u8; 78];
        avc1.extend(bx(b"avcC", &[1, 0x64, 0, 0x28, 0xFF, 0xE1]));
        let stsd = full(b"stsd", &[&1u32.to_be_bytes()[..], &bx(b"avc1", &avc1)].concat());
        let mut stco = (n as u32).to_be_bytes().to_vec();
        offsets.iter().for_each(|o| stco.extend(o.to_be_bytes()));
        let mut stsz = 0u32.to_be_bytes().to_vec();
        stsz.extend((n as u32).to_be_bytes());
        (0..n).for_each(|_| stsz.extend((size as u32).to_be_bytes()));
        let stsc = [1u32.to_be_bytes(), 1u32.to_be_bytes(), 1u32.to_be_bytes(), 1u32.to_be_bytes()].concat();
        let stbl =
            bx(b"stbl", &[stsd, full(b"stco", &stco), full(b"stsz", &stsz), full(b"stsc", &stsc)].concat());
        let hdlr = full(b"hdlr", b"\0\0\0\0vide\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let mdia = bx(b"mdia", &[hdlr, bx(b"minf", &stbl)].concat());
        let moov = bx(b"moov", &bx(b"trak", &mdia));
        [ftyp, mdat, moov].concat()
    }

    /// ftyp (20 bytes) + mdat header (8): where the first sample starts.
    const MDAT_DATA: usize = 28;

    fn verdict(data: Vec<u8>) -> MediaVerdict {
        let len = data.len() as u64;
        verify_isobmff(&MemSource::new(data, 512), 0, len).unwrap()
    }

    #[test]
    fn intact_movie_is_verified() {
        assert_eq!(verdict(movie(300, 1000)), MediaVerdict::Verified { samples_checked: 300 });
    }

    #[test]
    fn overwritten_middle_is_found_and_located() {
        let mut m = movie(300, 1000);
        // Overwrite samples ~150..200 with another file's bytes (e.g. a JPEG).
        let start = MDAT_DATA + 150 * 1000;
        for (i, b) in m[start..start + 50 * 1000].iter_mut().enumerate() {
            *b = [0xFF, 0xD8, 0xFF, 0xE1, 0x12][i % 5];
        }
        match verdict(m) {
            MediaVerdict::Damaged { from, bad, .. } => {
                assert!((0.45..0.55).contains(&from), "breaks about half way: {from}");
                assert!(bad >= 40);
            }
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn zeroed_media_is_damaged_from_the_start() {
        let mut m = movie(50, 800);
        m[MDAT_DATA..MDAT_DATA + 50 * 800].fill(0);
        assert!(matches!(verdict(m), MediaVerdict::Damaged { from, .. } if from == 0.0));
    }

    #[test]
    fn missing_or_truncated_index_is_reported() {
        let m = movie(20, 500);
        let cut = m.len() - 40;
        assert!(matches!(
            verdict(m[..cut].to_vec()),
            MediaVerdict::Damaged { .. } | MediaVerdict::Unknown(_)
        ));
        assert!(matches!(verdict(m[..MDAT_DATA + 20 * 500].to_vec()), MediaVerdict::Unknown(_)));
    }

    #[test]
    fn hostile_tables_do_not_panic() {
        let mut m = movie(10, 300);
        let n = m.len();
        for i in (n - 200..n).step_by(7) {
            m[i] = 0xFF;
        }
        let _ = verdict(m);
    }
}
