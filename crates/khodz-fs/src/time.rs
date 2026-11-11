//! Timestamp conversion. All results are milliseconds since the Unix epoch.

/// DOS date/time (FAT, exFAT): date bits 0-4 day, 5-8 month, 9-15 year-1980; time bits 0-4
/// seconds/2, 5-10 minute, 11-15 hour. Returns None for invalid fields. FAT stores local time;
/// the value is returned as if it were UTC.
// Every operand comes from a 16-bit field (years ≤ 2107, hours ≤ 31), so i64 cannot overflow.
#[allow(clippy::arithmetic_side_effects)]
pub fn dos_to_ms(date: u16, time: u16) -> Option<i64> {
    let day = i64::from(date & 0x1F);
    let month = i64::from((date >> 5) & 0x0F);
    let year = 1980i64.checked_add(i64::from(date >> 9))?;
    let sec = i64::from(time & 0x1F).checked_mul(2)?;
    let min = i64::from((time >> 5) & 0x3F);
    let hour = i64::from(time >> 11);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    days.checked_mul(86_400)?.checked_add(hour * 3600 + min * 60 + sec)?.checked_mul(1000)
}

/// NTFS FILETIME (100 ns intervals since 1601-01-01 UTC).
pub fn filetime_to_ms(ft: u64) -> Option<i64> {
    if ft == 0 {
        return None;
    }
    i64::try_from(ft / 10_000).ok()?.checked_sub(11_644_473_600_000)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
// Only called with 1980 ≤ y ≤ 2107, 1 ≤ m ≤ 12, 1 ≤ d ≤ 31: intermediate values are tiny.
#[allow(clippy::arithmetic_side_effects)]
fn days_from_civil(y: i64, m: i64, d: i64) -> Option<i64> {
    let y = if m <= 2 { y.checked_sub(1)? } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era.checked_mul(146_097)?.checked_add(doe)?.checked_sub(719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_times() {
        // 2024-02-29 13:45:30
        let date = ((2024 - 1980) << 9) | (2 << 5) | 29;
        let time = (13 << 11) | (45 << 5) | 15;
        assert_eq!(dos_to_ms(date, time), Some(1_709_214_330_000));
        assert_eq!(dos_to_ms(0, 0), None, "month 0 is invalid");
    }

    #[test]
    fn filetimes() {
        assert_eq!(filetime_to_ms(116_444_736_000_000_000), Some(0));
        assert_eq!(filetime_to_ms(0), None);
    }
}
