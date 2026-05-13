//! Pre-filter stack: bigram pre-filter narrows file candidates, then the bloom
//! cache confirms identifier presence before the (possibly expensive) full
//! search backend runs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use ffs_symbol::BloomFilterCache;

/// A file that passed the pre-filter stack and is ready for the heavy search.
#[derive(Debug, Clone)]
pub struct PreFilteredCandidate {
    pub path: PathBuf,
    pub mtime: SystemTime,
}

/// Stack of pre-filters. The bigram check is owned by `fff-core`; here we
/// expose a tiny adaptor that takes pre-narrowed candidates plus the bloom
/// cache and returns the survivors.
pub struct PreFilterStack {
    pub bloom: Arc<BloomFilterCache>,
}

impl PreFilterStack {
    /// Build with a shared bloom cache.
    #[must_use]
    pub fn new(bloom: Arc<BloomFilterCache>) -> Self {
        Self { bloom }
    }

    /// For each `(path, mtime)` in `candidates`, ask the bloom cache whether
    /// `symbol` *might* appear. Files where the answer is "definitely no" are
    /// dropped.
    #[must_use]
    pub fn confirm_symbol(
        &self,
        candidates: &[(PathBuf, SystemTime, String)],
        symbol: &str,
    ) -> Vec<PreFilteredCandidate> {
        candidates
            .iter()
            .filter_map(|(path, mtime, content)| {
                if self.bloom.contains(path, *mtime, content, symbol) {
                    Some(PreFilteredCandidate {
                        path: path.clone(),
                        mtime: *mtime,
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_files_without_symbol() {
        let cache = Arc::new(BloomFilterCache::new());
        let stack = PreFilterStack::new(cache);

        let mtime = SystemTime::UNIX_EPOCH;
        let candidates = vec![
            (
                PathBuf::from("/tmp/has_symbol.rs"),
                mtime,
                "fn target() {}".to_string(),
            ),
            (
                PathBuf::from("/tmp/no_symbol.rs"),
                mtime,
                "fn other() {}".to_string(),
            ),
        ];

        let survivors = stack.confirm_symbol(&candidates, "target");
        assert_eq!(survivors.len(), 1);
        assert!(survivors[0].path.ends_with("has_symbol.rs"));
    }
}
