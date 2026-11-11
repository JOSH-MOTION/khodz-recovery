//! Carving on synthetic media built from the format specifications. Real-encoder output is
//! covered by tests/encoder_fixture.rs.
#![allow(clippy::arithmetic_side_effects)]

use khodz_carve::*;
use khodz_core::CancelToken;
use khodz_io::MemSource;

const SECTOR: usize = 512;

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 32) as u8
        })
        .collect()
}

fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    c.extend_from_slice(&h.finalize().to_be_bytes());
    c
}

fn png(payload: usize, seed: u64) -> Vec<u8> {
    let mut f = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&64u32.to_be_bytes());
    ihdr.extend_from_slice(&64u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    f.extend(chunk(b"IHDR", &ihdr));
    // Several IDAT chunks so fragmentation lands inside image data.
    for part in noise(payload, seed).chunks(4096) {
        f.extend(chunk(b"IDAT", part));
    }
    f.extend(chunk(b"IEND", &[]));
    f
}

/// Structurally valid JPEG: SOI, APP0, DQT, SOF0, DHT, SOS, stuffed entropy data with
/// restart markers, EOI.
fn jpeg(entropy: usize, seed: u64) -> Vec<u8> {
    let mut f = vec![0xFF, 0xD8];
    let seg = |f: &mut Vec<u8>, m: u8, body: &[u8]| {
        f.extend_from_slice(&[0xFF, m]);
        f.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        f.extend_from_slice(body);
    };
    seg(&mut f, 0xE0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
    seg(&mut f, 0xDB, &[0u8; 65]);
    seg(&mut f, 0xC0, &[8, 0, 64, 0, 64, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    seg(&mut f, 0xC4, &[0u8; 29]);
    seg(&mut f, 0xDA, &[3, 1, 0, 2, 0x11, 3, 0x11, 0, 63, 0]);
    for (i, b) in noise(entropy, seed).into_iter().enumerate() {
        f.push(b);
        if b == 0xFF {
            f.push(0x00);
        }
        if i % 5000 == 4999 {
            f.extend_from_slice(&[0xFF, 0xD0 + ((i / 5000) % 8) as u8]);
        }
    }
    f.extend_from_slice(&[0xFF, 0xD9]);
    f
}

/// Standard GIF LZW encoder (variable code width, clear on a full table).
fn lzw_encode(min: u8, pixels: &[u8]) -> Vec<u8> {
    use std::collections::HashMap;
    let clear = 1u32 << min;
    let eoi = clear + 1;
    let (mut dict, mut next, mut width) = (HashMap::<(u32, u8), u32>::new(), eoi + 1, u32::from(min) + 1);
    let (mut out, mut acc, mut nbits) = (Vec::new(), 0u64, 0u32);
    let mut emit = |code: u32, width: u32, out: &mut Vec<u8>| {
        acc |= u64::from(code) << nbits;
        nbits += width;
        while nbits >= 8 {
            out.push(acc as u8);
            acc >>= 8;
            nbits -= 8;
        }
    };
    emit(clear, width, &mut out);
    let mut cur: Option<u32> = None;
    for &p in pixels {
        let Some(c) = cur else {
            cur = Some(u32::from(p));
            continue;
        };
        if let Some(&code) = dict.get(&(c, p)) {
            cur = Some(code);
            continue;
        }
        emit(c, width, &mut out);
        if next < 4096 {
            dict.insert((c, p), next);
            next += 1;
            // The decoder adds this entry one code later, so widen when it would.
            if next > (1 << width) && width < 12 {
                width += 1;
            }
        } else {
            emit(clear, width, &mut out);
            dict.clear();
            next = eoi + 1;
            width = u32::from(min) + 1;
        }
        cur = Some(u32::from(p));
    }
    if let Some(c) = cur {
        emit(c, width, &mut out);
    }
    emit(eoi, width, &mut out);
    if nbits > 0 {
        out.push(acc as u8);
    }
    out
}

fn gif(pixels: usize, seed: u64) -> Vec<u8> {
    let side = (pixels as f64).sqrt() as u16;
    let n = usize::from(side) * usize::from(side);
    // Low-entropy pixels (like real images) so the dictionary is exercised.
    let px: Vec<u8> = noise(n, seed).iter().map(|b| b % 4).collect();
    let mut f = b"GIF89a".to_vec();
    f.extend_from_slice(&side.to_le_bytes());
    f.extend_from_slice(&side.to_le_bytes());
    f.extend_from_slice(&[0x81, 0, 0]); // global colour table, 4 entries
    f.extend(noise(12, seed + 1));
    f.extend_from_slice(&[0x21, 0xF9, 4, 0, 0, 0, 0, 0]); // graphic control extension
    f.push(0x2C);
    f.extend_from_slice(&[0, 0, 0, 0]);
    f.extend_from_slice(&side.to_le_bytes());
    f.extend_from_slice(&side.to_le_bytes());
    f.push(0);
    f.push(2); // LZW min code size
    for part in lzw_encode(2, &px).chunks(255) {
        f.push(part.len() as u8);
        f.extend_from_slice(part);
    }
    f.push(0);
    f.push(0x3B);
    f
}

#[test]
fn gif_lzw_round_trip_is_accepted_and_corruption_rejected() {
    let good = gif(40_000, 61);
    let c = &carve_all(good.clone())[0];
    assert_eq!(c.status, CarveStatus::Complete, "{c:?}");
    let mut bad = good;
    let mid = bad.len() / 2;
    for b in &mut bad[mid..mid + 40] {
        *b ^= 0xA5;
    }
    assert_ne!(carve_all(bad)[0].status, CarveStatus::Complete);
}

fn zip(entries: &[(&str, usize)], seed: u64) -> Vec<u8> {
    let mut f = Vec::new();
    let mut central = Vec::new();
    for (i, (name, size)) in entries.iter().enumerate() {
        let data = noise(*size, seed + i as u64);
        let crc = crc32fast::hash(&data);
        let off = f.len() as u32;
        let mut lh = b"PK\x03\x04".to_vec();
        lh.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        lh.extend_from_slice(&crc.to_le_bytes());
        lh.extend_from_slice(&(data.len() as u32).to_le_bytes());
        lh.extend_from_slice(&(data.len() as u32).to_le_bytes());
        lh.extend_from_slice(&(name.len() as u16).to_le_bytes());
        lh.extend_from_slice(&0u16.to_le_bytes());
        lh.extend_from_slice(name.as_bytes());
        f.extend(lh);
        f.extend(&data);
        let mut ch = b"PK\x01\x02".to_vec();
        ch.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        ch.extend_from_slice(&crc.to_le_bytes());
        ch.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ch.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ch.extend_from_slice(&(name.len() as u16).to_le_bytes());
        ch.extend_from_slice(&[0u8; 12]);
        ch.extend_from_slice(&off.to_le_bytes());
        ch.extend_from_slice(name.as_bytes());
        central.extend(ch);
    }
    let cd_off = f.len() as u32;
    let cd_len = central.len() as u32;
    f.extend(central);
    let mut eocd = b"PK\x05\x06".to_vec();
    eocd.extend_from_slice(&[0, 0, 0, 0]);
    eocd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    eocd.extend_from_slice(&cd_len.to_le_bytes());
    eocd.extend_from_slice(&cd_off.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes());
    f.extend(eocd);
    f
}

fn pdf(body: usize) -> Vec<u8> {
    let mut f = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let obj1 = f.len();
    f.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let obj2 = f.len();
    f.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nstream\n");
    f.extend(noise(body, 77).iter().map(|b| b'a' + b % 26));
    f.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = f.len();
    f.extend_from_slice(
        format!("xref\n0 3\n0000000000 65535 f \n{obj1:010} 00000 n \n{obj2:010} 00000 n \n").as_bytes(),
    );
    f.extend_from_slice(format!("trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    f
}

fn mp4(mdat: usize) -> Vec<u8> {
    let bx = |t: &[u8], body: &[u8]| {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(t);
        b.extend_from_slice(body);
        b
    };
    let mut f = bx(b"ftyp", b"isom\0\0\x02\0isomiso2mp41");
    f.extend(bx(b"moov", &bx(b"mvhd", &[0u8; 100])));
    f.extend(bx(b"mdat", &noise(mdat, 5)));
    f
}

fn bmp() -> Vec<u8> {
    let (w, h) = (16u32, 16u32);
    let pixels = (w * h * 3) as usize;
    let size = 54 + pixels as u32;
    let mut f = b"BM".to_vec();
    f.extend_from_slice(&size.to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0]);
    f.extend_from_slice(&54u32.to_le_bytes());
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&w.to_le_bytes());
    f.extend_from_slice(&h.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&24u16.to_le_bytes());
    f.extend_from_slice(&[0u8; 24]);
    f.extend(noise(pixels, 9));
    f
}

fn wav(samples: usize) -> Vec<u8> {
    let data = noise(samples * 2, 11);
    let mut f = b"RIFF".to_vec();
    f.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&[1, 0, 1, 0, 0x44, 0xAC, 0, 0, 0x88, 0x58, 1, 0, 2, 0, 16, 0]);
    f.extend_from_slice(b"data");
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend(data);
    f
}

/// Lay files out at sector-aligned offsets separated by noise. Returns the image and the
/// (offset, length) of each file.
fn layout(files: &[Vec<u8>]) -> (Vec<u8>, Vec<(u64, u64)>) {
    let mut img = noise(4 * SECTOR, 1000);
    let mut at = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let off = img.len();
        img.extend_from_slice(f);
        let pad = (SECTOR - img.len() % SECTOR) % SECTOR;
        img.extend(noise(pad + 3 * SECTOR, 2000 + i as u64));
        at.push((off as u64, f.len() as u64));
    }
    (img, at)
}

fn carve_all(img: Vec<u8>) -> Vec<Carved> {
    let src = MemSource::new(img, 512);
    let mut out = Vec::new();
    carve(
        &src,
        0..src_len(&src),
        512,
        &CancelToken::new(),
        &|_| false,
        |_| {},
        |c| {
            out.push(c);
            Ok(())
        },
    )
    .unwrap();
    out
}

fn src_len(s: &MemSource) -> u64 {
    khodz_io::BlockSource::len(s)
}

#[test]
fn finds_every_format_with_exact_bounds_and_honest_status() {
    let files = vec![
        jpeg(30_000, 1),
        png(20_000, 2),
        gif(5_000, 3),
        zip(&[("a.txt", 3000), ("b.bin", 9000)], 4),
        zip(&[("[Content_Types].xml", 500), ("word/document.xml", 4000)], 5),
        pdf(10_000),
        mp4(20_000),
        bmp(),
        wav(4000),
    ];
    let expect = [
        (FileKind::Jpeg, CarveStatus::Complete),
        (FileKind::Png, CarveStatus::Complete),
        (FileKind::Gif, CarveStatus::Complete),
        (FileKind::Zip, CarveStatus::Complete),
        (FileKind::Docx, CarveStatus::Complete),
        (FileKind::Pdf, CarveStatus::Complete),
        (FileKind::Mp4, CarveStatus::Unverified),
        (FileKind::Bmp, CarveStatus::Unverified),
        (FileKind::Wav, CarveStatus::Unverified),
    ];
    let (img, at) = layout(&files);
    let found = carve_all(img);
    for ((off, len), (kind, status)) in at.iter().zip(expect) {
        let c = found
            .iter()
            .find(|c| c.offset == *off)
            .unwrap_or_else(|| panic!("{kind:?} at {off} not found: {found:#?}"));
        assert_eq!((c.kind, c.status), (kind, status), "{c:?}");
        assert_eq!(c.len, *len, "{kind:?} length");
    }
    assert_eq!(found.len(), files.len(), "no spurious hits in noise: {found:#?}");
}

/// The core regression: a file whose second half is somewhere else must never be "Complete".
#[test]
fn fragmented_files_are_never_reported_complete() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("jpeg", jpeg(60_000, 21)),
        ("png", png(60_000, 22)),
        ("gif", gif(60_000, 23)),
        ("zip", zip(&[("a.bin", 30_000), ("b.bin", 30_000)], 24)),
        ("pdf", pdf(60_000)),
    ];
    for (name, file) in &cases {
        for (gap_kind, gap) in
            [("noise", noise(8 * SECTOR, 99)), ("zeros", vec![0u8; 8 * SECTOR]), ("other file", png(4000, 7))]
        {
            // Split at a sector boundary roughly in the middle and insert the gap.
            let cut = (file.len() / 2) / SECTOR * SECTOR;
            let mut frag = file[..cut].to_vec();
            frag.extend_from_slice(&gap);
            frag.extend_from_slice(&file[cut..]);
            let mut img = frag;
            img.extend(noise(4 * SECTOR, 5));
            let found = carve_all(img);
            let first =
                found.iter().find(|c| c.offset == 0).unwrap_or_else(|| panic!("{name}: header not found"));
            assert_ne!(
                first.status,
                CarveStatus::Complete,
                "{name} with {gap_kind} gap falsely reported complete: {first:?}"
            );
            assert!(
                first.len <= (file.len() + gap.len()) as u64,
                "{name}/{gap_kind}: length beyond the data present"
            );
        }
    }
}

