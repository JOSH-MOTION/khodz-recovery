use std::fmt;

/// A byte count with human-readable (IEC, base-1024) formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteSize(pub u64);

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
        const LAST: usize = 6;
        let mut value = self.0 as f64;
        let mut unit = 0usize;
        while value >= 1024.0 && unit < LAST {
            value /= 1024.0;
            unit = unit.saturating_add(1);
        }
        if unit == 0 { write!(f, "{} B", self.0) } else { write!(f, "{value:.1} {}", UNITS[unit]) }
    }
}

/// Round `value` down to a multiple of `align`. Returns `None` if `align` is zero.
pub fn align_down(value: u64, align: u64) -> Option<u64> {
    // rem < align and rem <= value, so the subtraction cannot underflow.
    value.checked_rem(align).map(|rem| value.saturating_sub(rem))
}

/// Round `value` up to a multiple of `align`. Returns `None` on zero alignment or overflow.
pub fn align_up(value: u64, align: u64) -> Option<u64> {
    let rem = value.checked_rem(align)?;
    if rem == 0 { Some(value) } else { value.checked_add(align.saturating_sub(rem)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(ByteSize(0).to_string(), "0 B");
        assert_eq!(ByteSize(1023).to_string(), "1023 B");
        assert_eq!(ByteSize(1024).to_string(), "1.0 KiB");
        assert_eq!(ByteSize(1536).to_string(), "1.5 KiB");
        assert_eq!(ByteSize(500 * 1024 * 1024 * 1024).to_string(), "500.0 GiB");
        assert_eq!(ByteSize(u64::MAX).to_string(), "16.0 EiB");
    }

    #[test]
    fn aligns_down() {
        assert_eq!(align_down(0, 512), Some(0));
        assert_eq!(align_down(511, 512), Some(0));
        assert_eq!(align_down(4097, 4096), Some(4096));
        assert_eq!(align_down(10, 0), None);
    }

    #[test]
    fn aligns_up() {
        assert_eq!(align_up(0, 512), Some(0));
        assert_eq!(align_up(1, 512), Some(512));
        assert_eq!(align_up(4096, 4096), Some(4096));
        assert_eq!(align_up(10, 0), None);
        assert_eq!(align_up(u64::MAX, 4096), None, "must not overflow");
    }
}
