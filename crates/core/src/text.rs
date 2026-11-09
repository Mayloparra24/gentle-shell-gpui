//! One-line text for summaries, titles and chips.

/// The first line of `text`, cut to `max_chars` with `…` when it is longer
/// or when more lines follow.
pub fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() > max_chars {
        let cut: String = line.chars().take(max_chars).collect();
        format!("{cut}…")
    } else if text.lines().nth(1).is_some() {
        format!("{line} …")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_marks_truncation() {
        assert_eq!(first_line("abcdef", 3), "abc…");
        assert_eq!(first_line("ab\ncd", 10), "ab …");
        assert_eq!(first_line("ab", 10), "ab");
    }
}
