//! Per-format header detection and structural validation.

use std::collections::HashSet;

use khodz_core::Result;
use khodz_io::BlockSource;

use crate::stream::Stream;
use crate::{CarveStatus, Carved, FileKind};

const MIB: u64 = 1 << 20;
const MAX_JPEG: u64 = 128 * MIB;
const MAX_PNG: u64 = 256 * MIB;
const MAX_GIF: u64 = 64 * MIB;
const MAX_PDF: u64 = 256 * MIB;
const MAX_ZIP: u64 = 2048 * MIB;
const MAX_MP4: u64 = 4096 * MIB;
const MAX_RIFF: u64 = 4096 * MIB;
const MAX_BMP: u64 = 256 * MIB;
/// After a validated PDF end, keep looking this far for incremental-update sections.
const PDF_TAIL: u64 = MIB;
/// PDF search window: 1 MiB plus overlap so markers spanning a boundary are seen.
const PDF_BUF: usize = (1 << 20) + 16;

/// Size of a GIF colour table from a packed-fields byte: 3 × 2^(n+1) bytes, at most 768.
fn color_table_len(flags: u8) -> u64 {
    3u64.checked_shl(u32::from(flags & 0x07).saturating_add(1)).unwrap_or(u64::MAX)
}

pub fn looks_like_header(h: &[u8]) -> bool {
    h.starts_with(&[0xFF, 0xD8, 0xFF])
        || h.starts_with(b"\x89PNG\r\n\x1a\n")
        || h.starts_with(b"GIF87a")
        || h.starts_with(b"GIF89a")
        || (h.starts_with(b"BM") && h.get(6..10) == Some(&[0, 0, 0, 0]))
        || h.starts_with(b"%PDF-")
        || h.starts_with(b"PK\x03\x04")
        || h.get(4..8) == Some(b"ftyp")
        || h.starts_with(b"RIFF")
}

fn carved(offset: u64, len: u64, kind: FileKind, status: CarveStatus, note: Option<&str>) -> Option<Carved> {
    (len > 0).then(|| Carved { offset, len, kind, status, note: note.map(String::from) })
}

/// Validate a candidate at `at`. `None` means "not a file of a known type" (rejected).
pub fn validate_at<S: BlockSource + ?Sized>(src: &S, at: u64) -> Result<Option<Carved>> {
    let mut head = [0u8; 32];
    let n = src.read_exact_at(at, &mut head)?;
    let h = &head[..n];
    if h.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return jpeg(src, at);
    }
    if h.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png(src, at);
    }
    if h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") {
        return gif(src, at);
    }
    if h.starts_with(b"BM") {
        return Ok(bmp(h, src, at));
    }
    if h.starts_with(b"%PDF-") {
        return pdf(src, at);
    }
    if h.starts_with(b"PK\x03\x04") {
        return zip(src, at);
    }
    if h.get(4..8) == Some(b"ftyp") {
        return mp4(src, at);
    }
    if h.starts_with(b"RIFF") {
        return Ok(riff(h, src, at));
    }
    Ok(None)
}

// ---------------------------------------------------------------- JPEG

