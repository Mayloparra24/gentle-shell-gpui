//! State transitions of the notice list: record (deduping repeats), read
//! marks, clear and iteration.

use std::time::Instant;

use gentle_core::transcript::NoticeLevel;
use gpui_kit::SharedString;

use super::{CAP, Notice, Notices};

impl Notices {
    /// Record a notice. A message already listed at the same level is moved to
    /// the top and counted instead of listed twice; it keeps its read state so
    /// startup chatter repeated by every new tab does not light the bell again.
    pub fn push(
        &mut self,
        level: NoticeLevel,
        message: String,
        source: SharedString,
        origin: Option<usize>,
        at: Instant,
    ) {
        let notice = match self
            .items
            .iter()
            .position(|n| n.level == level && n.message == message)
            .and_then(|ix| self.items.remove(ix))
        {
            Some(mut old) => {
                old.repeats += 1;
                old.at = at;
                old.source = source;
                old.origin = origin;
                old
            }
            None => {
                self.next_id += 1;
                Notice {
                    id: self.next_id,
                    level,
                    message,
                    source,
                    origin,
                    at,
                    read: false,
                    repeats: 0,
                }
            }
        };
        self.items.push_front(notice);
        self.items.truncate(CAP);
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }

    pub fn mark_read(&mut self, id: u64) {
        if let Some(notice) = self.items.iter_mut().find(|n| n.id == id) {
            notice.read = true;
        }
    }

    pub fn mark_all_read(&mut self) {
        for notice in &mut self.items {
            notice.read = true;
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = &Notice> {
        self.items.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::features::notices::{CAP, Notices};
    use gentle_core::transcript::NoticeLevel;

    fn push(store: &mut Notices, message: &str, at: Instant) {
        store.push(
            NoticeLevel::Info,
            message.to_string(),
            "tab".into(),
            Some(1),
            at,
        );
    }

    #[test]
    fn dedupes_repeats_to_the_top() {
        let mut store = Notices::default();
        let t0 = Instant::now();
        push(&mut store, "a", t0);
        push(&mut store, "b", t0);
        push(&mut store, "a", t0 + Duration::from_secs(5));
        assert_eq!(store.iter().count(), 2);
        let first = store.iter().next().unwrap();
        assert_eq!(first.message, "a");
        assert_eq!(first.repeats, 1);
        assert_eq!(first.at, t0 + Duration::from_secs(5));
        // Same text at another level is a different notice.
        store.push(NoticeLevel::Error, "a".into(), "tab".into(), None, t0);
        assert_eq!(store.iter().count(), 3);
    }

    #[test]
    fn counts_unread() {
        let mut store = Notices::default();
        let t0 = Instant::now();
        push(&mut store, "a", t0);
        push(&mut store, "b", t0);
        assert_eq!(store.unread(), 2);
        let id = store.iter().next().unwrap().id;
        store.mark_read(id);
        assert_eq!(store.unread(), 1);
        store.mark_all_read();
        assert_eq!(store.unread(), 0);
        // A repeat of a read notice stays read.
        push(&mut store, "a", t0);
        assert_eq!(store.unread(), 0);
        push(&mut store, "c", t0);
        assert_eq!(store.unread(), 1);
        store.clear();
        assert!(store.is_empty());
        assert_eq!(store.unread(), 0);
    }

    #[test]
    fn caps_at_200_dropping_oldest() {
        let mut store = Notices::default();
        let t0 = Instant::now();
        for i in 0..CAP + 25 {
            push(&mut store, &format!("n{i}"), t0);
        }
        assert_eq!(CAP, 200);
        assert_eq!(store.iter().count(), CAP);
        assert_eq!(
            store.iter().next().unwrap().message,
            format!("n{}", CAP + 24)
        );
        assert_eq!(store.iter().last().unwrap().message, "n25");
        assert_eq!(store.unread(), CAP);
    }
}
