use std::fmt;

use serde::{Serialize, Serializer};

/// A GUID as stored on disk (mixed-endian: first three fields little-endian).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub fn from_slice(b: &[u8]) -> Option<Self> {
        Some(Guid(b.get(..16)?.try_into().ok()?))
    }

    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }

    /// Parse the canonical text form `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`.
    pub fn parse(s: &str) -> Option<Self> {
        let hex: String = s.chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 {
            return None;
        }
        let mut v = [0u8; 16];
        for (i, slot) in v.iter_mut().enumerate() {
            let pair = hex.get(i.checked_mul(2)?..i.checked_mul(2)?.checked_add(2)?)?;
            *slot = u8::from_str_radix(pair, 16).ok()?;
        }
        // Text order → on-disk order: swap the first three fields to little-endian.
        v[0..4].reverse();
        v[4..6].reverse();
        v[6..8].reverse();
        Some(Guid(v))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
            b[3],
            b[2],
            b[1],
            b[0],
            b[5],
            b[4],
            b[7],
            b[6],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for Guid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_canonical_form() {
        let text = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";
        let g = Guid::parse(text).unwrap();
        assert_eq!(g.0[0], 0xA2, "first field stored little-endian");
        assert_eq!(g.to_string(), text);
        assert_eq!(Guid::parse("ebd0a0a2b9e5443387c068b6b72699c7"), Some(g));
        assert_eq!(Guid::parse("nope"), None);
    }
}