fn jpeg<S: BlockSource + ?Sized>(src: &S, at: u64) -> Result<Option<Carved>> {
    let mut s = Stream::new(src, at, MAX_JPEG);
    s.skip(2); // SOI
    let mut seen_sof = false;
    let mut seen_sos = false;
    let mut segments = 0u32;
    let mut pending: Option<u8> = None;
    let fail = |pos: u64, segments: u32, why: &str| -> Option<Carved> {
        if segments == 0 { None } else { carved(at, pos, FileKind::Jpeg, CarveStatus::Partial, Some(why)) }
    };

    loop {
        let marker_pos = s.pos;
        let m = match pending.take() {
            Some(m) => m,
            None => {
                match s.byte()? {
                    Some(0xFF) => {}
                    Some(_) => return Ok(fail(marker_pos, segments, "expected a marker")),
                    None => return Ok(fail(marker_pos, segments, "truncated")),
                }
                let mut m = 0xFFu8;
                while m == 0xFF {
                    m = match s.byte()? {
                        Some(b) => b,
                        None => return Ok(fail(marker_pos, segments, "truncated")),
                    };
                }
                m
            }
        };
        match m {
            0xD9 => {
                return Ok(if seen_sos {
                    carved(at, s.pos, FileKind::Jpeg, CarveStatus::Complete, None)
                } else {
                    fail(marker_pos, segments, "end marker before image data")
                });
            }
            0x01 | 0xD0..=0xD7 => {}
            0xDA => {
                let Some(len) = s.u16_be()? else { return Ok(fail(marker_pos, segments, "truncated")) };
                let Some(ns) = s.byte()? else { return Ok(fail(marker_pos, segments, "truncated")) };
                if !seen_sof
                    || !(1..=4).contains(&ns)
                    || u32::from(len) != u32::from(ns).saturating_mul(2).saturating_add(6)
                {
                    return Ok(fail(marker_pos, segments, "invalid scan header"));
                }
                if !s.skip(u64::from(len).saturating_sub(3)) {
                    return Ok(fail(marker_pos, segments, "truncated"));
                }
                seen_sos = true;
                segments = segments.saturating_add(1);
                // Entropy-coded data: only stuffed FF00 and restart markers may appear inside.
                // A run of 512+ zero bytes does not occur in real Huffman-coded data but is what
                // a TRIMmed / zeroed fragment gap looks like, so it ends the valid part.
                let mut zeros = 0u32;
                loop {
                    let Some(b) = s.byte()? else {
                        return Ok(fail(s.pos, segments, "truncated in image data"));
                    };
                    if b == 0 {
                        zeros = zeros.saturating_add(1);
                        if zeros >= 512 {
                            let at_gap = s.pos.saturating_sub(u64::from(zeros));
                            return Ok(fail(
                                at_gap,
                                segments,
                                "zero-filled region in image data (erased or fragmented)",
                            ));
                        }
                        continue;
                    }
                    zeros = 0;
                    if b != 0xFF {
                        continue;
                    }
                    let mut n = 0xFFu8;
                    while n == 0xFF {
                        n = match s.byte()? {
                            Some(x) => x,
                            None => return Ok(fail(s.pos, segments, "truncated in image data")),
                        };
                    }
                    match n {
                        0x00 | 0xD0..=0xD7 => {}
                        0xC0..=0xFE => {
                            pending = Some(n);
                            break;
                        }
                        _ => {
                            return Ok(fail(
                                s.pos.saturating_sub(2),
                                segments,
                                "corrupt image data (fragmented?)",
                            ));
                        }
                    }
                }
            }
            0xC0..=0xFE => {
                if m == 0xD8 {
                    return Ok(fail(marker_pos, segments, "unexpected start marker"));
                }
                let Some(len) = s.u16_be()? else { return Ok(fail(marker_pos, segments, "truncated")) };
                if len < 2 {
                    return Ok(fail(marker_pos, segments, "invalid segment length"));
                }
                let is_sof = matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC);
                if is_sof {
                    if len < 11 {
                        return Ok(fail(marker_pos, segments, "invalid frame header"));
                    }
                    seen_sof = true;
                }
                if !s.skip(u64::from(len).saturating_sub(2)) {
                    return Ok(fail(marker_pos, segments, "truncated"));
                }
                segments = segments.saturating_add(1);
            }
            _ => return Ok(fail(marker_pos, segments, "invalid marker")),
        }
    }
}

// ---------------------------------------------------------------- PNG