#[test]
fn truncated_files_are_partial() {
    for (name, file) in
        [("jpeg", jpeg(20_000, 31)), ("png", png(20_000, 32)), ("zip", zip(&[("x", 20_000)], 33))]
    {
        let cut = file.len() * 2 / 3;
        let found = carve_all(file[..cut].to_vec());
        let c = &found[0];
        assert_eq!(c.status, CarveStatus::Partial, "{name}: {c:?}");
        assert!(c.len > 0 && c.len <= cut as u64);
    }
}

#[test]
fn corrupted_png_crc_is_detected() {
    let mut f = png(20_000, 41);
    let mid = f.len() / 2;
    f[mid] ^= 0x55;
    let c = &carve_all(f)[0];
    assert_eq!(c.status, CarveStatus::Partial);
    assert!(c.note.as_deref().unwrap_or("").contains("CRC"));
}

#[test]
fn headers_in_noise_are_rejected_not_reported() {
    // Valid-looking magic followed by garbage must not produce files.
    let mut img = noise(64 * SECTOR, 50);
    for (i, magic) in
        [&b"\x89PNG\r\n\x1a\n"[..], b"PK\x03\x04", b"GIF89a", b"RIFFxxxxJUNK", b"\0\0\0\x08ftypisom"]
            .iter()
            .enumerate()
    {
        let o = (i + 1) * 4 * SECTOR;
        img[o..o + magic.len()].copy_from_slice(magic);
    }
    let found = carve_all(img);
    assert!(found.iter().all(|c| c.status != CarveStatus::Complete), "{found:#?}");
}

