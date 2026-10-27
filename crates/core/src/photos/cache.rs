//! The thumbnails the desktop fetched, in memory and within a bound.

use std::collections::HashMap;

use super::Thumbnail;

/// One thumbnail: of the photo with this id and version, at most this
/// large.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ThumbnailKey {
    pub(crate) id: String,
    pub(crate) modified_unix_ms: i64,
    pub(crate) max_size: u32,
}

/// Thumbnails by [`ThumbnailKey`], holding at most `limit` bytes of image
/// data. The one used longest ago goes first to make room.
#[derive(Debug)]
pub(crate) struct ThumbnailCache {
    entries: HashMap<ThumbnailKey, Entry>,
    bytes: usize,
    limit: usize,
    /// Counts uses, to tell which entry was used longest ago.
    clock: u64,
}

#[derive(Debug)]
struct Entry {
    thumbnail: Thumbnail,
    used: u64,
}

impl ThumbnailCache {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            limit,
            clock: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &ThumbnailKey) -> Option<Thumbnail> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.used = self.clock;
        Some(entry.thumbnail.clone())
    }

    pub(crate) fn insert(&mut self, key: ThumbnailKey, thumbnail: Thumbnail) {
        let size = thumbnail.data.len();
        if size > self.limit {
            return;
        }
        self.remove(&key);
        while self.bytes + size > self.limit {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.remove(&oldest);
        }
        self.clock += 1;
        self.bytes += size;
        self.entries.insert(
            key,
            Entry {
                thumbnail,
                used: self.clock,
            },
        );
    }

    /// Keeps only the thumbnails `keep` accepts.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&ThumbnailKey) -> bool) {
        let bytes = &mut self.bytes;
        self.entries.retain(|key, entry| {
            let kept = keep(key);
            if !kept {
                *bytes -= entry.thumbnail.data.len();
            }
            kept
        });
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    fn remove(&mut self, key: &ThumbnailKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes -= entry.thumbnail.data.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> ThumbnailKey {
        ThumbnailKey {
            id: id.into(),
            modified_unix_ms: 1,
            max_size: 256,
        }
    }

    fn thumbnail(bytes: usize) -> Thumbnail {
        Thumbnail {
            mime_type: "image/jpeg",
            data: vec![0; bytes].into(),
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn the_thumbnail_used_longest_ago_makes_room() {
        let mut cache = ThumbnailCache::new(30);
        cache.insert(key("a"), thumbnail(10));
        cache.insert(key("b"), thumbnail(10));
        cache.insert(key("c"), thumbnail(10));
        assert!(cache.get(&key("a")).is_some());

        cache.insert(key("d"), thumbnail(10));

        assert!(cache.get(&key("b")).is_none());
        assert!(cache.get(&key("a")).is_some());
        assert!(cache.get(&key("c")).is_some());
        assert!(cache.get(&key("d")).is_some());
        assert_eq!(cache.bytes(), 30);
    }

    #[test]
    fn a_thumbnail_larger_than_the_cache_is_not_kept() {
        let mut cache = ThumbnailCache::new(30);
        cache.insert(key("a"), thumbnail(10));

        cache.insert(key("b"), thumbnail(31));

        assert!(cache.get(&key("b")).is_none());
        assert!(cache.get(&key("a")).is_some());
    }

    #[test]
    fn retaining_and_replacing_keep_the_count_right() {
        let mut cache = ThumbnailCache::new(100);
        cache.insert(key("a"), thumbnail(10));
        cache.insert(key("a"), thumbnail(20));
        cache.insert(key("b"), thumbnail(5));
        assert_eq!(cache.bytes(), 25);

        cache.retain(|key| key.id == "b");

        assert_eq!(cache.bytes(), 5);
        cache.clear();
        assert_eq!(cache.bytes(), 0);
    }
}