fn png<S: BlockSource + ?Sized>(src: &S, at: u64) -> Result<Option<Carved>> {
    let mut s = Stream::new(src, at, MAX_PNG);
    s.skip(8);
    let mut first = true;
    loop {
        let chunk_start = s.pos;
        let partial = |why: &str| carved(at, chunk_start, FileKind::Png, CarveStatus::Partial, Some(why));
        let Some(len) = s.u32_be()? else { return Ok(if first { None } else { partial("truncated") }) };
        let Some(kind) = s.take(4)? else { return Ok(if first { None } else { partial("truncated") }) };
        if len > 0x7FFF_FFFF || !kind.iter().all(u8::is_ascii_alphabetic) {
            return Ok(if first { None } else { partial("invalid chunk (fragmented?)") });
        }
        if first && (kind.as_slice() != b"IHDR" || len != 13) {
            return Ok(None);
        }
        let mut crc = crc32fast::Hasher::new();
        crc.update(&kind);
        if !s.consume(u64::from(len), |d| crc.update(d))? {
            return Ok(if first { None } else { partial("truncated") });
        }
        let Some(stored) = s.u32_be()? else { return Ok(if first { None } else { partial("truncated") }) };
        if stored != crc.finalize() {
            return Ok(if first { None } else { partial("chunk CRC mismatch (corrupt or fragmented)") });
        }
        first = false;
        if kind.as_slice() == b"IEND" {
            return Ok(carved(at, s.pos, FileKind::Png, CarveStatus::Complete, None));
        }
    }
}

// ---------------------------------------------------------------- GIF

/// Streaming GIF LZW *validator*: tracks code widths and string lengths (not pixels) to prove
/// the compressed stream is well-formed and decodes to exactly the declared pixel count.
pub(crate) struct LzwCheck {
    min: u32,
    width: u32,
    next: u32,
    prev: Option<u32>,
    lens: Vec<u32>,
    acc: u64,
    nbits: u32,
    pub pixels: u64,
    pub ended: bool,
    pub invalid: bool,
}

// All quantities here are bounded by the GIF format itself: min code size ≤ 8 (checked by the
// caller), code width ≤ 12, table size ≤ 4096, string lengths ≤ 4096, bit buffer < 8 + 12 bits.
#[allow(clippy::arithmetic_side_effects)]
impl LzwCheck {
    pub(crate) fn new(min: u8) -> Self {
        let min = u32::from(min);
        let mut c = Self {
            min,
            width: min + 1,
            next: 0,
            prev: None,
            lens: vec![1; 4096],
            acc: 0,
            nbits: 0,
            pixels: 0,
            ended: false,
            invalid: false,
        };
        c.reset();
        c
    }

    fn reset(&mut self) {
        self.width = self.min + 1;
        self.next = (1 << self.min) + 2;
        self.prev = None;
    }

    pub(crate) fn feed(&mut self, data: &[u8]) {
        let clear = 1u32 << self.min;
        let eoi = clear + 1;
        for &b in data {
            if self.ended || self.invalid {
                return;
            }
            self.acc |= u64::from(b) << self.nbits;
            self.nbits += 8;
            while self.nbits >= self.width && !self.ended && !self.invalid {
                let code = (self.acc & ((1u64 << self.width) - 1)) as u32;
                self.acc >>= self.width;
                self.nbits -= self.width;
                if code == clear {
                    self.reset();
                    continue;
                }
                if code == eoi {
                    self.ended = true;
                    return;
                }
                let len = match self.prev {
                    None if code < clear => 1,
                    None => {
                        self.invalid = true;
                        return;
                    }
                    Some(p) => {
                        let len = if code < self.next {
                            self.lens[code as usize]
                        } else if code == self.next {
                            self.lens[p as usize] + 1
                        } else {
                            self.invalid = true;
                            return;
                        };
                        if self.next < 4096 {
                            self.lens[self.next as usize] = self.lens[p as usize] + 1;
                            self.next += 1;
                            if self.next == (1 << self.width) && self.width < 12 {
                                self.width += 1;
                            }
                        }
                        len
                    }
                };
                self.pixels += u64::from(len);
                self.prev = Some(code);
            }
        }
    }
}