#[test]
fn skip_callback_and_cancellation() {
    let (img, at) = layout(&[png(5000, 1), png(5000, 2)]);
    let src = MemSource::new(img, 512);
    let first = at[0].0;
    let mut found = Vec::new();
    carve(
        &src,
        0..src_len(&src),
        512,
        &CancelToken::new(),
        &|o| o == first,
        |_| {},
        |c| {
            found.push(c.offset);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(found, vec![at[1].0], "offsets explained by filesystem metadata are skipped");

    let cancel = CancelToken::new();
    cancel.cancel();
    let r = carve(&src, 0..src_len(&src), 512, &cancel, &|_| false, |_| {}, |_| Ok(()));
    assert!(matches!(r, Err(khodz_core::Error::Cancelled)));
}

#[test]
fn hostile_sizes_do_not_exhaust_memory_or_panic() {
    // ZIP entry claiming a 4 GiB compressed size, BMP claiming 4 GiB, PNG chunk of 2^31-1.
    let mut z = b"PK\x03\x04".to_vec();
    z.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    z.extend_from_slice(&0xFFFF_FFFEu32.to_le_bytes());
    z.extend_from_slice(&[0u8; 8]);
    let mut b = b"BM".to_vec();
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    b.extend_from_slice(&[0u8; 50]);
    let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
    p.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
    p.extend_from_slice(b"IHDR");
    let (img, _) = layout(&[z, b, p]);
    let found = carve_all(img);
    assert!(found.iter().all(|c| c.status == CarveStatus::Partial || c.len < 1 << 20), "{found:#?}");
}
