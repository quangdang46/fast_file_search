//! Memory-budget enforcement.
//!
//! Per the architecture's `Q3` table:
//! | Repo size | RAM target | Strategy |
//! |---|---|---|
//! | ≤10k files | ≤128 MB | full Bigram + full Bloom |
//! | ≤100k files | ≤512 MB | full Bigram + Bloom LRU 10k |
//! | ≤500k files | ≤1 GB | full Bigram + Bloom LRU 5k + drop OutlineCache |
//! | >500k | streaming-only | drop SymbolIndex |

use std::sync::Arc;

use fff_symbol::outline_cache::OutlineCache;
use fff_symbol::{BloomFilterCache, SymbolIndex};

/// Repo-size bucket for budget allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoSize {
    Small,
    Medium,
    Large,
    Huge,
}

impl RepoSize {
    /// Classify by total file count.
    #[must_use]
    pub fn from_file_count(n: usize) -> Self {
        match n {
            0..=10_000 => Self::Small,
            10_001..=100_000 => Self::Medium,
            100_001..=500_000 => Self::Large,
            _ => Self::Huge,
        }
    }
}

/// Owns the shared caches and applies budget rules to them.
pub struct MemoryGuard {
    pub bloom: Arc<BloomFilterCache>,
    pub symbols: Arc<SymbolIndex>,
    pub outlines: Arc<OutlineCache>,
}

impl MemoryGuard {
    /// Build with shared cache references — usually those owned by `UnifiedScanner`.
    #[must_use]
    pub fn new(
        bloom: Arc<BloomFilterCache>,
        symbols: Arc<SymbolIndex>,
        outlines: Arc<OutlineCache>,
    ) -> Self {
        Self {
            bloom,
            symbols,
            outlines,
        }
    }

    /// Apply the strategy for `size`, possibly clearing or shrinking caches.
    pub fn apply(&self, size: RepoSize) {
        match size {
            RepoSize::Small => {
                self.bloom.set_limit(usize::MAX / 2);
                self.outlines.set_limit(usize::MAX / 2);
            }
            RepoSize::Medium => {
                self.bloom.set_limit(10_000);
                self.outlines.set_limit(5_000);
            }
            RepoSize::Large => {
                self.bloom.set_limit(5_000);
                self.outlines.set_limit(0);
                self.outlines.clear();
            }
            RepoSize::Huge => {
                self.bloom.set_limit(2_000);
                self.outlines.set_limit(0);
                self.outlines.clear();
                self.symbols.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_repos() {
        assert_eq!(RepoSize::from_file_count(0), RepoSize::Small);
        assert_eq!(RepoSize::from_file_count(10_000), RepoSize::Small);
        assert_eq!(RepoSize::from_file_count(50_000), RepoSize::Medium);
        assert_eq!(RepoSize::from_file_count(200_000), RepoSize::Large);
        assert_eq!(RepoSize::from_file_count(1_000_000), RepoSize::Huge);
    }

    #[test]
    fn apply_clears_outline_for_large() {
        let bloom = Arc::new(BloomFilterCache::new());
        let symbols = Arc::new(SymbolIndex::new());
        let outlines = Arc::new(OutlineCache::new());
        outlines.get_or_compute(
            std::path::Path::new("/tmp/some.rs"),
            std::time::SystemTime::UNIX_EPOCH,
            "fn x() {}",
            fff_symbol::types::Lang::Rust,
        );
        assert_eq!(outlines.len(), 1);

        let guard = MemoryGuard::new(bloom, symbols, outlines.clone());
        guard.apply(RepoSize::Large);
        assert_eq!(outlines.len(), 0);
    }
}