fn gif<S: BlockSource + ?Sized>(src: &S, at: u64) -> Result<Option<Carved>> {
    let mut s = Stream::new(src, at, MAX_GIF);
    s.skip(6);
    let Some(lsd) = s.take(7)? else { return Ok(None) };
    if lsd[4] & 0x80 != 0 && !s.skip(color_table_len(lsd[4])) {
        return Ok(None);
    }
    let mut images = 0u32;
    // Sub-block chain: length byte, data, ..., 0. Optionally feeds the data to an LZW check.
    fn sub_blocks<S: BlockSource + ?Sized>(
        s: &mut Stream<'_, S>,
        mut lzw: Option<&mut LzwCheck>,
    ) -> Result<bool> {
        loop {
            match s.byte()? {
                Some(0) => return Ok(true),
                Some(n) => {
                    let ok = match lzw.as_deref_mut() {
                        Some(l) => s.consume(u64::from(n), |d| l.feed(d))?,
                        None => s.skip(u64::from(n)),
                    };
                    if !ok {
                        return Ok(false);
                    }
                }
                None => return Ok(false),
            }
        }
    }
    loop {
        let block = s.pos;
        let partial = |why: &str| carved(at, block, FileKind::Gif, CarveStatus::Partial, Some(why));
        match s.byte()? {
            Some(0x3B) => {
                return Ok(if images > 0 {
                    carved(at, s.pos, FileKind::Gif, CarveStatus::Complete, None)
                } else {
                    partial("no image data")
                });
            }
            Some(0x21) => {
                if s.byte()?.is_none() || !sub_blocks(&mut s, None)? {
                    return Ok(partial("truncated extension"));
                }
            }
            Some(0x2C) => {
                let Some(desc) = s.take(9)? else { return Ok(partial("truncated image")) };
                let w = u64::from(u16::from_le_bytes([desc[4], desc[5]]));
                let h = u64::from(u16::from_le_bytes([desc[6], desc[7]]));
                if desc[8] & 0x80 != 0 && !s.skip(color_table_len(desc[8])) {
                    return Ok(partial("truncated colour table"));
                }
                let Some(min) = s.byte()? else { return Ok(partial("truncated image")) };
                if !(2..=8).contains(&min) {
                    return Ok(partial("corrupt image data"));
                }
                let mut lzw = LzwCheck::new(min);
                if !sub_blocks(&mut s, Some(&mut lzw))? {
                    return Ok(partial("truncated image data"));
                }
                // The compressed stream must decode to exactly width Ã— height pixels.
                if lzw.invalid || lzw.pixels != w.saturating_mul(h) {
                    return Ok(partial("image data does not decode (corrupt or fragmented)"));
                }
                images = images.saturating_add(1);
            }
            _ => return Ok(if images > 0 { partial("invalid block (fragmented?)") } else { None }),
        }
    }
}

// ---------------------------------------------------------------- BMP (unverifiable payload)

fn bmp<S: BlockSource + ?Sized>(h: &[u8], src: &S, at: u64) -> Option<Carved> {
    let u32le = |o: usize| khodz_core::bytes::le_u32(h, o);
    let size = u64::from(u32le(2)?);
    let data_off = u64::from(u32le(10)?);
    let dib = u32le(14)?;
    if !matches!(dib, 12 | 40 | 52 | 56 | 108 | 124)
        || data_off < 14 + u64::from(dib)
        || data_off >= size
        || size > MAX_BMP
    {
        return None;
    }
    if dib >= 40 {
        let planes = h.get(26..28).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
        let bpp = h.get(28..30).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
        if planes != 1 || !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
            return None;
        }
    }
    let avail = src.len().saturating_sub(at);
    if size > avail {
        return carved(at, avail, FileKind::Bmp, CarveStatus::Partial, Some("truncated"));
    }
    carved(at, size, FileKind::Bmp, CarveStatus::Unverified, Some("bitmap pixels cannot be verified"))
}

