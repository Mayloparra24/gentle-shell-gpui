//! The right rail's and file tree's change list: porcelain v1 status and
//! numstat, with untracked files' line counts read from disk and cached.

use std::path::Path;

use gentle_core::git::status::{GitSnapshot, parse_numstat, parse_status};
use pi_rpc::file_cache::{FileCache, Stamp};

use super::git;

/// `None` when `cwd` is not inside a git work tree.
pub fn snapshot(cwd: &Path) -> Option<GitSnapshot> {
    let status = git(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    // `symbolic-ref` also names the branch of a repository without commits;
    // it fails only on a detached HEAD.
    let branch = git(cwd, &["symbolic-ref", "--short", "-q", "HEAD"])
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty());
    // Staged + unstaged against HEAD; a repository without commits has no HEAD.
    let numstat = git(cwd, &["diff", "--numstat", "-z", "HEAD"])
        .or_else(|| git(cwd, &["diff", "--numstat", "-z"]))
        .unwrap_or_default();
    let mut files = parse_status(&status);
    let stats = parse_numstat(&numstat);
    for file in &mut files {
        if let Some(&(added, removed)) = stats.get(&file.path) {
            (file.added, file.removed) = (added, removed);
        } else if file.status == '?' {
            file.added = count_lines(&cwd.join(&file.path));
            file.removed = Some(0);
        }
    }
    Some(GitSnapshot { branch, files })
}

/// Untracked files' line counts, so each poll only reads files that changed.
static LINE_COUNTS: FileCache<Option<u64>> = FileCache::new(16_384);

fn count_lines(path: &Path) -> Option<u64> {
    count_lines_in(&LINE_COUNTS, path)
}

fn count_lines_in(cache: &FileCache<Option<u64>>, path: &Path) -> Option<u64> {
    const LIMIT: u64 = 4 * 1024 * 1024;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > LIMIT {
        return None;
    }
    let stamp = Stamp::from_metadata(&meta);
    if let Some(count) = cache.get(path, stamp) {
        return count;
    }
    // An unreadable file is not cached: it may become readable unchanged.
    let bytes = std::fs::read(path).ok()?;
    let count = (!bytes.contains(&0)).then(|| {
        let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
        newlines + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))
    });
    cache.insert(path, stamp, count);
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untracked_line_counts_are_cached_by_mtime_and_len() {
        use std::time::{Duration, SystemTime};
        let path = std::env::temp_dir().join(format!("gentle-git-lines-{}", std::process::id()));
        let set_mtime = |secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        let cache = FileCache::new(8);
        std::fs::write(&path, "a\nb\nc").unwrap();
        set_mtime(5_000_000);
        assert_eq!(count_lines_in(&cache, &path), Some(3));

        // Same mtime and length: served from the cache, not re-read.
        std::fs::write(&path, "abcd\n").unwrap();
        set_mtime(5_000_000);
        assert_eq!(count_lines_in(&cache, &path), Some(3));

        set_mtime(5_000_001);
        assert_eq!(count_lines_in(&cache, &path), Some(1));

        std::fs::write(&path, "a\0b\n").unwrap();
        assert_eq!(count_lines_in(&cache, &path), None);
        std::fs::remove_file(&path).unwrap();
    }
}
