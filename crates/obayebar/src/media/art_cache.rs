//! In-memory cache of decoded covers, keyed by URL: the least recently used
//! one is evicted past capacity, and a URL already on its way is not
//! requested twice.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use lru::LruCache;

use crate::services::media_art::Art;

/// Covers kept decoded, so skipping back and forth does not refetch.
const CACHE_CAPACITY: NonZeroUsize = match NonZeroUsize::new(8) {
    Some(capacity) => capacity,
    None => unreachable!(),
};

#[derive(Debug)]
pub struct ArtCache {
    entries: LruCache<String, Art>,
    pending: HashSet<String>,
}

impl Default for ArtCache {
    fn default() -> Self {
        Self {
            entries: LruCache::new(CACHE_CAPACITY),
            pending: HashSet::new(),
        }
    }
}

impl ArtCache {
    /// The cover to show, without counting the look as a use: a view holds
    /// only `&self` and must not reorder the cache while composing a frame.
    #[must_use]
    pub fn peek(&self, url: &str) -> Option<&Art> {
        self.entries.peek(url)
    }

    /// Whether `url` has to be fetched: neither cached nor already on its
    /// way. A `true` marks it as on its way. A cache hit counts as a use,
    /// promoting it so a track that stays in view survives eviction.
    pub fn request(&mut self, url: &str) -> bool {
        self.entries.get(url).is_none() && self.pending.insert(url.to_owned())
    }

    /// Store a finished fetch, evicting the least recently used cover past
    /// the capacity.
    pub fn insert(&mut self, url: String, art: Art) {
        self.pending.remove(&url);
        self.entries.put(url, art);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::image;

    fn handle() -> Art {
        Art::Loaded(image::Handle::from_rgba(1, 1, vec![0; 4]))
    }

    #[test]
    fn a_url_is_requested_once_until_it_lands() {
        let mut cache = ArtCache::default();
        assert!(cache.request("a"));
        assert!(!cache.request("a"));
        cache.insert("a".to_string(), handle());
        assert!(!cache.request("a"));
        assert!(matches!(cache.peek("a"), Some(Art::Loaded(_))));
    }

    #[test]
    fn a_failed_fetch_is_not_retried() {
        let mut cache = ArtCache::default();
        assert!(cache.request("a"));
        cache.insert("a".to_string(), Art::Failed);
        assert!(!cache.request("a"));
        assert!(matches!(cache.peek("a"), Some(Art::Failed)));
    }

    #[test]
    fn a_new_track_can_be_requested_while_another_is_pending() {
        let mut cache = ArtCache::default();
        assert!(cache.request("a"));
        assert!(cache.request("b"));
    }

    #[test]
    fn an_older_pending_url_is_not_requested_again() {
        let mut cache = ArtCache::default();
        assert!(cache.request("a"));
        assert!(cache.request("b"));
        assert!(!cache.request("a"));
    }

    #[test]
    fn the_least_recently_used_cover_is_evicted_past_capacity() {
        let mut cache = ArtCache::default();
        for i in 0..=CACHE_CAPACITY.get() {
            cache.insert(i.to_string(), handle());
        }
        assert!(cache.peek("0").is_none());
        assert!(cache.peek("1").is_some());
        assert!(cache.peek(&CACHE_CAPACITY.get().to_string()).is_some());
    }

    #[test]
    fn touching_the_oldest_cover_saves_it_from_eviction() {
        let mut cache = ArtCache::default();
        for i in 0..CACHE_CAPACITY.get() {
            cache.insert(i.to_string(), handle());
        }
        assert!(!cache.request("0"));
        cache.insert(CACHE_CAPACITY.get().to_string(), handle());
        assert!(cache.peek("0").is_some());
        assert!(cache.peek("1").is_none());
    }
}
