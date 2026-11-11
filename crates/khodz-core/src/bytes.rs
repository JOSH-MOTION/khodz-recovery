//! Bounds-checked little/big-endian readers for on-disk structures. Every accessor returns
//! `None` instead of panicking when the requested bytes are outside the buffer, so parsers of
//! untrusted media cannot be crashed by truncated or malicious structures.

macro_rules! reader {
    ($name:ident, $t:ty, $from:ident) => {
        #[inline]
        pub fn $name(buf: &[u8], off: usize) -> Option<$t> {
            const N: usize = std::mem::size_of::<$t>();
            let bytes = buf.get(off..off.checked_add(N)?)?;
            Some(<$t>::$from(bytes.try_into().ok()?))
        }
    };
}

reader!(le_u16, u16, from_le_bytes);
reader!(le_u32, u32, from_le_bytes);
reader!(le_u64, u64, from_le_bytes);
reader!(le_i64, i64, from_le_bytes);
reader!(be_u16, u16, from_be_bytes);
reader!(be_u32, u32, from_be_bytes);
reader!(be_u64, u64, from_be_bytes);

#[inline]
pub fn u8_at(buf: &[u8], off: usize) -> Option<u8> {
    buf.get(off).copied()
}

/// `len` bytes at `off`.
#[inline]
pub fn slice(buf: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    buf.get(off..off.checked_add(len)?)
}

/// Decode UTF-16LE, stopping at the first NUL. Invalid surrogates become U+FFFD.
pub fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> =
        bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Decode UTF-16BE (HFS+ names).
pub fn utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_are_bounds_checked() {
        let b = [1u8, 2, 3, 4, 5, 6, 7, 8, 9];
        assert_eq!(le_u16(&b, 0), Some(0x0201));
        assert_eq!(be_u16(&b, 0), Some(0x0102));
        assert_eq!(le_u32(&b, 5), Some(0x0908_0706));
        assert_eq!(le_u32(&b, 6), None);
        assert_eq!(le_u64(&b, 1), Some(0x0908_0706_0504_0302));
        assert_eq!(le_u64(&b, 2), None);
        assert_eq!(le_u16(&b, usize::MAX), None, "no overflow panic");
        assert_eq!(slice(&b, 7, 3), None);
        assert_eq!(u8_at(&b, 9), None);
    }

    #[test]
    fn utf16_decoding() {
        let s: Vec<u8> =
            "café 日本".encode_utf16().flat_map(|u| u.to_le_bytes()).chain([0, 0, b'x', 0]).collect();
        assert_eq!(utf16le(&s), "café 日本");
        let be: Vec<u8> = "Mac".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        assert_eq!(utf16be(&be), "Mac");
    }
}
