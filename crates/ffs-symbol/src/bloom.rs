//! Per-file Bloom filters for fast "does file X contain symbol Y?" queries.
//!
//! A Bloom filter can definitively say "no" (symbol is NOT in this file) but
//! may produce false positives. Identifier extraction uses a simple byte-level
//! state machine — no tree-sitter needed — making it fast enough to run on
//! every uncached file.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dashmap::DashMap;

/// A probabilistic set membership data structure.
pub struct BloomFilter {
    bits: Vec<u64>,
    num_hashes: u8,
    num_bits: usize,
}

impl BloomFilter {
    /// Create a new Bloom filter sized for `expected_items` with the given target FPR.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn new(expected_items: usize, target_fpr: f64) -> Self {
        assert!(expected_items > 0, "expected_items must be > 0");
        assert!(
            target_fpr > 0.0 && target_fpr < 1.0,
            "target_fpr must be in (0, 1)"
        );

        let n = expected_items as f64;
        let ln2 = std::f64::consts::LN_2;

        // Optimal number of bits: m = -(n * ln(p)) / (ln(2)^2)
        let m = (-(n * target_fpr.ln()) / (ln2 * ln2)).ceil() as usize;
        let m = m.max(64);

        // Optimal number of hash functions: k = (m/n) * ln(2)
        let k = ((m as f64 / n) * ln2).ceil() as u8;
        let k = k.clamp(1, 32);

        let num_words = m.div_ceil(64);
        let num_bits = num_words * 64;

        Self {
            bits: vec![0u64; num_words],
            num_hashes: k,
            num_bits,
        }
    }

    /// Insert an item into the filter.
    pub fn insert(&mut self, item: &str) {
        let (h1, h2) = double_hash(item);
        for i in 0..u64::from(self.num_hashes) {
            let idx = combined_hash(h1, h2, i, self.num_bits);
            let word = idx / 64;
            let bit = idx % 64;
            self.bits[word] |= 1u64 << bit;
        }
    }

    /// Check if an item is probably in the filter.
    #[must_use]
    pub fn contains(&self, item: &str) -> bool {
        let (h1, h2) = double_hash(item);
        for i in 0..u64::from(self.num_hashes) {
            let idx = combined_hash(h1, h2, i, self.num_bits);
            let word = idx / 64;
            let bit = idx % 64;
            if self.bits[word] & (1u64 << bit) == 0 {
                return false;
            }
        }
        true
    }

    /// Approximate memory footprint in bytes (bit array only).
    #[must_use]
    pub fn approx_bytes(&self) -> usize {
        self.bits.len() * std::mem::size_of::<u64>()
    }

    /// Number of hash functions used.
    #[must_use]
    pub fn num_hashes(&self) -> u8 {
        self.num_hashes
    }

    /// Total number of bits in the underlying array.
    #[must_use]
    pub fn num_bits(&self) -> usize {
        self.num_bits
    }
}

fn double_hash(item: &str) -> (u64, u64) {
    let h1 = hash_with_seed(item, 0);
    let h2 = hash_with_seed(item, 0x517c_c1b7_2722_0a95);
    (h1, h2)
}

fn combined_hash(h1: u64, h2: u64, i: u64, num_bits: usize) -> usize {
    let hash = h1.wrapping_add(i.wrapping_mul(h2));
    (hash % num_bits as u64) as usize
}

