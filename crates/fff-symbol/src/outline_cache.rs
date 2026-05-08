//! Concurrent per-file outline cache with mtime invalidation.
//!
//! Outlines are expensive (full tree-sitter parse) so we cache `Vec<OutlineEntry>`
//! keyed by path. Like the bloom cache, we wholesale-clear on overflow rather than
//! tracking per-entry LRU — outline workloads have low temporal locality.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use dashmap::DashMap;

use crate::outline::get_outline_entries;
use crate::types::{Lang, OutlineEntry};

/// Default outline-cache cap (entries). One outline ≈ 0.5–4 KB; 5k ≈ a few MB.
pub const DEFAULT_OUTLINE_CACHE_LIMIT: usize = 5_000;

/// Concurrent outline cache. Cheap to clone via `Arc<OutlineCache>`.
pub struct OutlineCache {
    map: DashMap<PathBuf, (SystemTime, Vec<OutlineEntry>)>,
    count: AtomicUsize,
    limit: AtomicUsize,
}

impl Default for OutlineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl OutlineCache {
    /// Create a new cache with the default cap.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_OUTLINE_CACHE_LIMIT)
    }

    /// Create a new cache with a specific cap.
    #[must_use]
    pub fn with_limit(limit: usize) -> Self {
        Self {
            map: DashMap::new(),
            count: AtomicUsize::new(0),
            limit: AtomicUsize::new(limit),
        }
    }

    /// Update the cap; called by `MemoryGuard` to shrink under pressure.
    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Relaxed);
    }

    /// Number of cached outlines.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop all cached outlines.
    pub fn clear(&self) {
        self.map.clear();
        self.count.store(0, Ordering::Relaxed);
    }

    /// Get or compute the outline for `(path, mtime, content, lang)`.
    pub fn get_or_compute(
        &self,
        path: &Path,
        mtime: SystemTime,
        content: &str,
        lang: Lang,
    ) -> Vec<OutlineEntry> {
        if let Some(entry) = self.map.get(path) {
            let (cached_mtime, ref outline) = *entry;
            if cached_mtime == mtime {
                return outline.clone();
            }
        }

        let outline = get_outline_entries(content, lang);

        let limit = self.limit.load(Ordering::Relaxed);
        if self.count.load(Ordering::Relaxed) >= limit {
            self.map.clear();
            self.count.store(0, Ordering::Relaxed);
        }

        if self
            .map
            .insert(path.to_path_buf(), (mtime, outline.clone()))
            .is_none()
        {
            self.count.fetch_add(1, Ordering::Relaxed);
        }

        outline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_returns_same_outline_for_same_mtime() {
        let cache = OutlineCache::new();
        let p = std::path::Path::new("/tmp/test_outline.rs");
        let code = "fn alpha() {}\nfn beta() {}\n";
        let mtime = SystemTime::UNIX_EPOCH;

        let first = cache.get_or_compute(p, mtime, code, Lang::Rust);
        let second = cache.get_or_compute(p, mtime, code, Lang::Rust);
        assert_eq!(first.len(), second.len());
        assert_eq!(first.len(), 2);
    }

    #[test]
    fn cache_invalidates_on_mtime_change() {
        let cache = OutlineCache::new();
        let p = std::path::Path::new("/tmp/test_outline_invalidate.rs");
        let code1 = "fn alpha() {}\n";
        let code2 = "fn alpha() {}\nfn beta() {}\n";

        let mtime1 = SystemTime::UNIX_EPOCH;
        let mtime2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);

        let first = cache.get_or_compute(p, mtime1, code1, Lang::Rust);
        assert_eq!(first.len(), 1);
        let second = cache.get_or_compute(p, mtime2, code2, Lang::Rust);
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn cache_set_limit() {
        let cache = OutlineCache::with_limit(10);
        cache.set_limit(20);
        assert_eq!(cache.limit.load(Ordering::Relaxed), 20);
    }
}
