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
    /// Directory-walk errors (permission denied, IO failures) that `ignore`
    /// reported and we skipped — previously swallowed by `flatten()`, which
    /// made partially-covered scans invisible.
    pub files_walk_errors: usize,
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

    /// Construct a scanner that reuses an externally-owned symbol index. The
    /// bloom and outline caches are still fresh — callers expect those to be
    /// rebuilt lazily from file content, while the symbol index is the
    /// expensive piece we want to load from disk.
    #[must_use]
    pub fn with_symbols(symbols: Arc<SymbolIndex>) -> Self {
        Self {
            bloom: Arc::new(BloomFilterCache::new()),
            symbols,
            outlines: Arc::new(OutlineCache::new()),
            progress: Arc::new(ScanProgress::default()),
        }
    }

    /// Scan the given root, populating all three indexes. Honors `.gitignore`.
    pub fn scan(&self, root: &Path) -> ScanReport {
        let (entries, walk_errors) = self.collect_entries(root);

        let bloom = Arc::clone(&self.bloom);
        let symbols = Arc::clone(&self.symbols);
        let progress = Arc::clone(&self.progress);

        let counters: Vec<SkipCounts> = entries
            .par_iter()
            .map(|path| {
                progress.files_visited.fetch_add(1, Ordering::Relaxed);
                index_one(path, bloom.as_ref(), symbols.as_ref(), progress.as_ref())
            })
            .collect();

        let mut report = ScanReport {
            files_walk_errors: walk_errors,
            ..ScanReport::default()
        };
        for counts in counters {
            report.files_visited += counts.visited;
            report.files_indexed += counts.indexed;
            report.files_skipped_binary += counts.binary;
            report.files_skipped_generated += counts.generated;
            report.files_skipped_other += counts.other;
        }
        report
    }

    /// Walk `root` collecting file paths. Returns `(paths, walk_errors)` —
    /// `ignore` reports per-entry errors (permission denied, vanished dirs)
    /// that `flatten()` used to drop silently.
    fn collect_entries(&self, root: &Path) -> (Vec<PathBuf>, usize) {
        let mut paths = Vec::new();
        let mut walk_errors = 0usize;
        let walker = WalkBuilder::new(root)
            .standard_filters(true)
            .follow_links(false)
            .build();
        for entry in walker {
            match entry {
                Ok(e) => {
                    if let Some(ft) = e.file_type() {
                        if ft.is_file() {
                            paths.push(e.into_path());
                        }
                    }
                }
                Err(_) => walk_errors += 1,
            }
        }
        (paths, walk_errors)
    }
}

/// Per-file outcome counters returned by [`index_one`].
#[derive(Default)]
struct SkipCounts {
    visited: usize,
    indexed: usize,
    binary: usize,
    generated: usize,
    other: usize,
}

impl SkipCounts {
    fn skipped(kind: SkipKind) -> Self {
        let mut c = Self {
            visited: 1,
            ..Self::default()
        };
        match kind {
            SkipKind::Binary => c.binary = 1,
            SkipKind::Generated => c.generated = 1,
            SkipKind::Other => c.other = 1,
        }
        c
    }
}

enum SkipKind {
    Binary,
    Generated,
    Other,
}

/// Index a single file. Returns per-category visit/index/skip counts.
fn index_one(
    path: &Path,
    bloom: &BloomFilterCache,
    symbols: &SymbolIndex,
    progress: &ScanProgress,
) -> SkipCounts {
    macro_rules! skip {
        ($kind:expr) => {{
            progress.files_skipped.fetch_add(1, Ordering::Relaxed);
            return SkipCounts::skipped($kind);
        }};
    }

    let Ok(meta) = std::fs::metadata(path) else {
        skip!(SkipKind::Other);
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let byte_len = meta.len();

    // >4 MiB files stay excluded from the Bloom/symbol indexes (indexing
    // cost); grep still searches them. UTF-16 and invalid-UTF-8 text are
    // transcoded/lossied below so coverage matches the grep path.
    if byte_len == 0 || byte_len > 4 * 1024 * 1024 {
        skip!(SkipKind::Other);
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if is_generated_by_name(name) {
            skip!(SkipKind::Generated);
        }
    }

    let Ok(content) = std::fs::read(path) else {
        skip!(SkipKind::Other);
    };

    // UTF-16 text is transcoded before indexing — raw UTF-16 is full of
    // NULs (would flag binary) and its bigrams can't match a UTF-8 query.
    let text = match ffs_search::bom::decode_utf16(&content) {
        Some(t) => t,
        None => {
            if is_binary(&content) {
                skip!(SkipKind::Binary);
            }
            if is_generated_by_content(&content) {
                skip!(SkipKind::Generated);
            }
            // Lossy decode keeps coverage parity with grep: a stray
            // non-UTF-8 byte must not drop the whole file from the indexes.
            String::from_utf8_lossy(&content).into_owned()
        }
    };

    bloom.install(path, mtime, &text);

    if matches!(detect_file_type(path), FileType::Code(_)) {
        symbols.index_file(path, mtime, &text);
    }

    progress.files_indexed.fetch_add(1, Ordering::Relaxed);
    SkipCounts {
        visited: 1,
        indexed: 1,
        ..SkipCounts::default()
    }
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
