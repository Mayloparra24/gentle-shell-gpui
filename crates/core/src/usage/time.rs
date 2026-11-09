//! Time helpers (no date crate: ISO-8601 UTC strings and civil-day math).

pub(super) const MINUTE: i64 = 60;
pub(super) const HOUR: i64 = 3_600;
pub const DAY: i64 = 86_400;
pub(super) const WEEK: i64 = 604_800;

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if month <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        month,
        day,
    )
}

/// `YYYY-MM-DD[THH:MM[:SS[.fff]]][Z|±HH:MM|±HHMM]` → Unix milliseconds.
/// A missing offset is read as UTC (Pi always writes `toISOString()`).
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    let s = text.trim().as_bytes();
    let digits = |from: usize, len: usize| -> Option<i64> {
        let slice = s.get(from..from + len)?;
        slice
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| slice.iter().fold(0i64, |n, d| n * 10 + (d - b'0') as i64))
    };
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    if s.get(4) != Some(&b'-')
        || s.get(7) != Some(&b'-')
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
    {
        return None;
    }
    let mut ms = days_from_civil(year, month, day) * DAY * 1000;
    if s.len() == 10 {
        return Some(ms);
    }
    if !matches!(s.get(10), Some(b'T' | b't' | b' ')) || s.get(13) != Some(&b':') {
        return None;
    }
    let (hour, minute) = (digits(11, 2)?, digits(14, 2)?);
    let mut at = 16;
    let mut second = 0;
    if s.get(at) == Some(&b':') {
        second = digits(17, 2)?;
        at = 19;
    }
    let mut frac_ms = 0i64;
    if s.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while s.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return None;
        }
        let padded: String = std::str::from_utf8(&s[start..at])
            .ok()?
            .chars()
            .chain("000".chars())
            .take(3)
            .collect();
        frac_ms = padded.parse().ok()?;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    ms += ((hour * 60 + minute) * 60 + second) * 1000 + frac_ms;
    let offset_s = match s.get(at) {
        None => 0,
        Some(b'Z' | b'z') if at + 1 == s.len() => 0,
        Some(sign @ (b'+' | b'-')) => {
            let h = digits(at + 1, 2)?;
            let (m, end) = if s.get(at + 3) == Some(&b':') {
                (digits(at + 4, 2)?, at + 6)
            } else {
                (digits(at + 3, 2)?, at + 5)
            };
            if end != s.len() {
                return None;
            }
            let total = (h * 60 + m) * 60;
            if *sign == b'+' { total } else { -total }
        }
        _ => return None,
    };
    Some(ms - offset_s * 1000)
}

pub(super) fn local_day(ms: i64, offset_secs: i64) -> i64 {
    (ms.div_euclid(1000) + offset_secs).div_euclid(DAY)
}

pub fn day_label(day: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, month, day) = civil_from_days(day);
    format!("{} {day}", MONTHS[(month - 1) as usize])
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, days_from_civil, parse_iso_ms};
    use crate::usage::subscription::{format_reset, window_label};

    const NOW: i64 = 1_762_400_000_000;

    #[test]
    fn iso_timestamps_and_resets() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_ms("2025-11-06T12:34:56.789Z"),
            Some(1_762_432_496_789)
        );
        assert_eq!(
            parse_iso_ms("2025-11-06T14:34:56.789+02:00"),
            Some(1_762_432_496_789)
        );
        assert_eq!(parse_iso_ms("2025-11-06"), Some(1_762_387_200_000));
        assert_eq!(parse_iso_ms("nope"), None);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(format_reset(Some(NOW + 30 * 60_000), NOW), "resets in 30m");
        assert_eq!(
            format_reset(Some(NOW + (2 * 3600 + 5 * 60) * 1000), NOW),
            "resets in 2h 5m"
        );
        assert_eq!(
            format_reset(Some(NOW + (3 * 86400 + 4 * 3600) * 1000), NOW),
            "resets in 3d 4h"
        );
        assert_eq!(format_reset(Some(NOW - 1), NOW), "resets now");
        assert_eq!(window_label(18_000), "5h");
        assert_eq!(window_label(604_800), "week");
    }
}
