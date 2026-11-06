//! Process-wide caches of values derived from files, valid while the file's
//! modification time and length are unchanged. Callers stat the file
//! *before* reading it, so a write that lands mid-read leaves the entry keyed
//! by the old stamp and the next lookup reads again.

use std::collections::BTreeMap;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

/// What identifies one version of a file's contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    /// Follows symlinks; `None` when `path` cannot be stat'ed.
    pub fn of(path: &Path) -> Option<Self> {
        std::fs::metadata(path)
            .ok()
            .map(|meta| Self::from_metadata(&meta))
    }

    pub fn from_metadata(meta: &Metadata) -> Self {
        Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        }
    }
}

/// Values per path, each valid for one [`Stamp`]. At most `capacity` entries;
/// inserting past it drops an arbitrary old one.
pub struct FileCache<V> {
    entries: Mutex<BTreeMap<PathBuf, (Stamp, V)>>,
    capacity: usize,
}

impl<V: Clone> FileCache<V> {
    pub const fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(BTreeMap::new()),
            capacity,
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<PathBuf, (Stamp, V)>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The value cached for `path` at exactly `stamp`.
    pub fn get(&self, path: &Path, stamp: Stamp) -> Option<V> {
        self.lock()
            .get(path)
            .filter(|(cached, _)| *cached == stamp)
            .map(|(_, value)| value.clone())
    }

    pub fn insert(&self, path: &Path, stamp: Stamp, value: V) {
        let mut entries = self.lock();
        if let Some(entry) = entries.get_mut(path) {
            *entry = (stamp, value);
            return;
        }
        while entries.len() >= self.capacity.max(1) {
            entries.pop_first();
        }
        entries.insert(path.to_path_buf(), (stamp, value));
    }

    /// The cached value at `stamp`, else `load()` (run without the lock
    /// held), cached.
    pub fn get_or_insert_with(&self, path: &Path, stamp: Stamp, load: impl FnOnce() -> V) -> V {
        if let Some(value) = self.get(path, stamp) {
            return value;
        }
        let value = load();
        self.insert(path, stamp, value.clone());
        value
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::time::Duration;

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("pi-rpc-file-cache-{}-{name}", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn set_mtime(path: &Path, secs: u64) {
        let file = File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn hit_while_stamp_unchanged_miss_after_change() {
        let path = temp_file("stamp", "one");
        set_mtime(&path, 1_000_000);
        let cache: FileCache<String> = FileCache::new(8);
        let read = || std::fs::read_to_string(&path).unwrap();
        let stamp = Stamp::of(&path).unwrap();
        assert_eq!(cache.get_or_insert_with(&path, stamp, read), "one");

        // Same length and mtime: the stale cached value is served.
        std::fs::write(&path, "two").unwrap();
        set_mtime(&path, 1_000_000);
        let stamp = Stamp::of(&path).unwrap();
        assert_eq!(cache.get_or_insert_with(&path, stamp, read), "one");

        // A new mtime re-reads.
        set_mtime(&path, 1_000_001);
        let stamp = Stamp::of(&path).unwrap();
        assert_eq!(cache.get_or_insert_with(&path, stamp, read), "two");

        // So does a new length at the same mtime.
        std::fs::write(&path, "three").unwrap();
        set_mtime(&path, 1_000_001);
        let stamp = Stamp::of(&path).unwrap();
        assert_eq!(cache.get_or_insert_with(&path, stamp, read), "three");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn capacity_is_bounded() {
        let cache: FileCache<u32> = FileCache::new(2);
        let stamp = Stamp {
            modified: None,
            len: 0,
        };
        for (i, name) in ["a", "b", "c"].into_iter().enumerate() {
            cache.insert(Path::new(name), stamp, i as u32);
        }
        assert_eq!(cache.lock().len(), 2);
        assert_eq!(cache.get(Path::new("c"), stamp), Some(2));
    }
}