fn hash_with_seed(item: &str, seed: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    item.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// BloomFilterCache — LRU-bounded by simple counter wholesale-clear
// ---------------------------------------------------------------------------

/// Default cap before the cache wholesale-clears. Each filter is ~1–2KB; 10k
/// entries ≈ 10–20MB. Walker workloads have no temporal locality, so a clear
/// is acceptable since refilling is the same cost as the original scan.
pub const DEFAULT_FILTER_CACHE_LIMIT: usize = 10_000;

/// Thread-safe cache of per-file Bloom filters keyed by path and validated by mtime.
pub struct BloomFilterCache {
    filters: DashMap<PathBuf, (BloomFilter, SystemTime)>,
    count: std::sync::atomic::AtomicUsize,
    limit: std::sync::atomic::AtomicUsize,
}

impl Default for BloomFilterCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BloomFilterCache {
    /// Create an empty cache with the default 10k entry limit.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_FILTER_CACHE_LIMIT)
    }

    /// Create an empty cache with a specific entry cap.
    #[must_use]
    pub fn with_limit(limit: usize) -> Self {
        Self {
            filters: DashMap::new(),
            count: std::sync::atomic::AtomicUsize::new(0),
            limit: std::sync::atomic::AtomicUsize::new(limit),
        }
    }

    /// Update the cache cap dynamically (used by MemoryGuard to shrink).
    pub fn set_limit(&self, limit: usize) {
        self.limit
            .store(limit, std::sync::atomic::Ordering::Relaxed);
    }

    /// Number of currently cached filters.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop all cached filters.
    pub fn clear(&self) {
        self.filters.clear();
        self.count.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if `symbol` might appear in the file at `path`.
    #[must_use]
    pub fn contains(&self, path: &Path, mtime: SystemTime, content: &str, symbol: &str) -> bool {
        use std::sync::atomic::Ordering;

        if let Some(entry) = self.filters.get(path) {
            let (ref filter, cached_mtime) = *entry;
            if cached_mtime == mtime {
                return filter.contains(symbol);
            }
        }

        let filter = build_filter(content);
        let result = filter.contains(symbol);

        let limit = self.limit.load(Ordering::Relaxed);
        if self.count.load(Ordering::Relaxed) >= limit {
            self.filters.clear();
            self.count.store(0, Ordering::Relaxed);
        }

        if self
            .filters
            .insert(path.to_path_buf(), (filter, mtime))
            .is_none()
        {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// Pre-populate the cache with a freshly-built filter, used by the unified
    /// scanner so we do not rebuild the same filter at query time.
    pub fn install(&self, path: &Path, mtime: SystemTime, content: &str) {
        use std::sync::atomic::Ordering;

        let filter = build_filter(content);
        let limit = self.limit.load(Ordering::Relaxed);
        if self.count.load(Ordering::Relaxed) >= limit {
            self.filters.clear();
            self.count.store(0, Ordering::Relaxed);
        }
        if self
            .filters
            .insert(path.to_path_buf(), (filter, mtime))
            .is_none()
        {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Build a Bloom filter from file content by extracting all identifiers.
pub fn build_filter(content: &str) -> BloomFilter {
    let idents: Vec<&str> = extract_identifiers(content).collect();
    let expected = idents.len().max(1);

    let mut filter = BloomFilter::new(expected, 0.01);
    for ident in idents {
        filter.insert(ident);
    }
    filter
}

// ---------------------------------------------------------------------------
// Identifier extraction (byte-level state machine)
// ---------------------------------------------------------------------------

/// Extract identifier tokens from source code using a byte-level state machine.
/// Skips string literals and block/line comments. Identifier ≈ `[a-zA-Z_][a-zA-Z0-9_]*`.
pub fn extract_identifiers(content: &str) -> impl Iterator<Item = &str> {
    IdentifierIter::new(content)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Code,
    StringDouble,
    StringSingle,
    StringBacktick,
    LineComment,
    BlockComment,
}

struct IdentifierIter<'a> {
    bytes: &'a [u8],
    src: &'a str,
    pos: usize,
    state: ScanState,
}

impl<'a> IdentifierIter<'a> {
    fn new(content: &'a str) -> Self {
        Self {
            bytes: content.as_bytes(),
            src: content,
            pos: 0,
            state: ScanState::Code,
        }
    }
}

impl<'a> Iterator for IdentifierIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let bytes = self.bytes;
        let len = bytes.len();

        while self.pos < len {
            let i = self.pos;
            let b = bytes[i];

            match self.state {
                ScanState::Code => {
                    if b == b'"' {
                        self.state = ScanState::StringDouble;
                        self.pos += 1;
                        continue;
                    }
                    if b == b'\'' {
                        self.state = ScanState::StringSingle;
                        self.pos += 1;
                        continue;
                    }
                    if b == b'`' {
                        self.state = ScanState::StringBacktick;
                        self.pos += 1;
                        continue;
                    }

                    if b == b'/' && i + 1 < len {
                        if bytes[i + 1] == b'/' {
                            self.state = ScanState::LineComment;
                            self.pos += 2;
                            continue;
                        }
                        if bytes[i + 1] == b'*' {
                            self.state = ScanState::BlockComment;
                            self.pos += 2;
                            continue;
                        }
                    }

                    if is_ident_start(b) {
                        let start = i;
                        self.pos += 1;
                        while self.pos < len && is_ident_continue(bytes[self.pos]) {
                            self.pos += 1;
                        }
                        return Some(&self.src[start..self.pos]);
                    }

                    self.pos += 1;
                }

                ScanState::StringDouble => {
                    if b == b'\\' && i + 1 < len {
                        self.pos += 2;
                    } else if b == b'"' {
                        self.state = ScanState::Code;
                        self.pos += 1;
                    } else {
                        self.pos += 1;
                    }
                }

                ScanState::StringSingle => {
                    if b == b'\\' && i + 1 < len {
                        self.pos += 2;
                    } else if b == b'\'' {
                        self.state = ScanState::Code;
                        self.pos += 1;
                    } else {
                        self.pos += 1;
                    }
                }

                ScanState::StringBacktick => {
                    if b == b'\\' && i + 1 < len {
                        self.pos += 2;
                    } else if b == b'`' {
                        self.state = ScanState::Code;
                        self.pos += 1;
                    } else {
                        self.pos += 1;
                    }
                }

                ScanState::LineComment => {
                    if b == b'\n' {
                        self.state = ScanState::Code;
                    }
                    self.pos += 1;
                }

                ScanState::BlockComment => {
                    if b == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                        self.state = ScanState::Code;
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
            }
        }

        None
    }
}

#[inline]
fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

#[inline]
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_membership() {
        let mut bf = BloomFilter::new(100, 0.01);
        bf.insert("foo");
        bf.insert("bar");
        assert!(bf.contains("foo"));
        assert!(bf.contains("bar"));
    }

    #[test]
    fn definitely_not_present() {
        let mut bf = BloomFilter::new(10, 0.01);
        bf.insert("alpha");
        bf.insert("beta");
        bf.insert("gamma");

        let mut false_positives = 0;
        let test_items = [
            "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa", "lambda", "mu", "nu",
            "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon", "phi", "chi", "psi", "omega",
        ];
        for item in &test_items {
            if bf.contains(item) {
                false_positives += 1;
            }
        }
        assert!(
            false_positives <= 1,
            "too many false positives: {false_positives}"
        );
    }

    #[test]
    fn identifier_extraction() {
        let code = "fn foo(bar: Baz) { qux() }";
        let idents: Vec<&str> = extract_identifiers(code).collect();
        assert_eq!(idents, vec!["fn", "foo", "bar", "Baz", "qux"]);
    }

    #[test]
    fn identifier_extraction_skips_strings() {
        let code = r#"let x = "hello world"; let y = 42;"#;
        let idents: Vec<&str> = extract_identifiers(code).collect();
        assert!(idents.contains(&"let"));
        assert!(idents.contains(&"x"));
        assert!(idents.contains(&"y"));
        assert!(!idents.contains(&"hello"));
        assert!(!idents.contains(&"world"));
    }

    #[test]
    fn identifier_extraction_skips_comments() {
        let code = "fn real() // fn fake()\n/* fn also_fake() */\nfn another()";
        let idents: Vec<&str> = extract_identifiers(code).collect();
        assert!(idents.contains(&"real"));
        assert!(idents.contains(&"another"));
        assert!(!idents.contains(&"fake"));
        assert!(!idents.contains(&"also_fake"));
    }

    #[test]
    fn cache_mtime_invalidation() {
        let cache = BloomFilterCache::new();
        let path = std::path::Path::new("/tmp/test_bloom.rs");

        let old_content = "fn old_function() {}";
        let new_content = "fn new_function() {}";

        let mtime_old = SystemTime::UNIX_EPOCH;
        let mtime_new = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);

        assert!(cache.contains(path, mtime_old, old_content, "old_function"));
        assert!(!cache.contains(path, mtime_old, old_content, "new_function"));
        assert!(cache.contains(path, mtime_old, new_content, "old_function"));
        assert!(cache.contains(path, mtime_new, new_content, "new_function"));
        assert!(!cache.contains(path, mtime_new, new_content, "old_function"));
    }

    #[test]
    fn cache_install_then_query() {
        let cache = BloomFilterCache::new();
        let path = std::path::Path::new("/tmp/test_install.rs");
        cache.install(path, SystemTime::UNIX_EPOCH, "fn alpha() { beta() }");
        assert!(cache.contains(path, SystemTime::UNIX_EPOCH, "ignored", "alpha"));
        assert!(cache.contains(path, SystemTime::UNIX_EPOCH, "ignored", "beta"));
        assert!(!cache.contains(
            path,
            SystemTime::UNIX_EPOCH,
            "ignored",
            "definitely_not_there"
        ));
    }
}