// ---------------------------------------------------------------- PDF

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Does `startxref` before the `%%EOF` at file-relative `eof` point at a real xref table or
/// cross-reference stream object inside the file?
fn pdf_xref_ok<S: BlockSource + ?Sized>(src: &S, at: u64, eof: u64) -> Result<bool> {
    let back = eof.min(128);
    let mut tail = vec![0u8; usize::try_from(back).unwrap_or(0)];
    src.read_exact_at(at.saturating_add(eof.saturating_sub(back)), &mut tail)?;
    let Some(p) = rfind(&tail, b"startxref") else { return Ok(false) };
    let digits: String = tail[p.saturating_add(9)..]
        .iter()
        .map(|&b| b as char)
        .skip_while(|c| c.is_ascii_whitespace())
        .take_while(char::is_ascii_digit)
        .collect();
    let Ok(xref) = digits.parse::<u64>() else { return Ok(false) };
    if xref == 0 || xref >= eof {
        return Ok(false);
    }
    let mut probe = [0u8; 24];
    let n = src.read_exact_at(at.saturating_add(xref), &mut probe)?;
    let p = &probe[..n];
    if p.starts_with(b"xref") {
        return Ok(true);
    }
    // "<num> <gen> obj" for cross-reference streams.
    let text: String = p.iter().map(|&b| b as char).collect();
    let mut parts = text.split_ascii_whitespace();
    Ok(matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), Some(o))
        if a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()) && o.starts_with("obj")))
}

fn pdf<S: BlockSource + ?Sized>(src: &S, at: u64) -> Result<Option<Carved>> {
    let mut head = [0u8; 8];
    src.read_exact_at(at, &mut head)?;
    if !head[5].is_ascii_digit() || head[6] != b'.' {
        return Ok(None);
    }
    let limit = MAX_PDF.min(src.len().saturating_sub(at));
    let mut buf = vec![0u8; PDF_BUF];
    let mut pos = 0u64;
    let mut last_eof: Option<u64> = None;
    let mut good_end: Option<u64> = None;
    while pos < limit {
        if good_end.is_some_and(|e| pos > e.saturating_add(PDF_TAIL)) {
            break;
        }
        let want = usize::try_from(limit.saturating_sub(pos)).unwrap_or(0).min(buf.len());
        let got = src.read_exact_at(at.saturating_add(pos), &mut buf[..want])?;
        if got == 0 {
            break;
        }
        let window = &buf[..got];
        let mut from = 0usize;
        while let Some(i) = find(&window[from..], b"%%EOF") {
            let eof = pos.saturating_add(from.saturating_add(i) as u64);
            last_eof = Some(eof);
            if pdf_xref_ok(src, at, eof)? {
                let mut end = eof.saturating_add(5);
                let mut eol = [0u8; 2];
                let k = src.read_exact_at(at.saturating_add(end), &mut eol)?;
                end =
                    end.saturating_add(
                        eol[..k].iter().take_while(|&&b| b == b'\r' || b == b'\n').count() as u64
                    );
                good_end = Some(end);
            }
            from = from.saturating_add(i).saturating_add(5);
        }
        // A new PDF header after a valid end means the next file began.
        if let (Some(e), Some(i)) = (good_end, find(window, b"%PDF-")) {
            if pos.saturating_add(i as u64) > e {
                break;
            }
        }
        // Overlap windows by 16 bytes so markers spanning a boundary are seen.
        pos = pos.saturating_add((got as u64).saturating_sub(16).max(1));
    }
    Ok(match (good_end, last_eof) {
        (Some(end), _) => carved(at, end, FileKind::Pdf, CarveStatus::Complete, None),
        (None, Some(eof)) => carved(
            at,
            eof.saturating_add(5),
            FileKind::Pdf,
            CarveStatus::Partial,
            Some("cross-reference offset invalid (corrupt or fragmented)"),
        ),
        (None, None) => {
            carved(at, limit.min(MIB), FileKind::Pdf, CarveStatus::Partial, Some("no end of file found"))
        }
    })
}

// ---------------------------------------------------------------- ZIP / OOXML

fn le16(b: &[u8], o: usize) -> u16 {
    khodz_core::bytes::le_u16(b, o).unwrap_or(0)
}
fn le32(b: &[u8], o: usize) -> u32 {
    khodz_core::bytes::le_u32(b, o).unwrap_or(0)
}

