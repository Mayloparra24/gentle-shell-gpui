//! Session browser model: the summary shown for one Pi session file (title,
//! search match, ordering), Pi's per-cwd session directory name, and the
//! timestamp parsing/formatting the summaries use.

use std::path::{Path, PathBuf};

/// What the browser shows for one session file.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub path: PathBuf,
    pub id: String,
    /// The header's `cwd`; empty when the header has none.
    pub cwd: PathBuf,
    /// Latest `session_info` name (`/name`, `set_session_name`); a later empty
    /// name clears it, as in Pi.
    pub name: Option<String>,
    /// Text of the first user message.
    pub first_message: Option<String>,
    /// Last user/assistant activity, else header time, else file mtime (ms).
    pub modified_ms: i64,
    /// Number of `message` entries (Pi counts every role).
    pub message_count: usize,
    /// The scan stopped at [`MAX_SCAN_BYTES`]; counts are a lower bound.
    pub truncated: bool,
    /// Model of the last assistant reply or `model_change`, whichever is later.
    pub model: Option<String>,
}

impl SessionSummary {
    pub fn title(&self) -> String {
        let text = self
            .name
            .as_deref()
            .or(self.first_message.as_deref())
            .unwrap_or("(no messages)");
        crate::text::first_line(text.trim(), 120)
    }

    pub fn matches(&self, query_lower: &str) -> bool {
        query_lower.is_empty()
            || [self.name.as_deref(), self.first_message.as_deref()]
                .into_iter()
                .flatten()
                .any(|t| t.to_lowercase().contains(query_lower))
            || self
                .cwd
                .to_string_lossy()
                .to_lowercase()
                .contains(query_lower)
            || self
                .path
                .to_string_lossy()
                .to_lowercase()
                .contains(query_lower)
    }
}

/// Stop reading a session after this many bytes; huge transcripts (inline
/// images, long tool output) would otherwise stall the list.
pub const MAX_SCAN_BYTES: u64 = 16 * 1024 * 1024;

/// Pi's per-cwd directory name: drop the leading separator, map `/ \ :` to `-`.
pub fn session_dir_name(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let trimmed = s.strip_prefix(['/', '\\']).unwrap_or(&s);
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{safe}--")
}

pub fn sort_newest_first(sessions: &mut [SessionSummary]) {
    sessions.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| b.path.cmp(&a.path))
    });
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]` to Unix milliseconds.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    let mut i = 19;
    let mut ms = 0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let frac = s.get(start..i)?;
        let padded: String = frac.chars().chain("000".chars()).take(3).collect();
        ms = padded.parse::<i64>().ok()?;
    }
    let offset_min = match b.get(i) {
        None | Some(b'Z' | b'z') => 0,
        Some(sign @ (b'+' | b'-')) => {
            let oh = num(i + 1..i + 3)?;
            let om = match b.get(i + 3) {
                Some(b':') => num(i + 4..i + 6)?,
                _ => num(i + 3..i + 5).unwrap_or(0),
            };
            let total = oh * 60 + om;
            if *sign == b'+' { total } else { -total }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo, d);
    Some(((days * 86_400 + h * 3600 + mi * 60 + sec - offset_min * 60) * 1000) + ms)
}

pub fn relative_time(now_ms: i64, then_ms: i64) -> String {
    let secs = (now_ms - then_ms).max(0) / 1000;
    match secs {
        0..60 => "just now".to_string(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        86_400..604_800 => format!("{}d ago", secs / 86_400),
        604_800..2_592_000 => format!("{}w ago", secs / 604_800),
        2_592_000..31_536_000 => format!("{}mo ago", secs / 2_592_000),
        _ => format!("{}y ago", secs / 31_536_000),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{parse_iso_ms, relative_time, session_dir_name};

    #[test]
    fn encodes_cwd_like_pi() {
        assert_eq!(
            session_dir_name(Path::new(
                "/Users/ismaelosuna/Documents/Git/personal/gentle-shell-gpui"
            )),
            "--Users-ismaelosuna-Documents-Git-personal-gentle-shell-gpui--"
        );
        assert_eq!(session_dir_name(Path::new("C:\\work\\x")), "--C--work-x--");
    }

    #[test]
    fn parses_iso_timestamps() {
        assert_eq!(
            parse_iso_ms("2024-12-03T14:00:00.000Z"),
            Some(1_733_234_400_000)
        );
        assert_eq!(
            parse_iso_ms("2024-12-03T14:00:00Z"),
            Some(1_733_234_400_000)
        );
        assert_eq!(
            parse_iso_ms("2024-12-03T14:00:00.5Z"),
            Some(1_733_234_400_500)
        );
        assert_eq!(
            parse_iso_ms("2024-12-03T16:00:00+02:00"),
            Some(1_733_234_400_000)
        );
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso_ms("yesterday"), None);
    }

    #[test]
    fn formats_relative_time() {
        let now = 1_000_000_000_000;
        assert_eq!(relative_time(now, now + 5_000), "just now");
        assert_eq!(relative_time(now, now - 30_000), "just now");
        assert_eq!(relative_time(now, now - 5 * 60_000), "5m ago");
        assert_eq!(relative_time(now, now - 3 * 3_600_000), "3h ago");
        assert_eq!(relative_time(now, now - 2 * 86_400_000), "2d ago");
        assert_eq!(relative_time(now, now - 14 * 86_400_000), "2w ago");
        assert_eq!(relative_time(now, now - 400 * 86_400_000), "1y ago");
    }
}
