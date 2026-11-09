//! Fuzzy matching for the switcher (⌘K): the prepared query, prepared file
//! paths, a case-insensitive subsequence score and the file ranking.

/// The trimmed query, lowercased once per edit.
#[derive(Default)]
pub struct Needle {
    pub text: String,
    pub lower: String,
    /// `lower` without leading slashes, for slash commands.
    pub command: String,
}

impl Needle {
    pub fn new(text: &str) -> Self {
        let text = text.trim().to_string();
        let lower = text.to_lowercase();
        let command = lower.trim_start_matches('/').to_string();
        Self {
            text,
            lower,
            command,
        }
    }
}

/// A file under the root, relative, lowercased once at load.
pub struct FileEntry {
    pub path: String,
    lower: String,
    /// Byte offset of the file name in `path`.
    pub name_at: usize,
    /// The lowercased name when it isn't `lower[name_at..]` (non-ASCII paths,
    /// whose lowercase can change byte lengths).
    name_lower: Option<String>,
}

impl FileEntry {
    pub fn new(path: String) -> Self {
        let name_at = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
        let (lower, name_lower) = if path.is_ascii() {
            (path.to_ascii_lowercase(), None)
        } else {
            (path.to_lowercase(), Some(path[name_at..].to_lowercase()))
        };
        Self {
            path,
            lower,
            name_at,
            name_lower,
        }
    }

    fn name_lower(&self) -> &str {
        self.name_lower
            .as_deref()
            .unwrap_or(&self.lower[self.name_at..])
    }
}

/// Case-insensitive subsequence score; lower is better, `None` = no match.
/// `query` must already be lowercase (see [`Needle`]); allocation-free for
/// ASCII `text`.
pub fn fuzzy_score(query: &str, text: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    if text.is_ascii() {
        ascii_score(query.as_bytes(), text.as_bytes())
    } else {
        lowered_score(query, &text.to_lowercase())
    }
}

/// [`fuzzy_score`] over ASCII `text`, folding its case byte by byte. A
/// non-ASCII byte in `query` never matches, as its lowercase char couldn't.
fn ascii_score(query: &[u8], text: &[u8]) -> Option<usize> {
    let folded = |a: &u8, q: &u8| a.to_ascii_lowercase() == *q;
    if query.len() <= text.len()
        && let Some(pos) = text
            .windows(query.len())
            .position(|w| w.iter().zip(query).all(|(a, q)| folded(a, q)))
    {
        return Some(pos);
    }
    let mut bytes = text.iter().enumerate();
    let mut last = 0;
    let mut gaps = 0;
    for q in query {
        let (i, _) = bytes.find(|(_, a)| folded(a, q))?;
        gaps += i - last;
        last = i;
    }
    Some(100 + gaps)
}

/// [`fuzzy_score`] with both sides already lowercase; allocation-free.
fn lowered_score(query: &str, text: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    if let Some(pos) = text.find(query) {
        return Some(pos);
    }
    let mut chars = text.char_indices();
    let mut last = 0;
    let mut gaps = 0;
    for q in query.chars() {
        let (i, _) = chars.find(|(_, c)| *c == q)?;
        gaps += i - last;
        last = i;
    }
    Some(100 + gaps)
}

/// Indices of the `files` matching `query` (lowercase), best first: name
/// matches before path-only ones, then shorter paths; at most `cap`.
pub fn rank_files(files: &[FileEntry], query: &str, cap: usize) -> Vec<usize> {
    if query.is_empty() {
        return (0..files.len().min(cap)).collect();
    }
    let mut scored: Vec<(usize, usize, usize)> = files
        .iter()
        .enumerate()
        .filter_map(|(i, file)| {
            let score = lowered_score(query, file.name_lower())
                .or_else(|| lowered_score(query, &file.lower).map(|s| s + 50))?;
            Some((score, file.path.len(), i))
        })
        .collect();
    scored.sort_unstable();
    scored.into_iter().take(cap).map(|(_, _, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::{FileEntry, Needle, fuzzy_score, rank_files};

    /// The scorer before it went allocation-free, kept to pin the ranking.
    fn reference_score(query: &str, text: &str) -> Option<usize> {
        if query.is_empty() {
            return Some(0);
        }
        let text = text.to_lowercase();
        if let Some(pos) = text.find(&query.to_lowercase()) {
            return Some(pos);
        }
        let mut chars = text.char_indices();
        let mut last = 0;
        let mut gaps = 0;
        for q in query.to_lowercase().chars() {
            let (i, _) = chars.find(|(_, c)| *c == q)?;
            gaps += i - last;
            last = i;
        }
        Some(100 + gaps)
    }

    /// The file ranking before rows were cached, on raw paths.
    fn reference_rank<'a>(files: &[&'a str], query: &str, cap: usize) -> Vec<&'a str> {
        let mut scored: Vec<(usize, &str)> = files
            .iter()
            .filter_map(|path| {
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                let score = reference_score(query, name)
                    .or_else(|| reference_score(query, path).map(|s| s + 50))?;
                Some((score, *path))
            })
            .collect();
        if !query.is_empty() {
            scored.sort_by_key(|(score, path)| (*score, path.len()));
        }
        scored.into_iter().take(cap).map(|(_, path)| path).collect()
    }

    #[test]
    fn prepared_scoring_ranks_like_the_reference() {
        let files = [
            "src/main.rs",
            "crates/app/src/palette.rs",
            "crates/app/src/files/changes.rs",
            "crates/app/src/files/refs.rs",
            "README.md",
            "Cargo.toml",
            "crates/app/Cargo.toml",
            "docs/Ünïcode/Straße.txt",
            "ΣΊΣΥΦΟΣ/notes.md",
            "assets/İstanbul.png",
            "a\\windows\\Path.TXT",
            "tests/palette_test.rs",
            "PAL/al/p.rs",
        ];
        let entries: Vec<FileEntry> = files
            .iter()
            .map(|p| FileEntry::new(p.to_string()))
            .collect();
        let queries = [
            "", "pal", "PAL", "cs", "rs", "Straße", "σ", "ΣΟΣ", "İ", "i", "ünï", "md", "cargo",
            "a/s/p", "path", "zzz", "c.r",
        ];
        for query in queries {
            let needle = Needle::new(query);
            for text in files {
                assert_eq!(
                    fuzzy_score(&needle.lower, text),
                    reference_score(query, text),
                    "{query:?} on {text:?}"
                );
            }
            for cap in [3, 60] {
                let ranked: Vec<&str> = rank_files(&entries, &needle.lower, cap)
                    .into_iter()
                    .map(|i| entries[i].path.as_str())
                    .collect();
                assert_eq!(ranked, reference_rank(&files, query, cap), "{query:?}");
            }
        }
    }

    #[test]
    fn fuzzy_prefers_substrings_then_tight_subsequences() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert_eq!(fuzzy_score("prof", "Profiles"), Some(0));
        assert!(fuzzy_score("gjb", "/gentle:jobs").unwrap() >= 100);
        assert!(
            fuzzy_score("gjb", "/gentle:jobs") < fuzzy_score("gjb", "/gentle:review-mode-jobs-x-b")
        );
        assert_eq!(fuzzy_score("zz", "Profiles"), None);
    }
}