fn zip<S: BlockSource + ?Sized>(src: &S, at: u64) -> Result<Option<Carved>> {
    let mut s = Stream::new(src, at, MAX_ZIP);
    let mut locals: HashSet<u64> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    let mut cd_start: Option<u64> = None;
    let mut cd_entries = 0u64;
    let kind_of = |names: &[String]| {
        let has = |p: &str| names.iter().any(|n| n.starts_with(p));
        if names.iter().any(|n| n == "[Content_Types].xml") {
            if has("word/") {
                return FileKind::Docx;
            }
            if has("xl/") {
                return FileKind::Xlsx;
            }
            if has("ppt/") {
                return FileKind::Pptx;
            }
        }
        FileKind::Zip
    };
    let available = s.limit();
    loop {
        let rec = s.pos;
        let Some(sig) = s.take(4)? else {
            return Ok(carved(at, available, kind_of(&names), CarveStatus::Partial, Some("truncated")));
        };
        // Truncation keeps everything up to the end of the source; other breaks stop at the
        // last record that validated.
        let partial = |why: &str, names: &[String]| {
            let len = if why == "truncated" { available } else { rec };
            carved(at, len, kind_of(names), CarveStatus::Partial, Some(why))
        };
        match le32(&sig, 0) {
            0x0403_4b50 if cd_start.is_none() => {
                let Some(h) = s.take(26)? else { return Ok(partial("truncated", &names)) };
                let (flags, csize, nlen, xlen) = (le16(&h, 2), le32(&h, 14), le16(&h, 22), le16(&h, 24));
                let Some(name) = s.take(usize::from(nlen))? else { return Ok(partial("truncated", &names)) };
                if !s.skip(u64::from(xlen)) {
                    return Ok(partial("truncated", &names));
                }
                if csize == 0xFFFF_FFFF {
                    return Ok(partial("ZIP64 archives are not supported yet", &names));
                }
                if flags & 0x08 != 0 && csize == 0 {
                    // Sizes follow the data in a descriptor: search for the next record.
                    if !skip_to_next_zip_record(&mut s)? {
                        return Ok(partial("truncated", &names));
                    }
                } else if !s.skip(u64::from(csize)) {
                    return Ok(partial("truncated", &names));
                } else if flags & 0x08 != 0 {
                    // Optional descriptor signature + crc + sizes.
                    let save = s.pos;
                    if s.take(4)?.as_deref() != Some(b"PK\x07\x08") {
                        s.pos = save;
                        s.skip(12);
                    } else {
                        s.skip(12);
                    }
                }
                locals.insert(rec);
                names.push(String::from_utf8_lossy(&name).into_owned());
            }
            0x0201_4b50 => {
                cd_start.get_or_insert(rec);
                let Some(h) = s.take(42)? else { return Ok(partial("truncated", &names)) };
                let (nlen, xlen, clen, local) = (le16(&h, 24), le16(&h, 26), le16(&h, 28), le32(&h, 38));
                if !locals.contains(&u64::from(local)) {
                    return Ok(partial(
                        "central directory does not match file entries (fragmented?)",
                        &names,
                    ));
                }
                if !s.skip(u64::from(nlen).saturating_add(u64::from(xlen)).saturating_add(u64::from(clen))) {
                    return Ok(partial("truncated", &names));
                }
                cd_entries = cd_entries.saturating_add(1);
            }
            0x0605_4b50 => {
                let Some(h) = s.take(18)? else { return Ok(partial("truncated", &names)) };
                let (total, cd_off, clen) = (u64::from(le16(&h, 6)), u64::from(le32(&h, 12)), le16(&h, 16));
                let consistent =
                    cd_start == Some(cd_off) && total == cd_entries && total == locals.len() as u64;
                if !consistent || !s.skip(u64::from(clen)) {
                    return Ok(partial("end record inconsistent (corrupt or fragmented)", &names));
                }
                return Ok(carved(at, s.pos, kind_of(&names), CarveStatus::Complete, None));
            }
            _ => {
                if locals.is_empty() {
                    return Ok(None);
                }
                return Ok(partial("unexpected data (fragmented?)", &names));
            }
        }
    }
}

