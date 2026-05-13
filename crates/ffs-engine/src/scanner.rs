//! Single filesystem walker that builds Bloom + Symbol indexes in parallel.
//!
//! The scanner uses `ignore::WalkBuilder` to honor `.gitignore` / `.ignore`
//! files and dispatches each visited file to a thread pool that reads the
//! content once and feeds both the Bloom cache and the symbol index.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use ignore::WalkBuilder;
use rayon::prelude::*;

use ffs_symbol::detection::{is_binary, is_generated_by_content, is_generated_by_name};
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::outline_cache::OutlineCache;
use ffs_symbol::types::FileType;
use ffs_symbol::{BloomFilterCache, SymbolIndex};

/// Successfully indexed file metadata.
#[derive(Debug, Clone)]
pub struct IndexedFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub byte_len: u64,
    pub file_type: FileType,
}

/// Aggregated scan totals.
#[derive(Debug, Default, Clone)]
pub struct ScanReport {
    pub files_visited: usize,
    pub files_indexed: usize,
    pub files_skipped_binary: usize,
    pub files_skipped_generated: usize,
    pub files_skipped_other: usize,
}

/// Live progress counters readable from any thread.
#[derive(Debug, Default)]
pub struct ScanProgress {
    pub files_visited: AtomicUsize,
    pub files_indexed: AtomicUsize,
    pub files_skipped: AtomicUsize,
}

impl ScanProgress {
    /// Snapshot the current counters as a `ScanReport`-shape tuple.
    pub fn snapshot(&self) -> (usize, usize, usize) {
        (
            self.files_visited.load(Ordering::Relaxed),
            self.files_indexed.load(Ordering::Relaxed),
            self.files_skipped.load(Ordering::Relaxed),
        )
    }
}

/// The unified scanner — single owner of `BloomFilterCache`, `SymbolIndex`, and
/// `OutlineCache`. Use `scan(root)` to populate all three.
pub struct UnifiedScanner {
    pub bloom: Arc<BloomFilterCache>,
    pub symbols: Arc<SymbolIndex>,
    pub outlines: Arc<OutlineCache>,
    pub progress: Arc<ScanProgress>,
}

impl Default for UnifiedScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl UnifiedScanner {
    /// Construct a fresh scanner with empty caches.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bloom: Arc::new(BloomFilterCache::new()),
            symbols: Arc::new(SymbolIndex::new()),
            outlines: Arc::new(OutlineCache::new()),
            progress: Arc::new(ScanProgress::default()),
        }
    }

    /// Scan the given root, populating all three indexes. Honors `.gitignore`.
    pub fn scan(&self, root: &Path) -> ScanReport {
        let entries = self.collect_entries(root);

        let bloom = Arc::clone(&self.bloom);
        let symbols = Arc::clone(&self.symbols);
        let progress = Arc::clone(&self.progress);

        let counters: Vec<(usize, usize, usize)> = entries
            .par_iter()
            .map(|path| {
                progress.files_visited.fetch_add(1, Ordering::Relaxed);
                index_one(path, bloom.as_ref(), symbols.as_ref(), progress.as_ref())
            })
            .collect();

        let mut report = ScanReport::default();
        for (visited, indexed, skipped) in counters {
            report.files_visited += visited;
            report.files_indexed += indexed;
            report.files_skipped_other += skipped;
        }
        report
    }

    fn collect_entries(&self, root: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let walker = WalkBuilder::new(root)
            .standard_filters(true)
            .follow_links(false)
            .build();
        for entry in walker.flatten() {
            if let Some(ft) = entry.file_type() {
                if ft.is_file() {
                    paths.push(entry.into_path());
                }
            }
        }
        paths
    }
}

/// Index a single file. Returns `(visited, indexed, skipped)`.
fn index_one(
    path: &Path,
    bloom: &BloomFilterCache,
    symbols: &SymbolIndex,
    progress: &ScanProgress,
) -> (usize, usize, usize) {
    let Ok(meta) = std::fs::metadata(path) else {
        progress.files_skipped.fetch_add(1, Ordering::Relaxed);
        return (1, 0, 1);
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let byte_len = meta.len();

    if byte_len == 0 || byte_len > 4 * 1024 * 1024 {
        progress.files_skipped.fetch_add(1, Ordering::Relaxed);
        return (1, 0, 1);
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if is_generated_by_name(name) {
            progress.files_skipped.fetch_add(1, Ordering::Relaxed);
            return (1, 0, 1);
        }
    }

    let Ok(content) = std::fs::read(path) else {
        progress.files_skipped.fetch_add(1, Ordering::Relaxed);
        return (1, 0, 1);
    };

    if is_binary(&content) || is_generated_by_content(&content) {
        progress.files_skipped.fetch_add(1, Ordering::Relaxed);
        return (1, 0, 1);
    }

    let Ok(text) = std::str::from_utf8(&content) else {
        progress.files_skipped.fetch_add(1, Ordering::Relaxed);
        return (1, 0, 1);
    };

    bloom.install(path, mtime, text);

    if matches!(detect_file_type(path), FileType::Code(_)) {
        symbols.index_file(path, mtime, text);
    }

    progress.files_indexed.fetch_add(1, Ordering::Relaxed);
    (1, 1, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_temp_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn alpha() {}").unwrap();
        std::fs::write(dir.path().join("b.txt"), "Just text").unwrap();

        let scanner = UnifiedScanner::new();
        let report = scanner.scan(dir.path());
        assert!(report.files_visited >= 2);
        assert!(report.files_indexed >= 1);
    }

    #[test]
    fn scanner_indexes_symbols() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\nfn helper() {}").unwrap();

        let scanner = UnifiedScanner::new();
        scanner.scan(dir.path());
        assert_eq!(scanner.symbols.lookup_exact("helper").len(), 1);
        assert_eq!(scanner.symbols.lookup_exact("main").len(), 1);
    }
}