/// Advance to just before the next `PK\x01\x02`, `PK\x03\x04` or after a `PK\x07\x08`
/// descriptor. Used for entries whose sizes are only stored after the data.
fn skip_to_next_zip_record<S: BlockSource + ?Sized>(s: &mut Stream<'_, S>) -> Result<bool> {
    let mut window = [0u8; 4];
    loop {
        let Some(b) = s.byte()? else { return Ok(false) };
        window.rotate_left(1);
        window[3] = b;
        match &window {
            b"PK\x07\x08" => return Ok(s.skip(12)),
            b"PK\x01\x02" | b"PK\x03\x04" => {
                s.pos = s.pos.saturating_sub(4);
                return Ok(true);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- MP4 / MOV / HEIC

const TOP_LEVEL_BOXES: &[&[u8; 4]] = &[
    b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"uuid", b"meta", b"pdin", b"moof", b"mfra",
    b"styp", b"sidx", b"pnot", b"prfl", b"emsg",
];

fn mp4<S: BlockSource + ?Sized>(src: &S, at: u64) -> Result<Option<Carved>> {
    let mut s = Stream::new(src, at, MAX_MP4);
    let mut seen: HashSet<[u8; 4]> = HashSet::new();
    let mut kind = FileKind::Mp4;
    loop {
        let start = s.pos;
        let Some(size32) = s.u32_be()? else { break };
        let Some(t) = s.take(4)? else { break };
        let typ: [u8; 4] = [t[0], t[1], t[2], t[3]];
        if !TOP_LEVEL_BOXES.contains(&&typ) {
            s.pos = start;
            break;
        }
        if start == 0 && &typ != b"ftyp" {
            return Ok(None);
        }
        let (size, header) = match size32 {
            0 => {
                return Ok(carved(
                    at,
                    start,
                    kind,
                    CarveStatus::Partial,
                    Some("box extends to end of stream"),
                ));
            }
            1 => match s.take(8)? {
                Some(b) => (u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]), 16),
                None => break,
            },
            n => (u64::from(n), 8),
        };
        if size < header {
            return Ok(if start == 0 {
                None
            } else {
                carved(at, start, kind, CarveStatus::Partial, Some("invalid box size"))
            });
        }
        if &typ == b"ftyp" {
            if let Some(b) = s.take(4)? {
                kind = match &b[..] {
                    b"qt  " => FileKind::Mov,
                    b"heic" | b"heix" | b"mif1" | b"msf1" => FileKind::Heic,
                    _ => FileKind::Mp4,
                };
                s.pos = s.pos.saturating_sub(4);
            }
        }
        if !s.skip(size.saturating_sub(header)) {
            return Ok(carved(at, start, kind, CarveStatus::Partial, Some("truncated")));
        }
        seen.insert(typ);
    }
    let end = s.pos;
    if end == 0 {
        return Ok(None);
    }
    let complete = if kind == FileKind::Heic {
        seen.contains(b"meta") && seen.contains(b"mdat")
    } else {
        seen.contains(b"moov") && (seen.contains(b"mdat") || seen.contains(b"moof"))
    };
    // The box chain is consistent, but the media payload (mdat) is opaque: a fragment gap
    // inside it cannot be detected, so this is never reported as Complete.
    Ok(if complete {
        carved(
            at,
            end,
            kind,
            CarveStatus::Unverified,
            Some("container structure valid; media data not verifiable"),
        )
    } else {
        carved(at, end, kind, CarveStatus::Partial, Some("missing movie header or media data"))
    })
}

// ---------------------------------------------------------------- RIFF (WAV / AVI / WebP)

fn riff<S: BlockSource + ?Sized>(h: &[u8], src: &S, at: u64) -> Option<Carved> {
    let size =
        u64::from(u32::from_le_bytes([*h.get(4)?, *h.get(5)?, *h.get(6)?, *h.get(7)?])).checked_add(8)?;
    let kind = match h.get(8..12)? {
        b"WAVE" => FileKind::Wav,
        b"AVI " => FileKind::Avi,
        b"WEBP" => FileKind::Webp,
        _ => return None,
    };
    if !(12..=MAX_RIFF).contains(&size) {
        return None;
    }
    let avail = src.len().saturating_sub(at);
    if size > avail {
        return carved(at, avail, kind, CarveStatus::Partial, Some("truncated"));
    }
    carved(at, size, kind, CarveStatus::Unverified, Some("declared length only; payload not verified"))
}
